//! Tray icon and its menu — the app's only always-visible surface, since it
//! lives in the background with no windows open.

use crate::pipeline;
use crate::state::lock;
use crate::windows;
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "snap-lingo-tray";
const ID_CAPTURE: &str = "tray-capture";
const ID_CANCEL: &str = "tray-cancel";
const ID_SETTINGS: &str = "tray-settings";
const ID_HISTORY: &str = "tray-history";
const ID_LOGS: &str = "tray-logs";
const ID_UPDATE: &str = "tray-update";
const ID_AUTOSTART: &str = "tray-autostart";
const ID_QUIT: &str = "tray-quit";

/// The check item is kept around so the menus and the settings checkbox never
/// drift apart.
#[derive(Default)]
pub struct TrayMenuState {
    autostart: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
}

/// Bundled next to the crate so the tray also works under `cargo run`, where
/// there is no installed icon resource to fall back on.
fn tray_icon() -> tauri::Result<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let capture = MenuItem::with_id(app, ID_CAPTURE, "截图翻译", true, None::<&str>)?;
    let cancel = MenuItem::with_id(app, ID_CANCEL, "取消本次截图", true, None::<&str>)?;
    let history = MenuItem::with_id(app, ID_HISTORY, "历史记录…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, ID_LOGS, "查看日志…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, ID_SETTINGS, "设置…", true, None::<&str>)?;
    let update = MenuItem::with_id(app, ID_UPDATE, "检查更新…", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let startup = CheckMenuItem::with_id(
        app,
        ID_AUTOSTART,
        "开机自启动",
        true,
        autostart_enabled(app),
        None::<&str>,
    )?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "退出 SnapLingo", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &capture,
            &cancel,
            &history,
            &logs,
            &settings,
            &update,
            &sep1,
            &startup,
            &sep2,
            &quit,
        ],
    )?;

    *lock(&app.state::<TrayMenuState>().autostart) = Some(startup);

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(tooltip(app))
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            // A plain click opens the settings window (the capture entry lives in
            // the menu above and on the global shortcut) — clicking the tray must
            // never grab the screen by surprise.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    windows::show_settings(&app);
                });
            }
        });

    if let Ok(icon) = tray_icon() {
        builder = builder.icon(icon);
    } else if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

/// Keep the tooltip in sync after the shortcut changes.
pub fn refresh_tooltip(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip(app)));
    }
}

/// Mirror a change made from the settings page back into the tray checkbox.
pub fn sync_autostart_check(app: &AppHandle, enabled: bool) {
    if let Some(item) = lock(&app.state::<TrayMenuState>().autostart).as_ref() {
        let _ = item.set_checked(enabled);
    }
}

fn tooltip(app: &AppHandle) -> String {
    let shortcut = app.state::<crate::state::AppState>().config().shortcut;
    format!("SnapLingo · {shortcut} 截图翻译 · 单击打开设置")
}

fn autostart_enabled(app: &AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt as _;
    app.autolaunch().is_enabled().unwrap_or(false)
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt as _;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|err| err.to_string())
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        ID_CAPTURE => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                pipeline::begin_capture(app).await;
            });
        }
        ID_CANCEL => pipeline::cancel_capture(app),
        ID_HISTORY => {
            windows::show_settings(app);
            let _ = app.emit(crate::pipeline::NAVIGATE_EVENT, "history");
        }
        ID_LOGS => {
            windows::show_settings(app);
            let _ = app.emit(crate::pipeline::NAVIGATE_EVENT, "logs");
        }
        ID_SETTINGS => windows::show_settings(app),
        ID_UPDATE => {
            // Menu callbacks run on the main thread, so the HTTP work and the
            // resulting notice are pushed onto the async runtime.
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let repo = app.state::<crate::state::AppState>().config().update.repo;
                let info = crate::update::check(&repo, &crate::update::app_version(&app)).await;
                if let Some(error) = info.error {
                    pipeline::notify(&app, "error", &format!("检查更新失败：{error}"));
                } else if info.has_update {
                    pipeline::notify_update(
                        &app,
                        &format!("发现新版本 v{}（当前 v{}），点这里查看更新。", info.latest, info.current),
                    );
                } else {
                    pipeline::notify(
                        &app,
                        "notice",
                        &format!("已是最新版本 v{}。", info.current),
                    );
                }
            });
        }
        ID_AUTOSTART => {
            let target = !autostart_enabled(app);
            match set_autostart(app, target) {
                Ok(()) => sync_autostart_check(app, target),
                Err(err) => log::warn!("切换开机自启失败: {err}"),
            }

            let state = app.state::<crate::state::AppState>();
            let mut cfg = state.config();
            cfg.startup = autostart_enabled(app);
            if let Err(err) = state.replace_config(cfg) {
                log::warn!("保存开机自启设置失败: {err:#}");
            }
            let _ = app.emit("config:changed", state.config());
        }
        ID_QUIT => {
            let state = app.state::<crate::state::AppState>();
            state.quitting.store(true, std::sync::atomic::Ordering::SeqCst);
            // Drop any overlay first, so quitting never leaves the screen
            // covered while the process unwinds.
            pipeline::cancel_capture(app);

            // Belt and braces: if anything refuses to unwind, take the process
            // down anyway. A tray app the user cannot quit is worse than one
            // that exits a little abruptly.
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(2500));
                log::warn!("退出超时，强制结束进程");
                std::process::exit(0);
            });

            app.exit(0);
        }
        _ => {}
    }
}
