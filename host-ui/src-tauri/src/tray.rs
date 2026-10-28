//! Menu-bar tray icon and its menu-panel toggle behavior.

use tauri::{
    image::Image, tray::MouseButton, tray::MouseButtonState, tray::TrayIconEvent, Listener, Manager,
};
use tauri_plugin_positioner::{on_tray_event, Position, WindowExt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayState {
    Idle,
    Controlled,
    Update,
    Offline,
}

fn parse_tray_state(state: &str) -> Option<TrayState> {
    match state {
        "idle" => Some(TrayState::Idle),
        "controlled" => Some(TrayState::Controlled),
        "update" => Some(TrayState::Update),
        "offline" => Some(TrayState::Offline),
        _ => None,
    }
}

fn tray_tooltip(state: TrayState) -> &'static str {
    match state {
        TrayState::Idle => "Remote Host",
        TrayState::Controlled => "Remote Host · 正在被控制",
        TrayState::Update => "Remote Host · 有更新",
        TrayState::Offline => "Remote Host · 离线",
    }
}

fn template_icon(state: TrayState) -> bool {
    matches!(state, TrayState::Idle | TrayState::Offline)
}

const BLACK: [u8; 4] = [0, 0, 0, 255];
const TRANSPARENT: [u8; 4] = [0, 0, 0, 0];
const CONTROLLED: [u8; 4] = [0xff, 0x9f, 0x43, 0xff];
const UPDATE_DOT: [u8; 4] = [0xff, 0x88, 0x73, 0xff];

fn rounded_contains(
    x: f64,
    y: f64,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    radius: f64,
) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let corner_x = if x < left + radius {
        left + radius
    } else if x > right - radius {
        right - radius
    } else {
        x
    };
    let corner_y = if y < top + radius {
        top + radius
    } else if y > bottom - radius {
        bottom - radius
    } else {
        y
    };
    let dx = x - corner_x;
    let dy = y - corner_y;
    dx * dx + dy * dy <= radius * radius
}

fn set_pixel(rgba: &mut [u8], x: u32, y: u32, color: [u8; 4]) {
    if x >= 36 || y >= 36 {
        return;
    }
    let offset = ((y * 36 + x) * 4) as usize;
    rgba[offset..offset + 4].copy_from_slice(&color);
}

fn draw_rounded_rect(
    rgba: &mut [u8],
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    radius: f64,
    color: [u8; 4],
) {
    for y in 0..36 {
        for x in 0..36 {
            if rounded_contains(
                f64::from(x) + 0.5,
                f64::from(y) + 0.5,
                left,
                top,
                right,
                bottom,
                radius,
            ) {
                set_pixel(rgba, x, y, color);
            }
        }
    }
}

fn draw_segment(
    rgba: &mut [u8],
    start: (f64, f64),
    end: (f64, f64),
    thickness: f64,
    color: [u8; 4],
) {
    let (sx, sy) = start;
    let (ex, ey) = end;
    let dx = ex - sx;
    let dy = ey - sy;
    let length_squared = dx * dx + dy * dy;
    for y in 0..36 {
        for x in 0..36 {
            let px = f64::from(x) + 0.5;
            let py = f64::from(y) + 0.5;
            let t = if length_squared == 0.0 {
                0.0
            } else {
                ((px - sx) * dx + (py - sy) * dy) / length_squared
            };
            let t = t.clamp(0.0, 1.0);
            let nearest_x = sx + t * dx;
            let nearest_y = sy + t * dy;
            let distance_squared = (px - nearest_x).powi(2) + (py - nearest_y).powi(2);
            if distance_squared <= thickness * thickness {
                set_pixel(rgba, x, y, color);
            }
        }
    }
}

fn draw_circle(rgba: &mut [u8], center: (f64, f64), radius: f64, color: [u8; 4]) {
    for y in 0..36 {
        for x in 0..36 {
            let dx = f64::from(x) + 0.5 - center.0;
            let dy = f64::from(y) + 0.5 - center.1;
            if dx * dx + dy * dy <= radius * radius {
                set_pixel(rgba, x, y, color);
            }
        }
    }
}

/// Draw the 18pt@2x tray icon without a binary resource.
fn tray_icon_rgba(state: TrayState) -> (Vec<u8>, u32, u32) {
    let mut rgba = vec![0; 36 * 36 * 4];
    draw_rounded_rect(&mut rgba, 4.0, 4.0, 32.0, 27.0, 4.0, BLACK);
    match state {
        TrayState::Controlled => {
            draw_rounded_rect(&mut rgba, 4.0, 4.0, 32.0, 27.0, 4.0, CONTROLLED);
            draw_rounded_rect(&mut rgba, 13.0, 27.0, 23.0, 31.0, 1.5, CONTROLLED);
        }
        _ => {
            draw_rounded_rect(&mut rgba, 7.0, 7.0, 29.0, 24.0, 1.5, TRANSPARENT);
            draw_rounded_rect(&mut rgba, 13.0, 27.0, 23.0, 31.0, 1.5, BLACK);
        }
    }
    if state == TrayState::Offline {
        draw_segment(&mut rgba, (7.0, 6.0), (30.0, 29.0), 1.5, BLACK);
    }
    if state == TrayState::Update {
        draw_circle(&mut rgba, (28.0, 8.0), 5.0, UPDATE_DOT);
    }
    (rgba, 36, 36)
}

