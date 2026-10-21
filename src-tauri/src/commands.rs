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
pub async fn test_ocr_connection(
    state: State<'_, AppState>,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<ConnectionCheck, String> {
    Ok(state
        .providers
        .test(&base_url, &api_key, &model, "ocr")
        .await)
}

/// Exercise the translation engine that is actually configured — a chat model,
/// DeepL, or one of the cloud translators (which have their own credentials).
#[tauri::command]
pub async fn test_translate_connection(
    state: State<'_, AppState>,
    config: crate::config::TranslateConfig,
) -> Result<ConnectionCheck, String> {
    Ok(state.providers.test_translate(&config).await)
}

/// Round-trip check for a local HTTP OCR service (Umi-OCR and compatible
/// servers). Nothing leaves the user's machine.
#[tauri::command]
pub async fn test_local_ocr_connection(
    state: State<'_, AppState>,
    config: crate::config::OcrConfig,
) -> Result<ConnectionCheck, String> {
    Ok(state.providers.local_ocr_check(&config).await)
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
pub async fn ocr_probe(language: String) -> ConnectionCheck {
    let started = std::time::Instant::now();
    // The WinRT OCR call blocks its thread while it runs; keep it off the main
    // thread so probing can never freeze the settings window.
    let probe_language = language.clone();
    let result =
        match tauri::async_runtime::spawn_blocking(move || crate::ocr_win::probe(&probe_language))
            .await
        {
            Ok(result) => result,
            Err(err) => Err(anyhow::anyhow!("OCR 检测任务未能完成：{err}")),
        };
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

/* ----------------------------------------------------- 实际调用地址 / 日志 */

#[derive(serde::Serialize)]
pub struct EndpointInfo {
    /// `POST` / `GET`，或 `本机`（完全不联网）。
    pub method: String,
    pub url: String,
    pub note: String,
}

#[derive(serde::Serialize)]
pub struct Endpoints {
    pub ocr: EndpointInfo,
    pub translate: EndpointInfo,
    /// 一步图片翻译的地址（只有当前配置真的会走它时才有值）。
    pub picture: Option<EndpointInfo>,
    pub summary: String,
}

/// 把所有渠道「实际会请求的地址」算出来给设置页展示，避免用户猜。
#[tauri::command]
pub fn describe_endpoints(config: AppConfig) -> Endpoints {
    use crate::providers;

    let ocr = match config.ocr.provider.as_str() {
        "winocr" => EndpointInfo {
            method: "本机".into(),
            url: "—".into(),
            note: "Windows 内置 OCR，离线运行，不发网络请求".into(),
        },
        "local" => EndpointInfo {
            method: "POST".into(),
            url: providers::ocr_endpoint(&config.ocr).unwrap_or_else(|| "—".into()),
            note: "本地 OCR 服务（Umi-OCR 兼容 /api/ocr），图片不出本机".into(),
        },
        "vision" => EndpointInfo {
            method: "POST".into(),
            url: providers::ocr_endpoint(&config.ocr).unwrap_or_else(|| "—".into()),
            note: format!("视觉大模型{}", model_note(&config.ocr.active().model)),
        },
        other => match config.picture_provider() {
            Some(id) => EndpointInfo {
                method: "POST".into(),
                url: providers::image_endpoint(id, &config.translate).unwrap_or_else(|| "—".into()),
                note: format!(
                    "由「{}」一步完成识别 + 翻译{}{}",
                    providers::provider_label(id),
                    model_note(&config.translate.profile(id).model),
                    tier_suffix(id, &config, true)
                ),
            },
            None => EndpointInfo {
                method: "—".into(),
                url: "—".into(),
                note: format!("{other}：请到「翻译」页选一家支持图片翻译的渠道"),
            },
        },
    };

    let translate = match config.translate.provider.as_str() {
        "none" => EndpointInfo {
            method: "—".into(),
            url: "—".into(),
            note: "只识别，不翻译".into(),
        },
        id => EndpointInfo {
            method: "POST".into(),
            url: providers::text_endpoint(id, &config.translate).unwrap_or_else(|| "—".into()),
            note: format!(
                "目标语言 {}{}{}",
                config.translate.target_lang,
                model_note(&config.translate.profile(id).model),
                tier_suffix(id, &config, false)
            ),
        },
    };

    let picture = config.picture_provider().and_then(|id| {
        providers::image_endpoint(id, &config.translate).map(|url| EndpointInfo {
            method: "POST".into(),
            url,
            note: format!(
                "一步图片翻译（{}）{}",
                providers::provider_label(id),
                tier_suffix(id, &config, true)
            ),
        })
    });

    Endpoints {
        summary: providers::describe_run(&config),
        ocr,
        translate,
        picture,
    }
}

fn model_note(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("，模型 {trimmed}")
    }
}

/// 「 · 档位 …」后缀，说明通用 / 大模型档位真正改变了什么。
fn tier_suffix(provider: &str, config: &AppConfig, image: bool) -> String {
    let tier = config.translate.profile(provider).tier(provider);
    match crate::providers::tier_note(provider, &tier, image) {
        Some(note) if !tier.is_empty() => format!(" · {note}"),
        _ => String::new(),
    }
}

#[derive(serde::Serialize)]
pub struct LogFile {
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
    pub content: String,
}

/// 日志文件尾部（默认最后 256KB），「日志」页用来展示最近这次执行走了什么。
#[tauri::command]
pub fn read_log(app: AppHandle) -> Result<LogFile, String> {
    const TAIL_BYTES: usize = 256 * 1024;
    let path = config_dir_of(&app).join(crate::LOG_FILE_NAME);
    let display = path.to_string_lossy().to_string();
    if !path.exists() {
        return Ok(LogFile {
            path: display,
            exists: false,
            bytes: 0,
            content: String::new(),
        });
    }
    let data = std::fs::read(&path).map_err(|err| format!("读取日志失败：{err}"))?;
    let total = data.len();
    let slice = if total > TAIL_BYTES {
        let mut start = total - TAIL_BYTES;
        // 从下一行开始，避免第一条是半行。
        while start < total && data[start] != b'\n' {
            start += 1;
        }
        &data[start.min(total)..]
    } else {
        &data[..]
    };
    Ok(LogFile {
        path: display,
        exists: true,
        bytes: total as u64,
        content: String::from_utf8_lossy(slice).to_string(),
    })
}

#[tauri::command]
pub fn clear_log(app: AppHandle) -> Result<(), String> {
    let path = config_dir_of(&app).join(crate::LOG_FILE_NAME);
    std::fs::write(&path, b"").map_err(|err| format!("清空日志失败：{err}"))?;
    log::info!("日志已由用户清空");
    Ok(())
}

/// 在资源管理器里选中日志文件，方便直接拖给开发者。
#[tauri::command]
pub fn reveal_log(app: AppHandle) -> Result<(), String> {
    let path = config_dir_of(&app).join(crate::LOG_FILE_NAME);
    let target = if path.exists() {
        path
    } else {
        config_dir_of(&app)
    };
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", target.to_string_lossy()))
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("打开日志位置失败：{err}"))
}

