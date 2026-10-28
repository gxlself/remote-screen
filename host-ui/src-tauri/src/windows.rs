//! Window creation and lifecycle for the host menu-bar application.
//!
//! The app starts with a small set of hidden windows.  The other pages are
//! created lazily so closing a page also drops its webview state.

use std::path::PathBuf;

use tauri::{
    Listener, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const PERSISTENT_LABELS: [&str; 5] = ["bg", "menu", "capsule", "border", "confirm"];

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowSpec {
    label: &'static str,
    persistent: bool,
    width: f64,
    height: f64,
    min_width: Option<f64>,
    min_height: Option<f64>,
    title: Option<&'static str>,
    centered: bool,
    visible: bool,
    transparent: bool,
    decorations: bool,
    always_on_top: bool,
    visible_on_all_workspaces: bool,
    skip_taskbar: bool,
    resizable: bool,
    shadow: bool,
    focusable: bool,
    ignore_cursor_events: bool,
}

/// The declarative window table.  Keeping this as data makes the dimensions
/// and lifecycle rules testable without constructing a Tauri runtime.
fn window_spec(label: &str) -> Option<WindowSpec> {
    let spec = match label {
        "bg" => WindowSpec {
            label: "bg",
            persistent: true,
            width: 1.0,
            height: 1.0,
            min_width: None,
            min_height: None,
            title: None,
            centered: false,
            visible: false,
            transparent: true,
            decorations: false,
            always_on_top: false,
            visible_on_all_workspaces: false,
            skip_taskbar: true,
            resizable: false,
            shadow: false,
            focusable: false,
            ignore_cursor_events: false,
        },
        "menu" => WindowSpec {
            label: "menu",
            persistent: true,
            width: 340.0,
            height: 560.0,
            min_width: None,
            min_height: None,
            title: None,
            centered: false,
            visible: false,
            transparent: true,
            decorations: false,
            always_on_top: true,
            visible_on_all_workspaces: false,
            skip_taskbar: true,
            resizable: false,
            shadow: true,
            focusable: true,
            ignore_cursor_events: false,
        },
        "capsule" => WindowSpec {
            label: "capsule",
            persistent: true,
            width: 560.0,
            height: 48.0,
            min_width: None,
            min_height: None,
            title: None,
            centered: false,
            visible: false,
            transparent: true,
            decorations: false,
            always_on_top: true,
            visible_on_all_workspaces: true,
            skip_taskbar: true,
            resizable: false,
            shadow: false,
            focusable: false,
            ignore_cursor_events: false,
        },
        "border" => WindowSpec {
            label: "border",
            persistent: true,
            width: 1.0,
            height: 1.0,
            min_width: None,
            min_height: None,
            title: None,
            centered: false,
            visible: false,
            transparent: true,
            decorations: false,
            always_on_top: true,
            visible_on_all_workspaces: true,
            skip_taskbar: true,
            resizable: false,
            shadow: false,
            focusable: false,
            ignore_cursor_events: true,
        },
        "confirm" => WindowSpec {
            label: "confirm",
            persistent: true,
            width: 440.0,
            height: 480.0,
            min_width: None,
            min_height: None,
            title: None,
            centered: false,
            visible: false,
            transparent: true,
            decorations: false,
            always_on_top: true,
            visible_on_all_workspaces: false,
            skip_taskbar: true,
            resizable: false,
            shadow: true,
            focusable: true,
            ignore_cursor_events: false,
        },
        "pair" => WindowSpec {
            label: "pair",
            persistent: false,
            width: 660.0,
            height: 470.0,
            min_width: None,
            min_height: None,
            title: Some("配对新设备"),
            centered: true,
            visible: false,
            transparent: true,
            decorations: true,
            always_on_top: false,
            visible_on_all_workspaces: false,
            skip_taskbar: false,
            resizable: false,
            shadow: true,
            focusable: true,
            ignore_cursor_events: false,
        },
        "settings" => WindowSpec {
            label: "settings",
            persistent: false,
            width: 1100.0,
            height: 640.0,
            min_width: Some(900.0),
            min_height: Some(560.0),
            title: Some("Remote Host 设置"),
            centered: true,
            visible: false,
            transparent: true,
            decorations: true,
            always_on_top: false,
            visible_on_all_workspaces: false,
            skip_taskbar: false,
            resizable: true,
            shadow: true,
            focusable: true,
            ignore_cursor_events: false,
        },
        "setup" => WindowSpec {
            label: "setup",
            persistent: false,
            width: 800.0,
            height: 540.0,
            min_width: None,
            min_height: None,
            title: Some("首次设置"),
            centered: true,
            visible: false,
            transparent: true,
            decorations: true,
            always_on_top: false,
            visible_on_all_workspaces: false,
            skip_taskbar: false,
            resizable: false,
            shadow: true,
            focusable: true,
            ignore_cursor_events: false,
        },
        "update" => WindowSpec {
            label: "update",
            persistent: false,
            width: 600.0,
            height: 560.0,
            min_width: None,
            min_height: None,
            title: Some("软件更新"),
            centered: true,
            visible: false,
            transparent: true,
            decorations: true,
            always_on_top: false,
            visible_on_all_workspaces: false,
            skip_taskbar: false,
            resizable: false,
            shadow: true,
            focusable: true,
            ignore_cursor_events: false,
        },
        _ => return None,
    };
    Some(spec)
}

fn validate_route(label: &str, route: Option<&str>) -> Result<String, String> {
    if window_spec(label).is_none() {
        return Err("bad route".to_string());
    }
    let route = route.unwrap_or(label);
    let first_segment = route.split('/').next().unwrap_or_default();
    if route.is_empty()
        || first_segment != label
        || !route.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"/?=&_-".contains(&byte)
        })
    {
        return Err("bad route".to_string());
    }
    Ok(route.to_string())
}

