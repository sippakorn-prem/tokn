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
#[cfg(target_os = "macos")]
use tauri_nspanel::ManagerExt;
use tauri_plugin_positioner::{Position, WindowExt};

const TRAY_ID: &str = "tokn-tray";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Treat tokens expiring within this margin as expired.
const EXPIRY_MARGIN_MS: u64 = 60_000;
/// ~6h of samples at the 60s refresh interval.
const HISTORY_CAP: usize = 360;
/// Burn-rate sample files in the app data dir, one per provider.
const HISTORY_FILE: &str = "history.json";
const CODEX_HISTORY_FILE: &str = "codex-history.json";
/// Drop persisted samples older than the sparkline window on load.
const HISTORY_WINDOW_MS: u64 = 6 * 60 * 60 * 1000;
/// Minimum spacing between network fetches; calls inside it return the cache.
const MIN_FETCH_SPACING_MS: u64 = 30_000;
/// Absolute hard floor between real API calls — a backstop that holds even on
/// paths that reset the normal spacing (auth-gate retries, an expired token
/// returning 401). Guarantees the endpoint can't be hit more than once per 10s.
const MIN_API_CALL_SPACING_MS: u64 = 10_000;
/// 429 backoff when the server sends no Retry-After: 2m → 4m → … capped 15m.
const BACKOFF_BASE_SECS: u64 = 120;
const BACKOFF_CAP_SECS: u64 = 900;
/// Never retry a rate-limited endpoint faster than this, even if the server's
/// Retry-After is tiny or zero — otherwise the retry-on-clear loop hammers the
/// API and deepens the rate limiting.
const MIN_RATELIMIT_RETRY_SECS: u64 = 60;
/// How often the running app re-checks GitHub Releases for a newer build.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum AuthStatus {
    Connected,
    MissingToken,
    ExpiredToken,
    KeychainDenied,
    /// Codex provider: no `~/.codex` usage data found (Codex CLI never run, or
    /// no turn has reported a rate-limit window yet).
    CodexNotFound,
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

/// The Codex provider's own burn-rate history, managed separately so the two
/// providers' sparklines don't blend (see [`UsageHistory`]).
struct CodexHistory(UsageHistory);

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
    let Ok(creds) = serde_json::from_slice::<StoredCreds>(&output.stdout) else {
        // Unreadable or reshaped credential blob (e.g. Claude Code changed its
        // format). Treat it as "not logged in" so the popover shows the connect
        // gate rather than a hard error — running Claude Code rewrites it.
        return Ok(TokenRead::Missing);
    };
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
    // The API sends `null` for these when a window is idle (e.g. no active
    // 5-hour session), and has added sibling fields we don't read. Keep both
    // optional so one empty window can't fail the whole response and blank
    // every gauge.
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
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
    let body = resp
        .text()
        .await
        .map_err(|e| UsageFetchError::Other(format!("could not read usage response: {e}")))?;
    serde_json::from_str::<ApiUsage>(&body).map_err(|e| {
        // Surface a short snippet of the actual body (usage %s and reset times —
        // no credentials) so a changed/unexpected response shape is diagnosable
        // instead of an opaque "error decoding response body".
        let snippet: String = body.chars().take(200).collect();
        UsageFetchError::Other(format!("unexpected usage response: {e} · body: {snippet}"))
    })
}

fn parse_resets_at(iso: &str) -> u64 {
    time::OffsetDateTime::parse(iso, &time::format_description::well_known::Rfc3339)
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as u64)
        .unwrap_or(0)
}

fn to_window(label: &str, api: Option<ApiWindow>) -> UsageWindow {
    let (pct, resets) = api
        .map(|w| {
            let resets = w.resets_at.as_deref().map(parse_resets_at).unwrap_or(0);
            (w.utilization.unwrap_or(0.0), resets)
        })
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
fn history_path(app: &tauri::AppHandle, file: &str) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(file))
}

