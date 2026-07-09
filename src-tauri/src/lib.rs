use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    LogicalPosition, LogicalSize, Manager, PhysicalPosition,
};
use tauri_plugin_positioner::{Position, WindowExt};

const TRAY_ID: &str = "tokn-tray";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Treat tokens expiring within this margin as expired.
const EXPIRY_MARGIN_MS: u64 = 60_000;
/// ~6h of samples at the 60s refresh interval.
const HISTORY_CAP: usize = 360;
/// Drop persisted samples older than the sparkline window on load.
const HISTORY_WINDOW_MS: u64 = 6 * 60 * 60 * 1000;
/// Minimum spacing between network fetches; calls inside it return the cache.
const MIN_FETCH_SPACING_MS: u64 = 30_000;
/// 429 backoff when the server sends no Retry-After: 2m → 4m → … capped 15m.
const BACKOFF_BASE_SECS: u64 = 120;
const BACKOFF_CAP_SECS: u64 = 900;
/// How often the running app re-checks GitHub Releases for a newer build.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

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
    /// Set while the usage endpoint is rate limiting us (429): the time the
    /// next fetch is allowed. The rest of the snapshot is the last good data.
    rate_limited_until_ms: Option<u64>,
    /// True when we have never fetched real data (e.g. rate limited on the
    /// very first call) — the windows are filler, not "0% used".
    placeholder: bool,
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
    /// 429 with the server's Retry-After (seconds), when present.
    RateLimited(Option<u64>),
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
    if status.as_u16() == 429 {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());
        return Err(UsageFetchError::RateLimited(retry_after));
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

/// Burn-rate history survives restarts so the sparkline doesn't reset to
/// "collecting data" on every launch.
fn history_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("history.json"))
}

fn load_history(app: &tauri::AppHandle) -> VecDeque<(u64, f64)> {
    let Some(path) = history_path(app) else {
        return VecDeque::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return VecDeque::new();
    };
    let Ok(mut samples) = serde_json::from_slice::<VecDeque<(u64, f64)>>(&bytes) else {
        return VecDeque::new();
    };
    let cutoff = now_ms().saturating_sub(HISTORY_WINDOW_MS);
    samples.retain(|&(ts, _)| ts <= now_ms() && ts >= cutoff);
    while samples.len() > HISTORY_CAP {
        samples.pop_front();
    }
    samples
}

fn save_history(app: &tauri::AppHandle, history: &UsageHistory) {
    let Some(path) = history_path(app) else {
        return;
    };
    let json = {
        let h = history.0.lock().expect("history lock poisoned");
        match serde_json::to_vec(&*h) {
            Ok(json) => json,
            Err(_) => return,
        }
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, json);
}

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

enum SnapshotError {
    RateLimited(Option<u64>),
    Other(String),
}

fn disconnected_snapshot(auth_status: AuthStatus) -> UsageSnapshot {
    let now = now_ms();
    UsageSnapshot {
        auth_status,
        session: to_window("current session", None),
        weekly: to_window("weekly limit", None),
        burn_rate: Vec::new(),
        fetched_at_ms: now,
        rate_limited_until_ms: None,
        placeholder: false,
    }
}

async fn build_snapshot(history: &UsageHistory) -> Result<UsageSnapshot, SnapshotError> {
    let token = match read_access_token().map_err(SnapshotError::Other)? {
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
        Err(UsageFetchError::RateLimited(retry_after)) => {
            return Err(SnapshotError::RateLimited(retry_after));
        }
        Err(UsageFetchError::Other(error)) => return Err(SnapshotError::Other(error)),
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
        rate_limited_until_ms: None,
        placeholder: false,
    })
}

/* ── fetch gate ───────────────────────────────────────────────── */

/// Serializes access to the usage endpoint: caches the last good snapshot,
/// enforces minimum spacing between network fetches, and tracks 429 backoff.
struct FetchGate(Mutex<GateState>);

#[derive(Default)]
struct GateState {
    last_snapshot: Option<UsageSnapshot>,
    /// No network fetch before this time; calls inside it serve the cache.
    next_fetch_at_ms: u64,
    /// Current 429 backoff (seconds); doubles per consecutive 429.
    backoff_secs: u64,
    /// Whether `next_fetch_at_ms` came from a 429 (vs. normal spacing).
    rate_limited: bool,
    /// A network fetch is in flight. Concurrent callers serve the cache
    /// instead of firing a second request (the spacing window isn't written
    /// until the fetch returns, so without this two overlapping calls would
    /// both hit the endpoint).
    in_flight: bool,
}

