use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use tauri::{
    image::Image,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tauri_plugin_positioner::{Position, WindowExt};

const TRAY_ID: &str = "tokn-tray";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Treat tokens expiring within this margin as expired.
const EXPIRY_MARGIN_MS: u64 = 60_000;
/// ~6h of samples at the 60s refresh interval.
const HISTORY_CAP: usize = 360;

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum AuthStatus {
    Connected,
    MissingToken,
    ExpiredToken,
    KeychainDenied,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UsageWindow {
    label: String,
    used_pct: f64,
    resets_at_ms: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UsageSnapshot {
    auth_status: AuthStatus,
    session: UsageWindow,
    weekly: UsageWindow,
    burn_rate: Vec<f64>,
    fetched_at_ms: u64,
}

/// (timestamp ms, five-hour utilization %) samples for the burn-rate sparkline.
struct UsageHistory(Mutex<VecDeque<(u64, f64)>>);

enum TokenRead {
    Ready(String),
    Missing,
    Expired,
    KeychainDenied,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as u64
}

/* ── credential ───────────────────────────────────────────────── */

#[derive(Deserialize)]
struct StoredCreds {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: StoredOauth,
}

#[derive(Deserialize)]
struct StoredOauth {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: u64,
}

/// Read the Claude Code OAuth access token from the macOS Keychain.
/// If the token is missing, expired, or blocked by macOS permissions, Tokn
/// stays in the gate state and asks the user to fix Claude Code login first.
fn read_access_token() -> Result<TokenRead, String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .map_err(|e| format!("failed to run security: {e}"))?;
    if !output.status.success() {
        return Ok(classify_keychain_failure(&output));
    }
    let creds: StoredCreds = serde_json::from_slice(&output.stdout)
        .map_err(|_| "unexpected credential format in Keychain".to_string())?;
    if creds.claude_ai_oauth.expires_at <= now_ms() + EXPIRY_MARGIN_MS {
        return Ok(TokenRead::Expired);
    }
    Ok(TokenRead::Ready(creds.claude_ai_oauth.access_token))
}

fn classify_keychain_failure(output: &std::process::Output) -> TokenRead {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    )
    .to_ascii_lowercase();

    if text.contains("user interaction is not allowed")
        || text.contains("user canceled")
        || text.contains("denied")
        || text.contains("authorization")
        || text.contains("passphrase")
    {
        return TokenRead::KeychainDenied;
    }

    TokenRead::Missing
}

/* ── usage endpoint ───────────────────────────────────────────── */

#[derive(Deserialize)]
struct ApiWindow {
    utilization: f64,
    resets_at: String,
}

#[derive(Deserialize)]
struct ApiUsage {
    five_hour: Option<ApiWindow>,
    seven_day: Option<ApiWindow>,
}

enum UsageFetchError {
    Unauthorized,
    Other(String),
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("reqwest client")
    })
}

async fn fetch_usage(token: &str) -> Result<ApiUsage, UsageFetchError> {
    let resp = http_client()
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .map_err(|e| UsageFetchError::Other(format!("usage request failed: {e}")))?;
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(UsageFetchError::Unauthorized);
    }
    if !status.is_success() {
        return Err(UsageFetchError::Other(format!(
            "usage endpoint returned {status}"
        )));
    }
    resp.json::<ApiUsage>()
        .await
        .map_err(|e| UsageFetchError::Other(format!("unexpected usage response: {e}")))
}

fn parse_resets_at(iso: &str) -> u64 {
    time::OffsetDateTime::parse(iso, &time::format_description::well_known::Rfc3339)
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as u64)
        .unwrap_or(0)
}

fn to_window(label: &str, api: Option<ApiWindow>) -> UsageWindow {
    let (pct, resets) = api
        .map(|w| (w.utilization, parse_resets_at(&w.resets_at)))
        .unwrap_or((0.0, 0));
    UsageWindow {
        label: label.into(),
        used_pct: pct,
        resets_at_ms: resets,
    }
}

/* ── burn rate ────────────────────────────────────────────────── */

/// Record a session-utilization sample and return per-interval positive
/// deltas — the burn-rate velocity series for the sparkline. Negative deltas
/// (window reset) clamp to zero.
fn record_burn_sample(history: &UsageHistory, pct: f64, now: u64) -> Vec<f64> {
    let mut h = history.0.lock().expect("history lock poisoned");
    // Ignore samples arriving faster than the refresh cadence (manual
    // refresh spam would distort the velocity series).
    let should_push = h.back().is_none_or(|&(last_ts, _)| now >= last_ts + 30_000);
    if should_push {
        h.push_back((now, pct));
        if h.len() > HISTORY_CAP {
            h.pop_front();
        }
    }
    h.iter()
        .zip(h.iter().skip(1))
        .map(|((_, a), (_, b))| (b - a).max(0.0))
        .collect()
}

