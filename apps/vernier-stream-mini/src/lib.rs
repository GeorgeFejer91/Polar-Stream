use stream_mini_runtime::{
    CommandResult, MiniAppKind, MiniAppState, MiniBootstrap, MiniDeviceSummary, MiniEvent,
    MiniPreferencesInput, MiniSaveResult, MiniSessionSnapshot, NewNodeLaunch, graceful_shutdown,
    setup_mini_state,
};
use tauri::{AppHandle, Manager, State, ipc::Channel};

mod bluetooth_radio;

#[tauri::command]
async fn get_bluetooth_radio() -> Result<bluetooth_radio::RadioStatus, String> {
    bluetooth_radio::status().await
}

#[tauri::command]
async fn set_bluetooth_radio(
    state: State<'_, MiniAppState>,
    enabled: bool,
) -> Result<bluetooth_radio::RadioStatus, String> {
    if !enabled
        && stream_mini_runtime::get_bootstrap(state)
            .await
            .map_err(|error| error.message)?
            .session
            .is_some()
    {
        return Err("Disconnect the belt before turning Bluetooth off.".into());
    }
    bluetooth_radio::set_enabled(enabled).await
}

#[tauri::command]
async fn get_bootstrap(state: State<'_, MiniAppState>) -> CommandResult<MiniBootstrap> {
    stream_mini_runtime::get_bootstrap(state).await
}

#[tauri::command]
fn attach_events(state: State<'_, MiniAppState>, events: Channel<MiniEvent>) {
    stream_mini_runtime::attach_events(state, events);
}

#[tauri::command]
async fn scan_devices(state: State<'_, MiniAppState>) -> CommandResult<Vec<MiniDeviceSummary>> {
    stream_mini_runtime::scan_devices(state).await
}

#[tauri::command]
async fn save_preferences(
    state: State<'_, MiniAppState>,
    preferences: MiniPreferencesInput,
) -> CommandResult<MiniSaveResult> {
    stream_mini_runtime::save_preferences(state, preferences).await
}

#[tauri::command]
async fn connect_device(
    state: State<'_, MiniAppState>,
    device_id: String,
    preferences: MiniPreferencesInput,
) -> CommandResult<MiniSessionSnapshot> {
    stream_mini_runtime::connect_device(state, device_id, preferences).await
}

#[tauri::command]
async fn connect_remembered(state: State<'_, MiniAppState>) -> CommandResult<MiniSessionSnapshot> {
    stream_mini_runtime::connect_remembered(state).await
}

#[tauri::command]
async fn start_mock_stream(
    state: State<'_, MiniAppState>,
    preferences: MiniPreferencesInput,
) -> CommandResult<MiniSessionSnapshot> {
    stream_mini_runtime::start_mock_stream(state, preferences).await
}

#[tauri::command]
async fn disconnect_device(state: State<'_, MiniAppState>) -> CommandResult<MiniSessionSnapshot> {
    stream_mini_runtime::disconnect_device(state).await
}

#[tauri::command]
fn open_new_node() -> CommandResult<NewNodeLaunch> {
    stream_mini_runtime::open_new_node()
}

#[tauri::command]
fn open_mock_node() -> CommandResult<NewNodeLaunch> {
    stream_mini_runtime::open_mock_node()
}

#[tauri::command]
fn close_app(app: AppHandle) {
    stream_mini_runtime::close_app(app);
}

#[cfg(desktop)]
#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;

    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[cfg(desktop)]
#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;

    let autostart = app.autolaunch();
    if enabled {
        autostart.enable()
    } else {
        autostart.disable()
    }
    .map_err(|error| error.to_string())?;
    autostart.is_enabled().map_err(|error| error.to_string())
}

#[cfg(mobile)]
#[tauri::command]
fn get_autostart() -> Result<bool, String> {
    Err("Autostart is available only in the desktop app.".into())
}

#[cfg(mobile)]
#[tauri::command]
fn set_autostart(_enabled: bool) -> Result<bool, String> {
    Err("Autostart is available only in the desktop app.".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        None,
    ));
    let app = builder
        .setup(|app| {
            if let Some(icon) = app.default_window_icon().cloned()
                && let Some(window) = app.get_webview_window("main")
            {
                window.set_icon(icon)?;
            }
            setup_mini_state(app, MiniAppKind::Vernier)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            attach_events,
            scan_devices,
            get_bluetooth_radio,
            set_bluetooth_radio,
            save_preferences,
            connect_device,
            connect_remembered,
            start_mock_stream,
            disconnect_device,
            open_new_node,
            open_mock_node,
            close_app,
            get_autostart,
            set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Vernier Stream Mini");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
            let state = app.state::<MiniAppState>();
            if state.begin_shutdown() {
                api.prevent_exit();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    graceful_shutdown(app.clone()).await;
                    app.exit(code.unwrap_or(0));
                });
            }
        }
    });
}