/// 手册文件名（随安装包发布，见 `tauri.conf.json` 的 bundle.resources）。
const GUIDE_FILE_NAME: &str = "GUIDE.md";

/// 打开《使用手册》：新用户照着做一遍就会用。
#[tauri::command]
pub fn open_guide(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("docs").join(GUIDE_FILE_NAME));
        candidates.push(dir.join(GUIDE_FILE_NAME));
    }
    // 开发时手册就在仓库里；这个编译期路径只在开发机上存在，找不到就跳过。
    candidates.push(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("docs")
            .join(GUIDE_FILE_NAME),
    );

    let path = candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| format!("没有找到手册文件 {GUIDE_FILE_NAME}，可以改看项目里的 README.md。"))?;

    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| format!("打开手册失败：{err}"))
}

/* ------------------------------------------------------------------- capture */

#[tauri::command]
pub fn start_capture(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        pipeline::begin_capture(app).await;
    });
}

#[tauri::command]
pub async fn cancel_capture(app: AppHandle) {
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

/// `async` on purpose: a sync command runs on the main thread, i.e. inside the
/// overlay webview's own IPC callback, and the window work below would then run
/// inline on a thread that is already nested in a WebView2 callback — the kind
/// of re-entrancy that wedges the app. As an async command it runs on the async
/// runtime and every window operation is dispatched normally.
#[tauri::command]
pub async fn finish_selection(app: AppHandle, rect: SelectionInput) {
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
pub async fn overlay_ready(app: AppHandle, monitor_id: u32) {
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

/// Grow/shrink the result window to its content (only used when "fit the
/// content" is switched on), never taller than the screen it sits on, and keep
/// it centred while it changes.
#[tauri::command]
pub fn fit_result_window(app: AppHandle, width: f64, height: f64) {
    let Some(window) = app.get_webview_window(windows::RESULT) else {
        return;
    };
    let mut width = width.clamp(360.0, 1600.0);
    let mut height = height.clamp(200.0, 1400.0);

    let Ok(Some(monitor)) = window.current_monitor() else {
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
        return;
    };

    // `work_area` is physical; the values coming from the page are CSS pixels, so
    // scale the allowance instead of mixing the two unit systems.
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let margin = 24.0;
    let usable_w = area.size.width as f64 / scale - margin;
    let usable_h = area.size.height as f64 / scale - margin;
    if usable_w > 320.0 {
        width = width.min(usable_w);
    }
    if usable_h > 200.0 {
        height = height.min(usable_h);
    }

    let _ = window.set_size(tauri::LogicalSize::new(width, height));

    let origin = area.position.to_logical::<f64>(scale);
    let x = origin.x + ((usable_w + margin) - width) / 2.0;
    let y = origin.y + ((usable_h + margin) - height) / 2.0;
    let _ = window.set_position(tauri::LogicalPosition::new(x.max(origin.x), y.max(origin.y)));
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

/// Dismiss the branded notice (the notice itself calls this on click).
#[tauri::command]
pub fn hide_toast(app: AppHandle) {
    windows::hide_toast(&app);
    let state = app.state::<crate::state::AppState>();
    *crate::state::lock(&state.last_toast) = None;
}

/// Open the settings window straight on the log page — the notice points here.
#[tauri::command]
pub fn open_logs(app: AppHandle) {
    windows::hide_toast(&app);
    windows::show_settings(&app);
    let _ = app.emit(crate::pipeline::NAVIGATE_EVENT, "logs");
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

/// Ask GitHub for the newest release and compare it with this build.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> crate::update::UpdateInfo {
    let current = crate::update::app_version(&app);
    let repo = app.state::<AppState>().config().update.repo;
    crate::update::check(&repo, &current).await
}

/// Snapshot of the update download, so a reopened settings page can re-attach
/// to a download that is already running.
#[tauri::command]
pub fn update_state(state: State<'_, AppState>) -> crate::state::UpdateDownload {
    crate::state::lock(&state.update).clone()
}

fn emit_update(app: &AppHandle, snapshot: crate::state::UpdateDownload) {
    let _ = app.emit("update:progress", snapshot);
}

/// Starts downloading the newest installer in the background and returns at
/// once. The work lives in the app state, not in the page that asked for it, so
/// switching tabs or closing the settings window cannot cancel it; when the file
/// is complete the installer is launched automatically (one button, one click).
#[tauri::command]
pub async fn start_update_download(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::state::UpdateDownload, String> {
    let current = crate::state::lock(&state.update).clone();
    // Already running, or already on disk: report it instead of starting over.
    if current.status == "downloading" || current.status == "ready" {
        return Ok(current);
    }

    let version = crate::update::app_version(&app);
    let config = state.config();
    let repo = config.update.repo;
    let mirror = config.update.mirror;

    let started = {
        let mut slot = crate::state::lock(&state.update);
        *slot = crate::state::UpdateDownload {
            status: "downloading".into(),
            ..crate::state::UpdateDownload::default()
        };
        slot.clone()
    };
    emit_update(&app, started.clone());

    let worker = app.clone();
    tauri::async_runtime::spawn(async move {
        let reporter = worker.clone();
        let result = crate::update::download(&repo, &version, &mirror, move |progress| {
            let snapshot = {
                let state = reporter.state::<AppState>();
                let mut slot = crate::state::lock(&state.update);
                slot.received = progress.received;
                slot.total = progress.total;
                slot.clone()
            };
            emit_update(&reporter, snapshot);
        })
        .await;

        match result {
            Ok(done) => {
                let snapshot = {
                    let state = worker.state::<AppState>();
                    let mut slot = crate::state::lock(&state.update);
                    slot.status = "ready".into();
                    slot.received = done.bytes;
                    slot.total = done.bytes;
                    slot.path = done.path.clone();
                    slot.name = done.name.clone();
                    slot.bytes = done.bytes;
                    slot.error.clear();
                    slot.clone()
                };
                emit_update(&worker, snapshot);
                log::info!("更新包已下载完成，即将启动安装程序");
                match crate::update::install(std::path::Path::new(&done.path)) {
                    Ok(()) => {
                        let state = worker.state::<AppState>();
                        state.quitting.store(true, std::sync::atomic::Ordering::SeqCst);
                        let handle = worker.clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                            handle.exit(0);
                        });
                    }
                    Err(err) => {
                        let message = format!("{err:#}");
                        log::error!("启动安装程序失败：{message}");
                        let snapshot = {
                            let state = worker.state::<AppState>();
                            let mut slot = crate::state::lock(&state.update);
                            slot.status = "error".into();
                            slot.error = message;
                            slot.clone()
                        };
                        emit_update(&worker, snapshot);
                    }
                }
            }
            Err(err) => {
                let message = format!("{err:#}");
                log::error!("下载更新失败：{message}");
                let snapshot = {
                    let state = worker.state::<AppState>();
                    let mut slot = crate::state::lock(&state.update);
                    slot.status = "error".into();
                    slot.error = message;
                    slot.clone()
                };
                emit_update(&worker, snapshot);
            }
        }
    });

    Ok(started)
}

/// An installer a previous click already downloaded, so the UI can offer
/// "install" instead of pulling it again.
#[tauri::command]
pub fn cached_update(asset_name: String) -> Option<crate::update::Downloaded> {
    crate::update::cached(&asset_name)
}

/// Run a downloaded installer: this process steps aside so the installer can
/// replace the executable.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    crate::update::install(std::path::Path::new(&path)).map_err(|err| format!("{err:#}"))?;
    state.quitting.store(true, std::sync::atomic::Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        app.exit(0);
    });
    Ok(())
}

/// Open the settings window on the update page — the notice points here.
#[tauri::command]
pub fn open_update(app: AppHandle) {
    windows::hide_toast(&app);
    windows::show_settings(&app);
    let _ = app.emit(crate::pipeline::NAVIGATE_EVENT, "update");
}
