use serde::Serialize;
use tauri::{
    image::Image,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tauri_plugin_positioner::{Position, WindowExt};

const TRAY_ID: &str = "tokn-tray";

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
    session: UsageWindow,
    weekly: UsageWindow,
    burn_rate: Vec<f64>,
    fetched_at_ms: u64,
    source: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as u64
}

/// Mock burn-rate velocity curve (~6h, 44 samples), ported from the design
/// prototype's genBurn().
fn mock_burn_rate(seed: u64) -> Vec<f64> {
    let mut v = 0.5 + (seed % 5) as f64 * 0.06;
    (0..44u64)
        .map(|i| {
            v += (i as f64 * 0.5 + seed as f64).sin() * 0.04
                + 0.012
                + (((i * 7 + seed * 13) % 9) as f64 - 4.0) / 220.0;
            v = v.clamp(0.22, 1.0);
            v
        })
        .collect()
}

fn mock_snapshot() -> UsageSnapshot {
    let now = now_ms();
    UsageSnapshot {
        session: UsageWindow {
            label: "current session".into(),
            used_pct: 12.0,
            resets_at_ms: now + (3 * 60 + 55) * 60 * 1000,
        },
        weekly: UsageWindow {
            label: "weekly limit".into(),
            used_pct: 38.0,
            resets_at_ms: now + (4 * 24 + 1) * 60 * 60 * 1000,
        },
        burn_rate: mock_burn_rate((now / 1000) % 97),
        fetched_at_ms: now,
        source: "mock".into(),
    }
}

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

fn sync_tray(app: &tauri::AppHandle, snapshot: &UsageSnapshot) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let hi = snapshot.session.used_pct.max(snapshot.weekly.used_pct);
        let _ = tray.set_icon(Some(tray_ring_image(hi)));
        let _ = tray.set_icon_as_template(true);
    }
}

/// Mock snapshot. Will be replaced by a real fetch against the usage endpoint
/// once the request shape is captured (see project notes, step 4).
#[tauri::command]
fn usage_snapshot(app: tauri::AppHandle) -> UsageSnapshot {
    let snapshot = mock_snapshot();
    sync_tray(&app, &snapshot);
    snapshot
}

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
        .invoke_handler(tauri::generate_handler![usage_snapshot])
        .setup(|app| {
            // Menu bar app: no Dock icon, no app switcher entry.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let initial = mock_snapshot();
            let hi = initial.session.used_pct.max(initial.weekly.used_pct);
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_ring_image(hi))
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