fn set_tray_icon(app: &tauri::AppHandle, state: TrayState) -> Result<(), String> {
    let Some(tray) = app.tray_by_id("main") else {
        return Err("tray not initialized".to_string());
    };
    let (rgba, width, height) = tray_icon_rgba(state);
    tray.set_icon_with_as_template(
        Some(Image::new_owned(rgba, width, height)),
        template_icon(state),
    )
    .map_err(|error| error.to_string())?;
    tray.set_tooltip(Some(tray_tooltip(state)))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn toggle_menu(app: &tauri::AppHandle) {
    let Some(menu) = app.get_webview_window("menu") else {
        return;
    };
    match menu.is_visible() {
        Ok(true) => {
            let _ = menu.hide();
        }
        Ok(false) => {
            let _ = menu.move_window(Position::TrayBottomCenter);
            let _ = menu.show();
            let _ = menu.set_focus();
        }
        Err(_) => {}
    }
}

fn handle_tray_event(app: &tauri::AppHandle, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left | MouseButton::Right,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        toggle_menu(app);
    }
}

fn handle_connection_event(app: &tauri::AppHandle, payload: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    let Some(connected) = value.get("connected").and_then(serde_json::Value::as_bool) else {
        return;
    };
    let state = if connected {
        TrayState::Idle
    } else {
        TrayState::Offline
    };
    let _ = set_tray_icon(app, state);
}

fn handle_agent_event(app: &tauri::AppHandle, payload: &str) {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    let Some(method) = event.get("method").and_then(serde_json::Value::as_str) else {
        return;
    };
    let params = event.get("params");
    let state = match method {
        "event.sessionsChanged" => {
            let has_sessions = params
                .and_then(|value| value.get("sessions"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|sessions| !sessions.is_empty());
            Some(if has_sessions {
                TrayState::Controlled
            } else {
                TrayState::Idle
            })
        }
        "event.updateState" => {
            let update_state = params
                .and_then(|value| value.get("state"))
                .and_then(serde_json::Value::as_str);
            Some(match update_state {
                Some("idle" | "none" | "up_to_date" | "installed") | None => TrayState::Idle,
                Some(_) => TrayState::Update,
            })
        }
        _ => None,
    };
    if let Some(state) = state {
        let _ = set_tray_icon(app, state);
    }
}

// H17T supplies tray behavior; keep these registered command signatures stable.

pub fn setup(app: &mut tauri::App) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let (rgba, width, height) = tray_icon_rgba(TrayState::Idle);
    let event_app = app.handle().clone();
    tauri::tray::TrayIconBuilder::with_id("main")
        .icon(Image::new_owned(rgba, width, height))
        .icon_as_template(true)
        .tooltip(tray_tooltip(TrayState::Idle))
        .show_menu_on_left_click(false)
        .on_tray_icon_event(move |_tray, event| {
            on_tray_event(&event_app, &event);
            handle_tray_event(&event_app, event);
        })
        .build(app)?;

    let connection_app = app.handle().clone();
    app.listen("ipc://connection", move |event| {
        handle_connection_event(&connection_app, event.payload());
    });
    let event_app = app.handle().clone();
    app.listen("ipc://event", move |event| {
        handle_agent_event(&event_app, event.payload());
    });
    Ok(())
}

#[tauri::command]
pub fn tray_set_state(app: tauri::AppHandle, state: String) -> Result<(), String> {
    let state = parse_tray_state(&state).ok_or_else(|| "invalid tray state".to_string())?;
    set_tray_icon(&app, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn icon_is_36_by_36_rgba_for_every_state() {
        for state in [
            TrayState::Idle,
            TrayState::Controlled,
            TrayState::Update,
            TrayState::Offline,
        ] {
            let (rgba, width, height) = tray_icon_rgba(state);
            assert_eq!((width, height), (36, 36));
            assert_eq!(rgba.len(), 36 * 36 * 4);
        }
    }

    #[test]
    fn icon_states_have_the_required_colors() {
        let (controlled, _, _) = tray_icon_rgba(TrayState::Controlled);
        assert!(controlled.chunks_exact(4).any(|pixel| pixel == CONTROLLED));
        let (update, _, _) = tray_icon_rgba(TrayState::Update);
        assert!(update
            .chunks_exact(4)
            .enumerate()
            .filter(|(index, _)| {
                let x = index % 36;
                let y = index / 36;
                x >= 23 && y <= 13
            })
            .any(|(_, pixel)| pixel == UPDATE_DOT));
        let (idle, _, _) = tray_icon_rgba(TrayState::Idle);
        let (offline, _, _) = tray_icon_rgba(TrayState::Offline);
        assert_ne!(idle, offline);
        let colors: BTreeSet<[u8; 4]> = idle
            .chunks_exact(4)
            .map(|pixel| pixel.try_into().expect("four bytes"))
            .collect();
        assert!(colors
            .iter()
            .all(|color| *color == BLACK || *color == TRANSPARENT));
    }

    #[test]
    fn parser_rejects_unknown_state() {
        assert_eq!(parse_tray_state("idle"), Some(TrayState::Idle));
        assert_eq!(parse_tray_state("bogus"), None);
    }
}