impl GateState {
    fn cached_response(&self) -> UsageSnapshot {
        let mut snap = self.last_snapshot.clone().unwrap_or_else(|| {
            // Never had real data (rate limited from the first call): the
            // zeros are filler, and the frontend must not show them as usage.
            let mut s = disconnected_snapshot(AuthStatus::Connected);
            s.placeholder = true;
            s
        });
        snap.rate_limited_until_ms = self.rate_limited.then_some(self.next_fetch_at_ms);
        snap
    }
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
    gate: tauri::State<'_, FetchGate>,
) -> Result<UsageSnapshot, String> {
    let now = now_ms();
    {
        let mut g = gate.0.lock().expect("gate lock poisoned");
        // Serve the cache when the spacing window is still open, or when
        // another call is already fetching — otherwise overlapping callers
        // (StrictMode double-mount, the 60s tick landing on a manual refresh,
        // the rate-limit-cleared refetch) would each hit the endpoint.
        let within_spacing =
            now < g.next_fetch_at_ms && (g.last_snapshot.is_some() || g.rate_limited);
        if within_spacing || g.in_flight {
            return Ok(g.cached_response());
        }
        g.in_flight = true;
    }

    let result = build_snapshot(&history).await;
    let mut g = gate.0.lock().expect("gate lock poisoned");
    g.in_flight = false;
    match result {
        Ok(snapshot) => {
            g.rate_limited = false;
            g.backoff_secs = 0;
            // Auth-gated states skip the spacing so the gate's retry
            // button reacts immediately once the user fixes login.
            g.next_fetch_at_ms = if matches!(snapshot.auth_status, AuthStatus::Connected) {
                now + MIN_FETCH_SPACING_MS
            } else {
                0
            };
            g.last_snapshot = Some(snapshot.clone());
            drop(g);
            sync_tray(&app, &snapshot);
            if matches!(snapshot.auth_status, AuthStatus::Connected) {
                save_history(&app, &history);
            }
            Ok(snapshot)
        }
        Err(SnapshotError::RateLimited(retry_after)) => {
            let wait_secs = retry_after.unwrap_or(if g.backoff_secs == 0 {
                BACKOFF_BASE_SECS
            } else {
                (g.backoff_secs * 2).min(BACKOFF_CAP_SECS)
            });
            g.backoff_secs = wait_secs;
            g.rate_limited = true;
            g.next_fetch_at_ms = now + wait_secs * 1000;
            Ok(g.cached_response())
        }
        Err(SnapshotError::Other(error)) => Err(error),
    }
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
                if rel <= fill_deg {
                    1.0
                } else {
                    0.30
                }
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

/// When and where the popover was last hidden by losing focus. A click on the
/// tray icon steals focus (hiding the popover) before the click event arrives,
/// so the click handler needs this to tell "toggle closed" from "open again".
struct PopoverHiddenAt(Mutex<Option<(Instant, PhysicalPosition<i32>)>>);

/// The same tray icon anchors the window to the same spot; allow a couple of
/// pixels of slack for rounding across monitors with different scale factors.
fn same_anchor(a: Option<PhysicalPosition<i32>>, b: Option<PhysicalPosition<i32>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a.x - b.x).abs() <= 2 && (a.y - b.y).abs() <= 2,
        _ => false,
    }
}

fn to_logical_pos(p: &tauri::Position, scale: f64) -> LogicalPosition<f64> {
    match *p {
        tauri::Position::Physical(p) => {
            LogicalPosition::new(p.x as f64 / scale, p.y as f64 / scale)
        }
        tauri::Position::Logical(p) => p,
    }
}

fn to_logical_size(s: &tauri::Size, scale: f64) -> LogicalSize<f64> {
    match *s {
        tauri::Size::Physical(s) => {
            LogicalSize::new(s.width as f64 / scale, s.height as f64 / scale)
        }
        tauri::Size::Logical(s) => s,
    }
}

/// Compute where the popover should open for a click on `tray_rect`, in
/// logical (points) coordinates: bottom-center of the clicked tray icon,
/// clamped to that monitor's edges.
///
/// The tray event reports the rect in physical pixels scaled by the clicked
/// screen's factor, so on mixed-DPI setups the same physical coordinates mean
/// different places on different monitors. Undo the scaling per candidate
/// monitor and accept the monitor whose logical bounds contain the icon.
fn popover_target(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    tray_rect: &tauri::Rect,
) -> Option<LogicalPosition<f64>> {
    const EDGE_MARGIN: f64 = 8.0;
    let win_w = to_logical_size(
        &tauri::Size::Physical(window.outer_size().ok()?),
        window.scale_factor().ok()?,
    )
    .width;

    for monitor in app.available_monitors().ok()? {
        let scale = monitor.scale_factor();
        let m_pos = monitor.position().to_logical::<f64>(scale);
        let m_size = monitor.size().to_logical::<f64>(scale);
        let tray_pos = to_logical_pos(&tray_rect.position, scale);
        let tray_size = to_logical_size(&tray_rect.size, scale);

        let cx = tray_pos.x + tray_size.width / 2.0;
        let cy = tray_pos.y + tray_size.height / 2.0;
        let inside = cx >= m_pos.x
            && cx <= m_pos.x + m_size.width
            && cy >= m_pos.y
            && cy <= m_pos.y + m_size.height;
        if !inside {
            continue;
        }

        let max_x = m_pos.x + m_size.width - win_w - EDGE_MARGIN;
        let x = (cx - win_w / 2.0).clamp(m_pos.x + EDGE_MARGIN, max_x.max(m_pos.x + EDGE_MARGIN));
        let y = tray_pos.y + tray_size.height;
        return Some(LogicalPosition::new(x, y));
    }
    None
}