/* ── snapshot assembly ────────────────────────────────────────── */

fn disconnected_snapshot(auth_status: AuthStatus) -> UsageSnapshot {
    let now = now_ms();
    UsageSnapshot {
        auth_status,
        session: to_window("current session", None),
        weekly: to_window("weekly limit", None),
        burn_rate: Vec::new(),
        fetched_at_ms: now,
    }
}

async fn build_snapshot(history: &UsageHistory) -> Result<UsageSnapshot, String> {
    let token = match read_access_token()? {
        TokenRead::Ready(token) => token,
        TokenRead::Missing => return Ok(disconnected_snapshot(AuthStatus::MissingToken)),
        TokenRead::Expired => return Ok(disconnected_snapshot(AuthStatus::ExpiredToken)),
        TokenRead::KeychainDenied => {
            return Ok(disconnected_snapshot(AuthStatus::KeychainDenied));
        }
    };
    let usage = match fetch_usage(&token).await {
        Ok(usage) => usage,
        Err(UsageFetchError::Unauthorized) => {
            return Ok(disconnected_snapshot(AuthStatus::ExpiredToken));
        }
        Err(UsageFetchError::Other(error)) => return Err(error),
    };
    let now = now_ms();
    let session = to_window("current session", usage.five_hour);
    let weekly = to_window("weekly limit", usage.seven_day);
    let burn_rate = record_burn_sample(history, session.used_pct, now);
    Ok(UsageSnapshot {
        auth_status: AuthStatus::Connected,
        session,
        weekly,
        burn_rate,
        fetched_at_ms: now,
    })
}

fn sync_tray(app: &tauri::AppHandle, snapshot: &UsageSnapshot) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let hi = if matches!(snapshot.auth_status, AuthStatus::Connected) {
            snapshot.session.used_pct.max(snapshot.weekly.used_pct)
        } else {
            0.0
        };
        let _ = tray.set_icon(Some(tray_ring_image(hi)));
        let _ = tray.set_icon_as_template(true);
    }
}

#[tauri::command]
async fn usage_snapshot(
    app: tauri::AppHandle,
    history: tauri::State<'_, UsageHistory>,
) -> Result<UsageSnapshot, String> {
    let snapshot = build_snapshot(&history).await?;
    sync_tray(&app, &snapshot);
    Ok(snapshot)
}

/* ── tray icon ────────────────────────────────────────────────── */

/// Tray ring per the design spec: 270° sweep from 135°, full track at 0.30
/// alpha, fill arc to `pct`, center dot. Black + alpha — macOS template image.
fn tray_ring_image(pct: f64) -> Image<'static> {
    const SIZE: usize = 36; // 18pt @2x
    const SCALE: f64 = SIZE as f64 / 24.0; // design viewBox is 24
    let c = SIZE as f64 / 2.0;
    let r = 8.2 * SCALE;
    let half_stroke = 1.2 * SCALE;
    let dot_r = 1.5 * SCALE;
    const SWEEP: f64 = 270.0;
    const START: f64 = 135.0;
    let fill_deg = SWEEP * pct.clamp(0.0, 100.0) / 100.0;

    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f64 + 0.5 - c;
            let dy = y as f64 + 0.5 - c;
            let dist = (dx * dx + dy * dy).sqrt();

            // 1px anti-aliased coverage of the ring band / dot
            let ring_cov = (half_stroke - (dist - r).abs() + 0.5).clamp(0.0, 1.0);
            let dot_cov = (dot_r - dist + 0.5).clamp(0.0, 1.0);

            let deg = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            let rel = (deg - START).rem_euclid(360.0);

            let ring_alpha = if rel <= SWEEP {
                if rel <= fill_deg { 1.0 } else { 0.30 }
            } else {
                0.0
            };

            let alpha = (ring_cov * ring_alpha).max(dot_cov);
            rgba[(y * SIZE + x) * 4 + 3] = (alpha * 255.0).round() as u8;
        }
    }
    Image::new_owned(rgba, SIZE as u32, SIZE as u32)
}

/* ── window / app shell ───────────────────────────────────────── */

fn toggle_popover(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.move_window(Position::TrayBottomCenter);
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .manage(UsageHistory(Mutex::new(VecDeque::new())))
        .invoke_handler(tauri::generate_handler![usage_snapshot])
        .setup(|app| {
            // Menu bar app: no Dock icon, no app switcher entry.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Empty ring until the first successful fetch.
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_ring_image(0.0))
                .icon_as_template(true)
                .tooltip("Tokn")
                .on_tray_icon_event(|tray, event| {
                    tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_popover(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // Popover behavior: clicking anywhere else dismisses it.
            if let tauri::WindowEvent::Focused(false) = event {
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
