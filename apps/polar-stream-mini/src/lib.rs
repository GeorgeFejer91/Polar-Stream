use stream_mini_runtime::{
    CommandResult, MiniAppKind, MiniAppState, MiniBootstrap, MiniDeviceSummary, MiniEvent,
    MiniPreferencesInput, MiniSaveResult, MiniSessionSnapshot, NewNodeLaunch, graceful_shutdown,
    setup_mini_state,
};
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, State, WebviewWindow, ipc::Channel,
};

const COMPACT_WINDOW_SIZE: (f64, f64) = (388.0, 332.0);
const METRICS_WINDOW_SIZE: (f64, f64) = (620.0, 602.0);
const METRICS_GUIDE_URL: &str = "https://georgefejer91.github.io/Polar-Stream/";

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

#[tauri::command]
fn set_metrics_dialog_open(window: WebviewWindow, open: bool) -> Result<(), String> {
    let (width, height) = if open {
        METRICS_WINDOW_SIZE
    } else {
        COMPACT_WINDOW_SIZE
    };
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    let current_size = window.outer_size().map_err(|error| error.to_string())?;
    let current_position = window.outer_position().map_err(|error| error.to_string())?;
    let target_width = (width * scale).round() as i32;
    let target_height = (height * scale).round() as i32;
    let center_x = i64::from(current_position.x) + i64::from(current_size.width) / 2;
    let center_y = i64::from(current_position.y) + i64::from(current_size.height) / 2;
    let mut target_x = (center_x - i64::from(target_width) / 2) as i32;
    let mut target_y = (center_y - i64::from(target_height) / 2) as i32;

    if let Some(monitor) = window
        .current_monitor()
        .map_err(|error| error.to_string())?
    {
        let work_area = monitor.work_area();
        let min_x = work_area.position.x;
        let min_y = work_area.position.y;
        let max_x = min_x
            .saturating_add(work_area.size.width as i32)
            .saturating_sub(target_width)
            .max(min_x);
        let max_y = min_y
            .saturating_add(work_area.size.height as i32)
            .saturating_sub(target_height)
            .max(min_y);
        target_x = target_x.clamp(min_x, max_x);
        target_y = target_y.clamp(min_y, max_y);
    }

    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|error| error.to_string())?;
    window
        .set_position(PhysicalPosition::new(target_x, target_y))
        .map_err(|error| error.to_string())
}

#[cfg(desktop)]
#[tauri::command]
fn open_metrics_guide(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    app.opener()
        .open_url(METRICS_GUIDE_URL, None::<&str>)
        .map_err(|error| error.to_string())
}

#[cfg(mobile)]
#[tauri::command]
fn open_metrics_guide() -> Result<(), String> {
    Err("The metric guide is available in the desktop app.".into())
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
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_opener::init());
    let app = builder
        .setup(|app| {
            if let Some(icon) = app.default_window_icon().cloned()
                && let Some(window) = app.get_webview_window("main")
            {
                window.set_icon(icon)?;
            }
            setup_mini_state(app, MiniAppKind::Polar)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            attach_events,
            scan_devices,
            save_preferences,
            connect_device,
            connect_remembered,
            start_mock_stream,
            disconnect_device,
            open_new_node,
            open_mock_node,
            close_app,
            set_metrics_dialog_open,
            open_metrics_guide,
            get_autostart,
            set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Polar Stream Mini");
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