/// Builds the script used to change the hash on an already-created webview.
/// The route grammar excludes quotes, but JSON encoding keeps this safe if the
/// grammar is expanded in the future.
fn route_script(route: &str) -> String {
    let hash = format!("#/{route}");
    let encoded = match serde_json::to_string(&hash) {
        Ok(encoded) => encoded,
        Err(_) => String::from("\"#/\""),
    };
    format!("location.hash = {encoded}")
}

/// Return the capsule frame in logical monitor coordinates.
fn capsule_frame(monitor: LogicalSize<f64>) -> (f64, f64, f64, f64) {
    ((monitor.width - 560.0) / 2.0, 30.0, 560.0, 48.0)
}

/// Return the border frame in logical monitor coordinates.
fn border_frame(monitor: LogicalSize<f64>) -> (f64, f64, f64, f64) {
    (0.0, 0.0, monitor.width, monitor.height)
}

fn monitor_logical_size(monitor: &tauri::Monitor) -> LogicalSize<f64> {
    let scale = monitor.scale_factor();
    LogicalSize::new(
        f64::from(monitor.size().width) / scale,
        f64::from(monitor.size().height) / scale,
    )
}

fn monitor_logical_origin(monitor: &tauri::Monitor) -> LogicalPosition<f64> {
    let scale = monitor.scale_factor();
    LogicalPosition::new(
        f64::from(monitor.position().x) / scale,
        f64::from(monitor.position().y) / scale,
    )
}

fn apply_primary_frame(window: &WebviewWindow, label: &str) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let size = monitor_logical_size(&monitor);
    let origin = monitor_logical_origin(&monitor);
    let (x, y, width, height) = match label {
        "capsule" => capsule_frame(size),
        "border" => border_frame(size),
        "confirm" => ((size.width - 440.0) / 2.0, 150.0, 440.0, 480.0),
        _ => return Ok(()),
    };
    window.set_size(LogicalSize::new(width, height))?;
    window.set_position(LogicalPosition::new(origin.x + x, origin.y + y))?;
    Ok(())
}

fn attach_window_handlers(window: &WebviewWindow, spec: WindowSpec) {
    let event_window = window.clone();
    let label = spec.label;
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } if spec.persistent => {
            api.prevent_close();
            let _ = event_window.hide();
        }
        WindowEvent::Focused(false) if label == "menu" => {
            let _ = event_window.hide();
        }
        _ => {}
    });
}

fn build_window(
    app: &tauri::AppHandle,
    route: &str,
    spec: WindowSpec,
) -> tauri::Result<WebviewWindow> {
    let url = WebviewUrl::App(PathBuf::from(format!("index.html#/{route}")));
    let mut builder = WebviewWindowBuilder::new(app, spec.label, url)
        .inner_size(spec.width, spec.height)
        .transparent(spec.transparent)
        .decorations(spec.decorations)
        .always_on_top(spec.always_on_top)
        .visible_on_all_workspaces(spec.visible_on_all_workspaces)
        .skip_taskbar(spec.skip_taskbar)
        .resizable(spec.resizable)
        .shadow(spec.shadow)
        .focusable(spec.focusable)
        .focused(false)
        .visible(spec.visible);
    if let (Some(width), Some(height)) = (spec.min_width, spec.min_height) {
        builder = builder.min_inner_size(width, height);
    }
    if let Some(title) = spec.title {
        builder = builder.title(title);
        #[cfg(target_os = "macos")]
        {
            builder = builder
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .hidden_title(true);
        }
    }
    if spec.centered {
        builder = builder.center();
    }
    let window = builder.build()?;
    if spec.ignore_cursor_events {
        window.set_ignore_cursor_events(true)?;
    }
    attach_window_handlers(&window, spec);
    Ok(window)
}

fn ensure_window(
    app: &tauri::AppHandle,
    route: &str,
    spec: WindowSpec,
) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(spec.label) {
        return Ok(window);
    }
    build_window(app, route, spec)
}