fn load_history(app: &tauri::AppHandle, file: &str) -> VecDeque<(u64, f64)> {
    let Some(path) = history_path(app, file) else {
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

fn save_history(app: &tauri::AppHandle, history: &UsageHistory, file: &str) {
    let Some(path) = history_path(app, file) else {
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
    /// When the last real fetch was *started*. Backs the absolute
    /// `MIN_API_CALL_SPACING_MS` floor, which caps the API call rate even on
    /// paths that bypass `next_fetch_at_ms` (e.g. the auth-gate's `= 0`).
    last_fetch_at_ms: u64,
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

/// How long to wait before the next fetch after a 429. Honors the server's
/// `Retry-After` (or grows our own exponential backoff when it's absent), but
/// never returns less than `MIN_RATELIMIT_RETRY_SECS`: a zero/tiny Retry-After
/// would otherwise send the frontend's retry-on-clear straight into another
/// 429, hammering the endpoint.
fn ratelimit_wait_secs(retry_after: Option<u64>, backoff_secs: u64) -> u64 {
    let requested = retry_after.unwrap_or(if backoff_secs == 0 {
        BACKOFF_BASE_SECS
    } else {
        (backoff_secs * 2).min(BACKOFF_CAP_SECS)
    });
    requested.max(MIN_RATELIMIT_RETRY_SECS)
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
        // Absolute floor: never start a real fetch within MIN_API_CALL_SPACING_MS
        // of the last one, whatever the higher-level spacing says. Backstop
        // against spamming the API (rapid gate retries, an expired token, or a
        // future logic slip).
        let too_soon = now < g.last_fetch_at_ms.saturating_add(MIN_API_CALL_SPACING_MS);
        if within_spacing || too_soon || g.in_flight {
            return Ok(g.cached_response());
        }
        g.in_flight = true;
        g.last_fetch_at_ms = now;
    }

    let result = build_snapshot(&history).await;
    let mut g = gate.0.lock().expect("gate lock poisoned");
    g.in_flight = false;
    match result {
        Ok(snapshot) => {
            g.rate_limited = false;
            g.backoff_secs = 0;
            // Auth-gated states skip the normal spacing so the gate's retry
            // reacts fast once login is fixed — the MIN_API_CALL_SPACING_MS
            // floor still caps it so rapid retries can't spam the API.
            g.next_fetch_at_ms = if matches!(snapshot.auth_status, AuthStatus::Connected) {
                now + MIN_FETCH_SPACING_MS
            } else {
                0
            };
            g.last_snapshot = Some(snapshot.clone());
            drop(g);
            sync_tray(&app, &snapshot);
            if matches!(snapshot.auth_status, AuthStatus::Connected) {
                save_history(&app, &history, HISTORY_FILE);
            }
            Ok(snapshot)
        }
        Err(SnapshotError::RateLimited(retry_after)) => {
            let wait_secs = ratelimit_wait_secs(retry_after, g.backoff_secs);
            g.backoff_secs = wait_secs;
            g.rate_limited = true;
            g.next_fetch_at_ms = now + wait_secs * 1000;
            Ok(g.cached_response())
        }
        Err(SnapshotError::Other(error)) => Err(error),
    }
}

/* ── codex provider ───────────────────────────────────────────── */

/// Reads Codex CLI usage from its local rollout logs. Unlike Claude, Codex
/// needs no API call or credential: every turn appends a `token_count` event
/// to `~/.codex/sessions/<Y>/<M>/<D>/rollout-*.jsonl`, carrying a `rate_limits`
/// block with `used_percent` / `window_minutes` / `resets_at` per window — the
/// same shape Tokn already renders. We read the newest turn's windows.
mod codex {
    use super::UsageWindow;
    use serde::Deserialize;
    use std::path::{Path, PathBuf};

    #[derive(Deserialize)]
    struct Line {
        #[serde(rename = "type")]
        kind: String,
        payload: Option<Payload>,
    }

    #[derive(Deserialize)]
    struct Payload {
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        rate_limits: Option<RateLimits>,
    }

    #[derive(Deserialize)]
    struct RateLimits {
        #[serde(default)]
        primary: Option<Window>,
        #[serde(default)]
        secondary: Option<Window>,
    }

    // Optional throughout for the same reason as Claude's `ApiWindow`: idle
    // windows and future sibling fields must not fail the whole parse.
    #[derive(Deserialize, Clone)]
    struct Window {
        #[serde(default)]
        used_percent: Option<f64>,
        #[serde(default)]
        window_minutes: Option<u64>,
        #[serde(default)]
        resets_at: Option<i64>,
    }

    /// The newest turn's windows mapped onto Tokn's session/weekly gauges.
    pub(crate) struct Windows {
        pub session: UsageWindow,
        pub weekly: UsageWindow,
    }

    fn sessions_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex").join("sessions"))
    }

    fn dir_entries(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect()
    }

    /// All `rollout-*.jsonl` paths under `sessions/<Y>/<M>/<D>`, newest mtime first.
    fn rollouts_newest_first() -> Vec<PathBuf> {
        let Some(root) = sessions_dir() else {
            return Vec::new();
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for year in dir_entries(&root) {
            for month in dir_entries(&year) {
                for day in dir_entries(&month) {
                    for path in dir_entries(&day) {
                        let is_rollout = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
                        if !is_rollout {
                            continue;
                        }
                        let mtime = path
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::UNIX_EPOCH);
                        files.push((mtime, path));
                    }
                }
            }
        }
        files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        files.into_iter().map(|(_, p)| p).collect()
    }

    /// The last `token_count` event's rate limits in a rollout file, if any.
    /// Active rollouts reach several MB and are mostly `response_item` lines, so
    /// scan from the end and only JSON-parse lines that could be the one we want
    /// (cheap substring test first) — the newest `token_count` wins.
    fn last_rate_limits(path: &Path) -> Option<RateLimits> {
        let text = std::fs::read_to_string(path).ok()?;
        for line in text.lines().rev() {
            if !line.contains("\"token_count\"") {
                continue;
            }
            let Ok(l) = serde_json::from_str::<Line>(line) else {
                continue;
            };
            if l.kind != "event_msg" {
                continue;
            }
            let Some(payload) = l.payload else { continue };
            if payload.kind != "token_count" {
                continue;
            }
            if let Some(rl) = payload.rate_limits {
                return Some(rl);
            }
        }
        None
    }

    fn to_window(label: &str, w: Option<&Window>) -> UsageWindow {
        let (pct, resets) = w
            .map(|w| {
                // Codex sends `resets_at` as unix *seconds* (Claude uses an
                // RFC3339 string), so convert directly rather than via parse.
                let resets = w
                    .resets_at
                    .filter(|&s| s > 0)
                    .map(|s| s as u64 * 1000)
                    .unwrap_or(0);
                (w.used_percent.unwrap_or(0.0), resets)
            })
            .unwrap_or((0.0, 0));
        UsageWindow {
            label: label.into(),
            used_pct: pct,
            resets_at_ms: resets,
        }
    }

    /// Shortest window → session gauge, longest → weekly. Codex reports a 5h
    /// window (`window_minutes` 300) and/or a weekly one (10080); with a single
    /// window present, place it on the matching gauge and idle the other.
    fn map_windows(rl: &RateLimits) -> Windows {
        let mut wins: Vec<Window> = [rl.primary.clone(), rl.secondary.clone()]
            .into_iter()
            .flatten()
            .collect();
        wins.sort_by_key(|w| w.window_minutes.unwrap_or(u64::MAX));
        let (session, weekly) = match wins.as_slice() {
            [] => (None, None),
            [only] => {
                if only.window_minutes.unwrap_or(0) <= 600 {
                    (Some(only), None)
                } else {
                    (None, Some(only))
                }
            }
            [short, long, ..] => (Some(short), Some(long)),
        };
        Windows {
            session: to_window("current session", session),
            weekly: to_window("weekly limit", weekly),
        }
    }

    /// Freshest Codex usage, or `None` when Codex has never run / no turn has
    /// reported rate-limit windows yet. Scans a few recent rollouts so a
    /// brand-new session without a `token_count` line yet doesn't blank it.
    pub(crate) fn read_windows() -> Option<Windows> {
        let rl = rollouts_newest_first()
            .iter()
            .take(8)
            .find_map(|p| last_rate_limits(p))?;
        Some(map_windows(&rl))
    }

    #[cfg(test)]
    pub(crate) fn map_windows_for_test(json_line: &str) -> Windows {
        let line: Line = serde_json::from_str(json_line).expect("valid token_count line");
        map_windows(&line.payload.unwrap().rate_limits.unwrap())
    }
}

/// Codex counterpart to `usage_snapshot`: builds the snapshot from local
/// rollout logs (no network, no fetch gate) and syncs the tray to it, so the
/// tray follows whichever provider the popover is currently polling.
#[tauri::command]
async fn codex_snapshot(
    app: tauri::AppHandle,
    history: tauri::State<'_, CodexHistory>,
) -> Result<UsageSnapshot, String> {
    let now = now_ms();
    // The newest rollout can be several MB; read/parse it off the UI thread.
    let windows = tauri::async_runtime::spawn_blocking(codex::read_windows)
        .await
        .map_err(|e| format!("codex read task failed: {e}"))?;
    let snapshot = match windows {
        Some(w) => {
            let burn_rate = record_burn_sample(&history.0, w.session.used_pct, now);
            UsageSnapshot {
                auth_status: AuthStatus::Connected,
                session: w.session,
                weekly: w.weekly,
                burn_rate,
                fetched_at_ms: now,
                rate_limited_until_ms: None,
                placeholder: false,
            }
        }
        None => disconnected_snapshot(AuthStatus::CodexNotFound),
    };
    sync_tray(&app, &snapshot);
    if matches!(snapshot.auth_status, AuthStatus::Connected) {
        save_history(&app, &history.0, CODEX_HISTORY_FILE);
    }
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

/// Non-activating `NSPanel` support for the popover. A plain `NSWindow` can't
/// open over another app's full-screen Space: `show`/`set_focus` activates
/// Tokn, and macOS resolves that by switching you to the desktop Space (what
/// you saw). A non-activating panel shows and takes clicks without activating
/// the app, so it overlays full-screen apps — the standard macOS menu-bar
/// trick. `CanJoinAllSpaces | FullScreenAuxiliary` lets it join whichever Space
/// (regular or full-screen) is frontmost.
///
/// Lives in its own module for the inner `#![allow(clippy::unused_unit)]`: the
/// `panel_event!` grammar requires an explicit `-> ()` return, which the macro
/// emits as a unit-returning fn — and an `#[allow]` on the invocation alone
/// doesn't reach into the expansion.
#[cfg(target_os = "macos")]
mod popover_panel {
    #![allow(clippy::unused_unit)]

    use tauri::Manager; // the generated from_window() calls window.app_handle()
    use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelLevel, StyleMask, WebviewWindowExt};

    tauri_panel! {
        panel!(PopoverPanel {
            config: {
                // Non-activating, but still allowed to become key so it
                // receives clicks and fires resign-key when focus moves away.
                can_become_key_window: true,
                is_floating_panel: true
            }
        })

        panel_event!(PopoverPanelDelegate {
            window_did_resign_key(notification: &NSNotification) -> ()
        })
    }

    /// Reclass the popover window and wire its click-away dismissal.
    pub(crate) fn install(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
        let Ok(panel) = window.to_panel::<PopoverPanel>() else {
            return;
        };
        panel.set_level(PanelLevel::Floating.value());
        panel.set_style_mask(StyleMask::empty().nonactivating_panel().into());
        panel.set_collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary()
                .into(),
        );
        // Keep the transparent, rounded look — the panel would otherwise paint
        // an opaque system background behind the CSS popover.
        panel.set_transparent(true);

        // Click-away dismissal: a non-activating panel resigns key instead of
        // firing Tauri's `Focused(false)`, so drive the hide from the delegate.
        // `set_event_handler` retains the delegate, so the local can drop.
        let delegate = PopoverPanelDelegate::new();
        let handle = app.clone();
        delegate.window_did_resign_key(move |_| super::dismiss_popover(&handle));
        panel.set_event_handler(Some(delegate.as_ref()));
    }
}

/// Hide the popover and record where/when — a tray click that stole key (and so
/// triggered the resign-key that called this) can then tell "close" from
/// "reopen on another display".
#[cfg(target_os = "macos")]
fn dismiss_popover(app: &tauri::AppHandle) {
    let Ok(panel) = app.get_webview_panel("main") else {
        return;
    };
    if !panel.is_visible() {
        return;
    }
    let stamped = app
        .get_webview_window("main")
        .and_then(|w| w.outer_position().ok())
        .map(|pos| (Instant::now(), pos));
    if let Ok(mut g) = app.state::<PopoverHiddenAt>().0.lock() {
        *g = stamped;
    }
    panel.hide();
}

/// Show the popover. On macOS it becomes key without activating the app (it's a
/// non-activating panel), which is what lets it float over full-screen apps.
#[cfg(target_os = "macos")]
fn popover_show(app: &tauri::AppHandle) {
    if let Ok(panel) = app.get_webview_panel("main") {
        panel.show_and_make_key();
    }
}

#[cfg(not(target_os = "macos"))]
fn popover_show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(target_os = "macos")]
fn popover_hide(app: &tauri::AppHandle) {
    if let Ok(panel) = app.get_webview_panel("main") {
        panel.hide();
    }
}

