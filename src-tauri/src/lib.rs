pub mod helper;
mod models;
mod summary_monitor;
mod tray_icon;

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};

use anyhow::{Context, Result};
use helper::{request_process_monitoring_inner, stop_process_monitoring_inner, HelperSession};
use models::{
    BootstrapState, MonitoringState, PermissionState, ProcessSnapshot, SummarySnapshot,
    EVENT_MONITORING_STATE, EVENT_PROCESSES, EVENT_SUMMARY,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, Position, State, WebviewWindow,
    WebviewWindowBuilder, WebviewUrl, WindowEvent,
};
use tray_icon::{format_speed, render_summary_icon};

#[derive(Default)]
pub struct AppState {
    pub summary: RwLock<SummarySnapshot>,
    pub processes: RwLock<Vec<ProcessSnapshot>>,
    pub monitoring: RwLock<MonitoringState>,
    pub helper: Mutex<Option<HelperSession>>,
    pub details_visible: AtomicBool,
}

pub type SharedState = Arc<AppState>;

#[tauri::command]
fn get_bootstrap_state(state: State<'_, SharedState>) -> BootstrapState {
    BootstrapState {
        summary: state.summary.read().unwrap().clone(),
        processes: state.processes.read().unwrap().clone(),
        monitoring_state: state.monitoring.read().unwrap().clone(),
    }
}