fn handle_ipc_event(app: &tauri::AppHandle, payload: &str) {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    let Some(method) = event.get("method").and_then(serde_json::Value::as_str) else {
        return;
    };
    let params = event.get("params");
    match method {
        "event.approvalRequested" => {
            let _ = window_show(app.clone(), "confirm".to_string(), None);
        }
        "event.approvalClosed" => {
            let _ = window_hide(app.clone(), "confirm".to_string());
        }
        "event.sessionsChanged" => {
            let has_sessions = params
                .and_then(|value| value.get("sessions"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|sessions| !sessions.is_empty());
            if has_sessions {
                let _ = window_show(app.clone(), "capsule".to_string(), None);
                let _ = window_show(app.clone(), "border".to_string(), None);
            } else {
                let _ = window_hide(app.clone(), "capsule".to_string());
                let _ = window_hide(app.clone(), "border".to_string());
            }
        }
        _ => {}
    }
}

fn handle_connection_event(app: &tauri::AppHandle, payload: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    let Some(connected) = value.get("connected").and_then(serde_json::Value::as_bool) else {
        return;
    };
    if !connected {
        let _ = window_hide(app.clone(), "capsule".to_string());
        let _ = window_hide(app.clone(), "border".to_string());
    }
}

// H17T supplies window behavior; keep these registered command signatures stable.

pub fn setup(app: &mut tauri::App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    for label in PERSISTENT_LABELS {
        let spec = window_spec(label).ok_or(tauri::Error::WindowNotFound)?;
        build_window(&handle, label, spec)?;
    }
    let event_handle = handle.clone();
    app.listen("ipc://event", move |event| {
        handle_ipc_event(&event_handle, event.payload());
    });
    let connection_handle = handle.clone();
    app.listen("ipc://connection", move |event| {
        handle_connection_event(&connection_handle, event.payload());
    });
    Ok(())
}

#[tauri::command]
pub fn window_show(
    app: tauri::AppHandle,
    label: String,
    route: Option<String>,
) -> Result<(), String> {
    let spec = window_spec(&label).ok_or_else(|| "unknown window".to_string())?;
    let has_route = route.is_some();
    let existed = app.get_webview_window(&label).is_some();
    let route = validate_route(&label, route.as_deref())?;
    if label == "bg" {
        return Ok(());
    }
    let window = ensure_window(&app, &route, spec).map_err(|error| error.to_string())?;
    if matches!(label.as_str(), "capsule" | "border" | "confirm") {
        apply_primary_frame(&window, &label).map_err(|error| error.to_string())?;
    }
    if has_route && existed {
        window
            .eval(route_script(&route))
            .map_err(|error| error.to_string())?;
    }
    window.show().map_err(|error| error.to_string())?;
    if !matches!(label.as_str(), "capsule" | "border") {
        window.set_focus().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn window_hide(app: tauri::AppHandle, label: String) -> Result<(), String> {
    let spec = window_spec(&label).ok_or_else(|| "unknown window".to_string())?;
    if label == "bg" {
        return Ok(());
    }
    let Some(window) = app.get_webview_window(&label) else {
        return Ok(());
    };
    if spec.persistent {
        window.hide().map_err(|error| error.to_string())?;
    } else {
        window.close().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_table_contains_all_labels_and_expected_lifecycle() {
        for label in [
            "bg", "menu", "capsule", "border", "confirm", "pair", "settings", "setup", "update",
        ] {
            assert!(window_spec(label).is_some(), "missing {label}");
        }
        for label in ["bg", "menu", "capsule", "border", "confirm"] {
            assert!(window_spec(label).expect("known label").persistent);
        }
        for label in ["pair", "settings", "setup", "update"] {
            assert!(!window_spec(label).expect("known label").persistent);
        }
        assert_eq!(window_spec("menu").expect("menu").width, 340.0);
        assert_eq!(window_spec("confirm").expect("confirm").width, 440.0);
        let settings = window_spec("settings").expect("settings");
        assert_eq!((settings.width, settings.height), (1100.0, 640.0));
        assert_eq!(
            (settings.min_width, settings.min_height),
            (Some(900.0), Some(560.0))
        );
        assert!(window_spec("unknown").is_none());
    }

    #[test]
    fn route_validation_rejects_cross_window_and_script_injection() {
        assert!(validate_route("settings", Some("settings/devices")).is_ok());
        assert!(validate_route("settings", Some("pair")).is_err());
        assert!(validate_route("pair", Some("pair';alert(1)//")).is_err());
        assert!(validate_route("nope", None).is_err());
        assert_eq!(
            route_script("settings/devices"),
            "location.hash = \"#/settings/devices\""
        );
    }

    #[test]
    fn frame_helpers_use_expected_logical_coordinates() {
        assert_eq!(
            capsule_frame(LogicalSize::new(1440.0, 900.0)),
            (440.0, 30.0, 560.0, 48.0)
        );
        assert_eq!(
            border_frame(LogicalSize::new(1440.0, 900.0)),
            (0.0, 0.0, 1440.0, 900.0)
        );
    }
}