#[cfg(not(target_os = "macos"))]
fn popover_hide(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[cfg(target_os = "macos")]
fn popover_is_visible(app: &tauri::AppHandle) -> bool {
    app.get_webview_panel("main")
        .map(|p| p.is_visible())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn popover_is_visible(app: &tauri::AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

fn toggle_popover(app: &tauri::AppHandle, tray_rect: &tauri::Rect) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let was_visible = popover_is_visible(app);
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
            popover_hide(app);
        } else {
            // Clicked the tray on another display: already moved there, keep open.
            popover_show(app);
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
    popover_show(app);
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
            // Background poll ignores failures and just retries next interval.
            if stage_update(&handle).await.unwrap_or(false) {
                break;
            }
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
        }
    });
}

/// Check GitHub Releases once and, if a newer signed build exists, download and
/// install it in the background and stage it for restart. Returns whether an
/// update was staged (`false` = already on the latest version). `Err` carries a
/// real failure so a manual check can distinguish "up to date" from "failed".
async fn stage_update(app: &tauri::AppHandle) -> Result<bool, String> {
    use tauri::{Emitter, Manager};
    use tauri_plugin_updater::UpdaterExt;

    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let version = update.version.clone();
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(mut pending) = app.state::<PendingUpdate>().0.lock() {
        *pending = Some(version.clone());
    }
    let _ = app.emit("update-ready", version);
    Ok(true)
}