#[tauri::command]
fn show_details_window(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> std::result::Result<MonitoringState, String> {
    show_details_window_inner(&app).map_err(|error| error.to_string())?;
    request_process_monitoring_inner(&app, state.inner().clone()).map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_details_window(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> std::result::Result<MonitoringState, String> {
    hide_details_window_inner(&app).map_err(|error| error.to_string())?;
    stop_process_monitoring_inner(&app, state.inner().clone()).map_err(|error| error.to_string())
}

#[tauri::command]
fn request_process_monitoring(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> std::result::Result<MonitoringState, String> {
    request_process_monitoring_inner(&app, state.inner().clone()).map_err(|error| error.to_string())
}

#[tauri::command]
fn stop_process_monitoring(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> std::result::Result<MonitoringState, String> {
    stop_process_monitoring_inner(&app, state.inner().clone()).map_err(|error| error.to_string())
}

pub fn emit_summary(app: &AppHandle, summary: &SummarySnapshot) {
    let _ = app.emit(EVENT_SUMMARY, summary);
}

pub fn emit_processes(app: &AppHandle, processes: &[ProcessSnapshot]) {
    let _ = app.emit(EVENT_PROCESSES, processes);
}

pub fn emit_monitoring_state(app: &AppHandle, monitoring_state: &MonitoringState) {
    let _ = app.emit(EVENT_MONITORING_STATE, monitoring_state);
}

pub fn update_summary_state(app: &AppHandle, state: &SharedState, summary: SummarySnapshot) {
    {
        let mut current = state.summary.write().unwrap();
        *current = summary.clone();
    }

    update_tray(app, &summary);
    emit_summary(app, &summary);
}

pub fn update_process_state(app: &AppHandle, state: &SharedState, processes: Vec<ProcessSnapshot>) {
    {
        let mut current = state.processes.write().unwrap();
        *current = processes.clone();
    }

    emit_processes(app, &processes);
}

pub fn update_monitoring_state(
    app: &AppHandle,
    state: &SharedState,
    apply: impl FnOnce(&mut MonitoringState),
) -> MonitoringState {
    let snapshot = {
        let mut current = state.monitoring.write().unwrap();
        apply(&mut current);
        current.clone()
    };

    emit_monitoring_state(app, &snapshot);
    snapshot
}

pub fn clear_process_state(app: &AppHandle, state: &SharedState) {
    update_process_state(app, state, Vec::new());
}

fn ensure_main_window(app: &AppHandle) -> Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window("main") {
        return Ok(window);
    }

    WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("网速监控")
        .inner_size(920.0, 680.0)
        .visible(false)
        .build()
        .context("failed to create main window")
}

fn position_details_window(window: &WebviewWindow) -> Result<()> {
    let Some(monitor) = window.current_monitor()? else {
        return Ok(());
    };

    let size = window.outer_size()?;
    let monitor_size = monitor.size();
    let monitor_position = monitor.position();

    let x = monitor_position.x + monitor_size.width as i32 - size.width as i32 - 24;
    let y = monitor_position.y + monitor_size.height as i32 - size.height as i32 - 56;

    window.set_position(Position::Physical(PhysicalPosition::new(x.max(0), y.max(0))))?;
    Ok(())
}

fn show_details_window_inner(app: &AppHandle) -> Result<()> {
    let window = ensure_main_window(app)?;
    position_details_window(&window)?;
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;

    if let Some(state) = app.try_state::<SharedState>() {
        state.details_visible.store(true, Ordering::Relaxed);
    }

    Ok(())
}

fn hide_details_window_inner(app: &AppHandle) -> Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.hide()?;
    }

    if let Some(state) = app.try_state::<SharedState>() {
        state.details_visible.store(false, Ordering::Relaxed);
    }

    Ok(())
}

fn update_tray(app: &AppHandle, summary: &SummarySnapshot) {
    let Some(tray) = app.tray_by_id("netmonitor-tray") else {
        return;
    };

    let icon = render_summary_icon(summary.down_bps, summary.up_bps);
    let tooltip = format!(
        "网速监控\n下载: {down}\n上传: {up}\n网卡: {adapters}",
        down = format_speed(summary.down_bps),
        up = format_speed(summary.up_bps),
        adapters = if summary.adapters.is_empty() {
            "暂无可用网卡".to_string()
        } else {
            summary.adapters.join(", ")
        }
    );

    let _ = tray.set_icon(Some(icon));
    let _ = tray.set_tooltip(Some(tooltip));
}

fn setup_tray(app: &AppHandle, state: &SharedState) -> Result<()> {
    let open_item = MenuItem::with_id(app, "open", "打开详情", true, None::<&str>)?;
    let retry_item =
        MenuItem::with_id(app, "retry", "重试进程监控", true, None::<&str>)?;
    let stop_item =
        MenuItem::with_id(app, "stop", "停止进程监控", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &retry_item, &stop_item, &quit_item])?;

    let state_for_menu = state.clone();
    let state_for_tray = state.clone();

    TrayIconBuilder::with_id("netmonitor-tray")
        .icon(render_summary_icon(0, 0))
        .tooltip("网速监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => {
                let _ = show_details_window_inner(app);
                let _ = request_process_monitoring_inner(app, state_for_menu.clone());
            }
            "retry" => {
                let _ = request_process_monitoring_inner(app, state_for_menu.clone());
            }
            "stop" => {
                let _ = stop_process_monitoring_inner(app, state_for_menu.clone());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                let _ = show_details_window_inner(&app);
                let _ = request_process_monitoring_inner(&app, state_for_tray.clone());
            }
        })
        .build(app)
        .context("failed to build tray icon")?;

    Ok(())
}

fn install_window_behavior(app: &AppHandle, state: &SharedState) -> Result<()> {
    let window = ensure_main_window(app)?;
    let app_handle = app.clone();
    let state = state.clone();

    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hide_details_window_inner(&app_handle);
            let _ = stop_process_monitoring_inner(&app_handle, state.clone());
        }
    });

    Ok(())
}

fn seed_initial_state(app: &AppHandle, state: &SharedState) {
    let monitoring = MonitoringState {
        process_details_enabled: false,
        permission_state: PermissionState::Idle,
        last_error: None,
    };
    {
        let mut current = state.monitoring.write().unwrap();
        *current = monitoring.clone();
    }

    emit_monitoring_state(app, &monitoring);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state: SharedState = Arc::new(AppState::default());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state.clone())
        .setup(move |app| {
            ensure_main_window(app.handle())?;
            install_window_behavior(app.handle(), &state)?;
            setup_tray(app.handle(), &state)?;
            seed_initial_state(app.handle(), &state);
            summary_monitor::start(app.handle().clone(), state.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap_state,
            show_details_window,
            hide_details_window,
            request_process_monitoring,
            stop_process_monitoring
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
