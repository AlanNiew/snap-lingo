//! The frontend ↔ Rust surface.
//!
//! Every name here is mirrored in `src/lib/api.ts`. Commands stay thin: the
//! interesting work is in `pipeline`, `state` and `providers`.

use crate::capture;
use crate::config::AppConfig;
use crate::model::{ConnectionCheck, HistoryItem, ResultPayload, SelectionRect, SessionImage};
use crate::pipeline;
use crate::state::{lock, AppState};
use crate::tray;
use crate::windows;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

/* -------------------------------------------------------------------- config */

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config()
}

#[tauri::command]
pub fn save_config(app: AppHandle, mut config: AppConfig) -> Result<AppConfig, String> {
    config.sanitize();

    let state = app.state::<AppState>();
    let previous = state.config();
    state
        .replace_config(config.clone())
        .map_err(|err| format!("保存配置失败：{err:#}"))?;

    if previous.shortcut != config.shortcut {
        if let Err(err) = crate::hotkey::apply(&app, &config.shortcut) {
            // The old shortcut was already unregistered, so restore it rather
            // than leaving the user with no hotkey at all.
            let _ = crate::hotkey::apply(&app, &previous.shortcut);
            let mut reverted = config.clone();
            reverted.shortcut = previous.shortcut.clone();
            let state = app.state::<AppState>();
            let _ = state.replace_config(reverted.clone());
            let _ = app.emit(pipeline::CONFIG_EVENT, reverted);
            return Err(err);
        }
    }

    if previous.startup != config.startup {
        if let Err(err) = tray::set_autostart(&app, config.startup) {
            log::warn!("设置开机自启失败: {err}");
        }
    }
    tray::sync_autostart_check(&app, config.startup);
    tray::refresh_tooltip(&app);

    let _ = app.emit(pipeline::CONFIG_EVENT, config.clone());
    Ok(config)
}

#[tauri::command]
pub fn reset_config(app: AppHandle) -> Result<AppConfig, String> {
    save_config(app, AppConfig::default())
}

/* ------------------------------------------------------------------ services */

#[tauri::command]
pub async fn list_models(
    state: State<'_, AppState>,
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    state
        .providers
        .list_models(&base_url, &api_key)
        .await
        .map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    kind: String,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<ConnectionCheck, String> {
    Ok(state.providers.test(&base_url, &api_key, &model, &kind).await)
}

/// One language pack the local Windows OCR engine can be told to use.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrLanguage {
    pub tag: String,
    pub name: String,
}

#[tauri::command]
pub fn ocr_languages() -> Vec<OcrLanguage> {
    crate::ocr_win::available_languages()
        .into_iter()
        .map(|tag| {
            let name = crate::ocr_win::describe_language(&tag).unwrap_or_else(|| tag.clone());
            OcrLanguage { tag, name }
        })
        .collect()
}

/// Exercise the offline engine without a screenshot, so the user can tell an
/// unconfigured machine from a broken one.
#[tauri::command]
pub fn ocr_probe(language: String) -> ConnectionCheck {
    let started = std::time::Instant::now();
    let result = crate::ocr_win::probe(&language);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let mut check = match result {
        Ok(()) => ConnectionCheck {
            ok: true,
            message: if language.trim().is_empty() {
                "离线 OCR 引擎可用（跟随系统语言）".to_string()
            } else {
                format!("离线 OCR 引擎可用（{language}）")
            },
            elapsed_ms,
            sample: None,
        },
        Err(err) => ConnectionCheck {
            ok: false,
            message: format!("{err:#}"),
            elapsed_ms,
            sample: None,
        },
    };
    if !language.trim().is_empty()
        && !crate::ocr_win::available_languages()
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(language.trim()))
    {
        check.ok = false;
        check.message = format!(
            "系统未安装「{language}」的 OCR 语言包：请在 Windows 设置 → 时间和语言 → 语言和区域 中为该语言添加「可选功能 → 光学字符识别」。"
        );
    }
    check
}