/// Manual "check for updates" from the popover. Same path as the background
/// poll, but surfaces the outcome: `Ok(true)` staged an update (the popover
/// shows the restart prompt via `update-ready`), `Ok(false)` is up to date,
/// `Err` is a failed check.
#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<bool, String> {
    stage_update(&app).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_updater::Builder::new().build());
    // Non-activating panel support (popover over full-screen apps).
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .manage(FetchGate(Mutex::new(GateState::default())))
        .manage(PopoverHiddenAt(Mutex::new(None)))
        .manage(PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            usage_snapshot,
            codex_snapshot,
            quit,
            restart,
            pending_update,
            check_for_update
        ])
        .setup(|app| {
            // Menu bar app: no Dock icon, no app switcher entry.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Reclass the popover to a non-activating panel so it can open over
            // full-screen apps' Spaces without activating Tokn.
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                popover_panel::install(app.handle(), &window);
            }

            app.manage(UsageHistory(Mutex::new(load_history(
                app.handle(),
                HISTORY_FILE,
            ))));
            app.manage(CodexHistory(UsageHistory(Mutex::new(load_history(
                app.handle(),
                CODEX_HISTORY_FILE,
            )))));

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
            // On macOS the non-activating panel drives this from its delegate
            // (`dismiss_popover`) instead — Tauri's own focus events don't fire
            // for it — so this path is only for other platforms.
            #[cfg(not(target_os = "macos"))]
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
            #[cfg(target_os = "macos")]
            let _ = (window, event);
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the usage-response contract: the API sends `null` for idle windows
    /// and keeps adding sibling fields (`*_in_dollars`, …), so parsing must
    /// tolerate both without failing the whole response. Regression guard for
    /// the "error decoding response body" bug that blanked every gauge.
    #[test]
    fn usage_tolerates_null_fields_and_unknown_keys() {
        let body = r#"{
            "five_hour":  { "utilization": null, "resets_at": null, "cost_in_dollars": null },
            "seven_day":  { "utilization": 74.0, "resets_at": "2026-07-13T09:00:00.473221+00:00", "limit_in_dollars": null },
            "brand_new_top_level_field": 123
        }"#;

        let usage: ApiUsage =
            serde_json::from_str(body).expect("null fields and unknown keys must not fail parsing");

        let session = to_window("current session", usage.five_hour);
        assert!(session.used_pct.abs() < 1e-9);
        assert_eq!(session.resets_at_ms, 0); // null resets_at -> 0 (idle) sentinel

        let weekly = to_window("weekly limit", usage.seven_day);
        assert!((weekly.used_pct - 74.0).abs() < 1e-9);
        assert!(weekly.resets_at_ms > 0); // fractional-second RFC3339 must parse
    }

    /// Missing windows (and a bare `{}`) must degrade to zeroed, non-panicking
    /// gauges rather than erroring.
    #[test]
    fn usage_tolerates_missing_windows() {
        let usage: ApiUsage = serde_json::from_str("{}").expect("empty object is valid");
        assert!(usage.five_hour.is_none());
        assert!(usage.seven_day.is_none());
        let session = to_window("current session", usage.five_hour);
        assert!(session.used_pct.abs() < 1e-9);
        assert_eq!(session.resets_at_ms, 0);
    }

    /// The exact reset-time shape observed from the live API.
    #[test]
    fn resets_at_parses_fractional_rfc3339() {
        assert!(parse_resets_at("2026-07-13T09:00:00.473221+00:00") > 0);
    }

    /// A 429 must never schedule a retry faster than the floor — the guard
    /// against the spam loop a zero/tiny Retry-After would otherwise cause.
    #[test]
    fn ratelimit_retry_never_below_floor() {
        assert_eq!(ratelimit_wait_secs(Some(0), 0), MIN_RATELIMIT_RETRY_SECS);
        assert_eq!(ratelimit_wait_secs(Some(1), 0), MIN_RATELIMIT_RETRY_SECS);
        assert_eq!(ratelimit_wait_secs(Some(300), 0), 300); // honors a larger server value
        assert!(ratelimit_wait_secs(None, 0) >= MIN_RATELIMIT_RETRY_SECS);
        assert!(ratelimit_wait_secs(None, 480) <= BACKOFF_CAP_SECS); // doubling stays capped
    }

    /// A real Codex `token_count` event: the 5h window (`window_minutes` 300)
    /// maps to the session gauge, the weekly (10080) to the weekly gauge, and
    /// the unix-seconds `resets_at` becomes epoch ms.
    #[test]
    fn codex_maps_primary_and_secondary_windows() {
        let line = r#"{
            "timestamp": "2026-07-20T08:06:39.990Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": { "model_context_window": 258400 },
                "rate_limits": {
                    "primary":   { "used_percent": 42.0, "window_minutes": 300,   "resets_at": 1785000000 },
                    "secondary": { "used_percent": 7.5,  "window_minutes": 10080, "resets_at": 1785062347 },
                    "brand_new_field": true
                }
            }
        }"#;

        let w = codex::map_windows_for_test(line);
        assert!((w.session.used_pct - 42.0).abs() < 1e-9);
        assert_eq!(w.session.resets_at_ms, 1785000000_000); // seconds -> ms
        assert!((w.weekly.used_pct - 7.5).abs() < 1e-9);
        assert_eq!(w.weekly.resets_at_ms, 1785062347_000);
    }

    /// Codex often reports only the weekly window (primary 10080, no secondary):
    /// it must land on the weekly gauge and leave the session gauge idle.
    #[test]
    fn codex_single_weekly_window_idles_session() {
        let line = r#"{
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "rate_limits": {
                    "primary":   { "used_percent": 5.0, "window_minutes": 10080, "resets_at": 1785062347 },
                    "secondary": null
                }
            }
        }"#;

        let w = codex::map_windows_for_test(line);
        assert!(w.session.used_pct.abs() < 1e-9);
        assert_eq!(w.session.resets_at_ms, 0);
        assert!((w.weekly.used_pct - 5.0).abs() < 1e-9);
        assert_eq!(w.weekly.resets_at_ms, 1785062347_000);
    }
}
