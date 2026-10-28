mod agent_service;
pub mod ipc_bridge;
mod pipe_trust;
mod system;
mod tray;
#[cfg(windows)]
mod win_pipe;
mod windows;

pub fn run() {
    // Uninstallation must work even when the normal UI instance is running.
    // Handle this before initializing the single-instance plugin or any windows.
    if std::env::args_os().any(|arg| arg == "--unregister-agent") {
        match agent_service::unregister() {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                eprintln!("Could not unregister Remote Host agent: {error}");
                std::process::exit(1);
            }
        }
    }
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_positioner::init())
        .invoke_handler(tauri::generate_handler![
            ipc_bridge::ipc_call,
            ipc_bridge::ipc_connected,
            windows::window_show,
            windows::window_hide,
            tray::tray_set_state,
            system::notify,
            system::open_url,
            system::reveal_path,
            system::quit_app,
            agent_service::agent_service_status,
            agent_service::agent_service_register,
            agent_service::agent_service_open_settings,
        ])
        .setup(|app| {
            agent_service::ensure_registered();
            ipc_bridge::start(app)?;
            windows::setup(app)?;
            tray::setup(app)?;
            Ok(())
        })
        .run(tauri::generate_context!());

    if result.is_err() {
        tracing::error!("Remote Host UI failed to run");
    }
}