fn toggle_popover(app: &tauri::AppHandle, tray_rect: &tauri::Rect) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let was_visible = window.is_visible().unwrap_or(false);
    let anchor_before = window.outer_position().ok();
    // Anchor to the tray icon that was clicked — each display has its own
    // menu bar. The positioner plugin is only the fallback: it mixes physical
    // coordinates across monitors, which misplaces the popover on mixed-DPI
    // setups.
    match popover_target(app, &window, tray_rect) {
        Some(target) => {
            let _ = window.set_position(tauri::Position::Logical(target));
        }
        None => {
            let _ = window.move_window(Position::TrayBottomCenter);
        }
    }
    let anchor = window.outer_position().ok();

    if was_visible {
        if same_anchor(anchor, anchor_before) {
            let _ = window.hide();
        } else {
            // Clicked the tray on another display: already moved there, keep open.
            let _ = window.set_focus();
        }
        return;
    }

    let hidden = app.state::<PopoverHiddenAt>();
    let closed_by_this_click =
        hidden
            .0
            .lock()
            .ok()
            .and_then(|mut g| g.take())
            .is_some_and(|(at, pos)| {
                at.elapsed() < Duration::from_millis(400) && same_anchor(Some(pos), anchor)
            });
    if closed_by_this_click {
        // The focus loss from this same click already hid it at this tray:
        // the click means "close", so don't reopen.
        return;
    }
    let _ = window.show();
    let _ = window.set_focus();
}

/// Quit the app. Fired by the tray menu's Quit item and the popover's ⌘Q.
#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// Relaunch into an already-downloaded update — the install happened in the
/// background, so restarting just swaps in the new bundle. Fired by the
/// popover's "restart to update" affordance.
#[tauri::command]
fn restart(app: tauri::AppHandle) {
    app.restart();
}

/// The version of an update that has been downloaded + installed and is waiting
/// for a restart to take effect (if any). Held so the popover can show the
/// "restart to update" prompt even if it mounted after the `update-ready`
/// event fired.
#[derive(Default)]
struct PendingUpdate(Mutex<Option<String>>);

#[tauri::command]
fn pending_update(state: tauri::State<'_, PendingUpdate>) -> Option<String> {
    state.0.lock().ok().and_then(|g| g.clone())
}

/// Poll GitHub Releases for a newer signed build — immediately on launch, then
/// every `UPDATE_CHECK_INTERVAL`. A found update is downloaded and installed in
/// the background (best-effort; never interrupts the running session), then the
/// popover is told to offer a restart. Polling stops once one is staged.
fn spawn_update_check(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if check_and_stage_update(&handle).await {
                break;
            }
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
        }
    });
}

async fn check_and_stage_update(app: &tauri::AppHandle) -> bool {
    use tauri::{Emitter, Manager};
    use tauri_plugin_updater::UpdaterExt;

    let Ok(updater) = app.updater() else {
        return false;
    };
    let Ok(Some(update)) = updater.check().await else {
        return false;
    };
    let version = update.version.clone();
    if update.download_and_install(|_, _| {}, || {}).await.is_err() {
        return false;
    }
    if let Ok(mut pending) = app.state::<PendingUpdate>().0.lock() {
        *pending = Some(version.clone());
    }
    let _ = app.emit("update-ready", version);
    true
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(FetchGate(Mutex::new(GateState::default())))
        .manage(PopoverHiddenAt(Mutex::new(None)))
        .manage(PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            usage_snapshot,
            quit,
            restart,
            pending_update
        ])
        .setup(|app| {
            // Menu bar app: no Dock icon, no app switcher entry.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            app.manage(UsageHistory(Mutex::new(load_history(app.handle()))));

            // Right-click menu — the app's only quit path (accessory apps have
            // no Dock icon or menu bar). Left-click still toggles the popover.
            let quit_item = MenuItemBuilder::with_id("quit", "Quit Tokn")
                .accelerator("Cmd+Q")
                .build(app)?;
            let tray_menu = MenuBuilder::new(app).item(&quit_item).build()?;

            // Empty ring until the first successful fetch.
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_ring_image(0.0))
                .icon_as_template(true)
                .tooltip("Tokn")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_popover(tray.app_handle(), &rect);
                    }
                })
                .build(app)?;

            // Best-effort: pull a newer signed build in the background.
            spawn_update_check(app.handle());

            Ok(())
        })
        .on_window_event(|window, event| {
            // Popover behavior: clicking anywhere else dismisses it. Record
            // where it was anchored so a tray click that caused this focus
            // loss can tell "toggle closed" from "reopen on another display".
            if let tauri::WindowEvent::Focused(false) = event {
                if window.is_visible().unwrap_or(false) {
                    let hidden = window.app_handle().state::<PopoverHiddenAt>();
                    if let Ok(mut g) = hidden.0.lock() {
                        *g = window
                            .outer_position()
                            .ok()
                            .map(|pos| (Instant::now(), pos));
                    }
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
