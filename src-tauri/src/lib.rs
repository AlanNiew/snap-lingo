//! SnapLingo — a lightweight, always-running screenshot OCR + translation
//! tool for Windows.
//!
//! Design notes that matter for the "light and fast" goal:
//!
//! * **No window exists at startup.** Only a tray icon. Webviews are created
//!   on demand and then reused, because building one costs tens of megabytes of
//!   WebView2 process memory.
//! * **The frozen-frame session** (see `capture`) means the screenshot is read
//!   once per hotkey press, and the overlay paints a pre-encoded preview.
//! * **Everything expensive is off the main thread**: capture, JPEG encoding,
//!   offline OCR, and the network.
//! * **Text extraction defaults to the offline Windows OCR engine**, so the app
//!   is useful with zero configuration and without a network round trip.

pub mod capture;
pub mod commands;
pub mod config;
pub mod hotkey;
pub mod imageops;
pub mod model;
pub mod ocr_win;
pub mod pipeline;
pub mod providers;
pub mod state;
pub mod tray;
pub mod windows;

use crate::state::AppState;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

/// Keep the log from growing without bound; it is a debugging aid, not a
/// record the user is expected to curate.
const LOG_MAX_BYTES: u64 = 512 * 1024;

/// 日志文件名，`commands.rs` 的「日志」页也读它。
pub const LOG_FILE_NAME: &str = "snaplingo.log";

/* ------------------------------------------------------------------ logging */

struct FileLogger {
    path: PathBuf,
}

impl log::Log for FileLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{}] {:<5} {}\n",
            timestamp(),
            record.level(),
            record.args()
        );
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

/// `HH:MM:SS` in UTC — enough to correlate events with a clock, without
/// pulling in a date/time library for it.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{:02}:{:02}:{:02}Z", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

fn init_logging(dir: &Path) {
    let path = dir.join(LOG_FILE_NAME);
    if std::fs::metadata(&path).map(|m| m.len() > LOG_MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }

    let levels = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    // A GUI build has no console, so the only durable channel is the file.
    let _ = log::set_boxed_logger(Box::new(FileLogger { path }));
    log::set_max_level(levels);
}

/// Record panics in the log instead of letting them vanish into a GUI build's
/// missing console. The release profile keeps `panic = "unwind"`, so a panic in
/// a command or task is contained to that task rather than killing the app.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        previous(info);
    }));
}

/// Watch for the one failure mode the app cannot recover from on its own: the
/// main thread stuck inside a webview call, which would leave an overlay
/// covering the screen with no way to close it. The pipeline cancels abandoned
/// selections well before this fires, so reaching the limit means the cancel
/// itself could not run — at that point the process takes itself down rather
/// than hold the user's screen hostage.
fn supervise(app: tauri::AppHandle) {
    use std::time::{Duration, Instant};

    const STUCK_LIMIT: Duration = Duration::from_secs(240);

    let mut busy_since: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_secs(10));

        if !app.state::<AppState>().busy.load(Ordering::SeqCst) {
            busy_since = None;
            continue;
        }

        let since = *busy_since.get_or_insert_with(Instant::now);
        if since.elapsed() < STUCK_LIMIT {
            continue;
        }

        log::error!(
            "截图流程卡住超过 {} 秒，强制退出进程，避免用户无法操作",
            STUCK_LIMIT.as_secs()
        );
        pipeline::notify(
            &app,
            "SnapLingo 已自动退出",
            "截图流程卡住了，程序为避免继续占用屏幕已自动退出，重新打开即可。",
        );
        std::thread::sleep(Duration::from_millis(400));
        std::process::exit(1);
    }
}

/* -------------------------------------------------------------------- paths */

fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .config_dir()
        .map_err(|err| format!("无法定位配置目录：{err}"))?;
    let dir = base.join(config::APP_DIR);
    std::fs::create_dir_all(&dir).map_err(|err| format!("无法创建配置目录 {dir:?}：{err}"))?;
    Ok(dir)
}

/* ----------------------------------------------------------------------- run */

pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            pipeline::begin_capture(app).await;
                        });
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None::<Vec<&str>>,
        ));

    builder = builder
        .setup(|app| {
            let handle = app.handle().clone();

            let dir = config_dir(&handle).map_err(|err| -> Box<dyn std::error::Error> { err.into() })?;
            init_logging(&dir);
            install_panic_hook();
            log::info!("SnapLingo {} starting", env!("CARGO_PKG_VERSION"));

            let state = AppState::new(dir)
                .map_err(|err| -> Box<dyn std::error::Error> { format!("{err:#}").into() })?;
            let shortcut = state.config().shortcut;
            let ready = state.config().is_ready();
            app.manage(state);
            app.manage(tray::TrayMenuState::default());

            tray::build(&handle)
                .map_err(|err| -> Box<dyn std::error::Error> { format!("创建托盘图标失败：{err}").into() })?;

            // Build the webviews the capture flow needs while nothing else is
            // happening, so a later capture only has to show/hide them.
            windows::prewarm(&handle);
            pipeline::spawn_delayed(
                handle.clone(),
                std::time::Duration::from_millis(1500),
                |app| windows::prewarm_overlay(&app),
            );

            match hotkey::apply(&handle, &shortcut) {
                Ok(()) => log::info!("全局快捷键已注册: {shortcut}"),
                Err(err) => {
                    log::warn!("{err}");
                    // Not fatal: the tray menu still works. Tell the settings
                    // page so it can show the problem instead of pretending.
                    let _ = handle.emit("hotkey:error", err);
                }
            }

            // First run (or a wiped config): open settings instead of leaving
            // the user with an app that does nothing when they press the key.
            if !ready {
                windows::show_settings(&handle);
            }

            // Last line of defence: never leave the screen covered forever.
            std::thread::spawn({
                let handle = handle.clone();
                move || supervise(handle)
            });

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let state = window.state::<AppState>();
                if state.quitting.load(Ordering::SeqCst) {
                    return;
                }
                // Settings and result windows are hidden, not destroyed: the
                // webview is reused and the tray app stays alive.
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Focused(false) => {
                if window.label() != windows::RESULT {
                    return;
                }
                let state = window.state::<AppState>();
                if state.quitting.load(Ordering::SeqCst) {
                    return;
                }
                if state.config().ui.close_on_blur {
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::reset_config,
            commands::list_models,
            commands::test_ocr_connection,
            commands::test_translate_connection,
            commands::test_local_ocr_connection,
            commands::describe_endpoints,
            commands::read_log,
            commands::clear_log,
            commands::reveal_log,
            commands::ocr_languages,
            commands::ocr_probe,
            commands::reveal_config_dir,
            commands::start_capture,
            commands::cancel_capture,
            commands::finish_selection,
            commands::overlay_ready,
            commands::get_session_image,
            commands::monitors_info,
            commands::get_last_result,
            commands::retry_translation,
            commands::hide_result,
            commands::fit_result_window,
            commands::copy_text,
            commands::get_history,
            commands::clear_history,
            commands::delete_history_item,
            commands::open_settings,
            commands::hide_settings,
            commands::open_external,
        ]);

    builder
        .build(tauri::generate_context!())
        .expect("构建 SnapLingo 失败")
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, code, .. } = event {
                let quitting = app.state::<AppState>().quitting.load(Ordering::SeqCst);
                // Closing the settings window must not end the process: this is
                // a background app. An explicit exit (`code.is_some()`) wins.
                if !quitting && code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