#[tauri::command]
pub fn reveal_config_dir(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;
    let dir = config_dir_of(&app);
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| format!("打开目录失败：{err}"))
}

/// Same app-config folder `lib.rs` sets up; kept here so the command does not
/// reach into private helpers.
fn config_dir_of(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .config_dir()
        .map(|base| base.join(crate::config::APP_DIR))
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/* ------------------------------------------------------------------- capture */

#[tauri::command]
pub fn start_capture(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        pipeline::begin_capture(app).await;
    });
}

#[tauri::command]
pub fn cancel_capture(app: AppHandle) {
    pipeline::cancel_capture(&app);
}

/// Rect coordinates arrive in *physical* pixels relative to the monitor's
/// top-left corner — the overlay converts CSS pixels using the frame size it
/// gets from `get_session_image`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionInput {
    pub monitor_id: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[tauri::command]
pub fn finish_selection(app: AppHandle, rect: SelectionInput) {
    let rect = SelectionRect {
        monitor_id: rect.monitor_id,
        x: rect.x.round() as i32,
        y: rect.y.round() as i32,
        width: rect.width.round() as i32,
        height: rect.height.round() as i32,
    };
    pipeline::finish_selection(&app, rect);
}

#[tauri::command]
pub fn overlay_ready(app: AppHandle, monitor_id: u32) {
    pipeline::overlay_ready(&app, monitor_id);
}

#[tauri::command]
pub fn get_session_image(state: State<'_, AppState>, monitor_id: u32) -> Option<SessionImage> {
    let session = lock(&state.session);
    let entry = session.as_ref().and_then(|s| s.entry(monitor_id))?;
    // `width`/`height` are the *frame* size in physical pixels: the overlay
    // needs them to turn mouse positions into physical coordinates, and it
    // paints the preview stretched over the whole viewport anyway.
    Some(SessionImage {
        monitor_id: entry.frame.id,
        data_url: entry.preview_data_url.clone(),
        width: entry.frame.width,
        height: entry.frame.height,
        scale_factor: entry.frame.scale_factor,
        name: entry.frame.name.clone(),
    })
}

#[tauri::command]
pub fn monitors_info() -> Result<Vec<capture::MonitorInfo>, String> {
    capture::monitors().map_err(|err| format!("{err:#}"))
}

/* -------------------------------------------------------------------- result */

#[tauri::command]
pub fn get_last_result(state: State<'_, AppState>) -> Option<ResultPayload> {
    lock(&state.last_result).clone()
}

#[tauri::command]
pub fn retry_translation(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        pipeline::retranslate(app).await;
    });
}

#[tauri::command]
pub fn hide_result(app: AppHandle) {
    if let Some(window) = app.get_webview_window(windows::RESULT) {
        let _ = window.hide();
    }
}

/// Grow/shrink the result window to its content, within sane bounds.
#[tauri::command]
pub fn fit_result_window(app: AppHandle, width: f64, height: f64) {
    if let Some(window) = app.get_webview_window(windows::RESULT) {
        let width = width.clamp(360.0, 900.0);
        let height = height.clamp(200.0, 820.0);
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
    }
}

#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt as _;
    app.clipboard()
        .write_text(text)
        .map_err(|err| format!("复制失败：{err}"))
}

/* ------------------------------------------------------------------- history */

#[tauri::command]
pub fn get_history(state: State<'_, AppState>) -> Vec<HistoryItem> {
    state.history_snapshot()
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) {
    state.clear_history();
}

#[tauri::command]
pub fn delete_history_item(state: State<'_, AppState>, id: String) -> Vec<HistoryItem> {
    state.remove_history(&id)
}

/* ---------------------------------------------------------------- misc / ui */

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    windows::show_settings(&app);
}

#[tauri::command]
pub fn hide_settings(app: AppHandle) {
    if let Some(window) = app.get_webview_window(windows::SETTINGS) {
        let _ = window.hide();
    }
}

#[tauri::command]
pub fn open_external(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| format!("打开链接失败：{err}"))
}
