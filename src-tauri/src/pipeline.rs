//! The whole product in one file: hotkey → frozen frames → overlays →
//! selection → OCR → translation → result.
//!
//! Two rules shape everything here:
//!
//! 1. **The UI thread never blocks.** Capturing, encoding, OCR and the network
//!    all happen on the async runtime or a blocking pool.
//! 2. **Failures are shown, never swallowed.** Any error becomes a result
//!    window with the message in it, because a background app that silently
//!    does nothing is indistinguishable from a broken one.

use crate::capture::{self, MonitorFrame};
use crate::config::AppConfig;
use crate::imageops;
use crate::model::{new_id, now_ms, ResultPayload, SelectionRect, Stage, StageEvent};
use crate::ocr_win;
use crate::providers::{self, ImagePayload, Providers};
use crate::state::{lock, AppState, CaptureSession, SessionEntry};
use crate::windows;
use anyhow::{anyhow, bail, Result};
use image::RgbaImage;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const STAGE_EVENT: &str = "pipeline:stage";
pub const RESULT_EVENT: &str = "result:update";
pub const CONFIG_EVENT: &str = "config:changed";

/// Sent to a warm overlay when a new capture is frozen, so the page swaps in the
/// fresh frame instead of showing the previous one.
pub const SESSION_EVENT: &str = "session:changed";
/// Ask the settings window to switch to a page (`"logs"`, `"history"`, …).
pub const NAVIGATE_EVENT: &str = "ui:navigate";
/// Payload sent to the branded notice window (`toast.html`).
pub const TOAST_EVENT: &str = "toast:changed";

/// If a webview has not reported that it painted within this window, reveal the
/// overlay anyway. Without it a failed page load would leave the app stuck in
/// "capturing" forever.
const OVERLAY_READY_TIMEOUT: Duration = Duration::from_millis(1400);

/// How long a frozen session may sit unconfirmed before it cancels itself, so a
/// forgotten overlay can never leave the hotkey dead.
const SESSION_TIMEOUT: Duration = Duration::from_secs(180);

/// Hard ceilings on every stage that can stall. Each one ends in a visible,
/// retryable error rather than a spinner that never stops.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
const OCR_TIMEOUT: Duration = Duration::from_secs(30);
const VISION_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSLATE_TIMEOUT: Duration = Duration::from_secs(120);
const JOB_TIMEOUT: Duration = Duration::from_secs(150);

/// One-step picture translation is a single upload, so it gets its own ceiling.
const IMAGE_TRANSLATE_TIMEOUT: Duration = Duration::from_secs(90);

/// Youdao refuses pictures over 5 MB once base64 encoded, Baidu over 4 MB raw.
/// Anything above this is re-encoded smaller before it is uploaded.
const MAX_PICTURE_BYTES: usize = 3_500_000;

fn emit_stage(app: &AppHandle, stage: Stage, message: Option<&str>, elapsed_ms: u64) {
    let event = StageEvent {
        stage,
        message: message.map(|m| m.to_string()),
        elapsed_ms,
    };
    if let Err(err) = app.emit(STAGE_EVENT, event) {
        log::error!("stage 事件发送失败: {err}");
    }
}

/* ------------------------------------------------------------------ capture */

/// Downscale and JPEG-encode one monitor's backdrop.
///
/// This is the slowest step of the whole hotkey path, so it is kept apart and
/// can run once per monitor on its own thread. Quality 78 is plenty: the image
/// sits behind the selection rectangle purely so the user can aim, and the real
/// crop is taken from the full-resolution frame.
fn build_entry(frame: MonitorFrame, edge: u32) -> Result<SessionEntry> {
    // The scaled copy is only needed to encode the backdrop; dropping it here
    // saves a full-size RGBA buffer per monitor (16 MB on a 2560×1600 screen).
    let preview_data_url = {
        let scaled = imageops::scale_to_long_edge(&frame.frame, edge);
        imageops::to_jpeg_data_url(&scaled, 82)?
    };
    Ok(SessionEntry {
        frame,
        preview_data_url,
    })
}

/// Hotkey / tray / button entry point. Freezes every screen, then hands control
/// to the overlays.
pub async fn begin_capture(app: AppHandle) {
    let state = app.state::<AppState>();

    if state.busy.swap(true, Ordering::SeqCst) {
        // Pressing the hotkey again is the way out: the same key that got the
        // user in gets them out, even when the overlay on the screen they are
        // looking at is not the one holding focus.
        log::info!("截图进行中再次触发快捷键：取消本次截图");
        cancel_capture(&app);
        return;
    }

    let cfg = state.config();
    if !cfg.is_ocr_ready() {
        fail(
            &app,
            "还没有可用的识别方式。请在设置里把 OCR 切换为「Windows 离线 OCR」（离线、免费），填写视觉模型接口的地址、密钥和模型，或把翻译渠道换成支持「一步图片翻译」的有道 / 百度 / 腾讯。",
        );
        return;
    }
    if !cfg.is_ready() {
        // Translation is not configured: still take the shot. The recognised
        // text is worth having on its own, and the translation step reports its
        // own error (with a retry button) instead of blocking the feature.
        log::warn!("翻译服务尚未配置完整，本次只做识别");
    }

    emit_stage(&app, Stage::Capturing, Some("正在截取屏幕…"), 0);

    let started = Instant::now();

    // Our own windows must never appear inside the shot.
    let hid = windows::hide_transient(&app);
    if hid {
        // Only pay for the compositor round trip when something was actually on
        // screen: an extra 60 ms on every hotkey press is exactly the kind of
        // unexplained lag we want out of this path.
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    let all_monitors = cfg.capture.all_monitors;
    let preview_edge = windows::PREVIEW_LONG_EDGE;
    // Physical pixels, resolved before the blocking capture starts. Single-screen
    // mode follows the pointer, so the layer appears on the screen being used.
    let cursor = app
        .cursor_position()
        .ok()
        .map(|point| (point.x.round() as i32, point.y.round() as i32));
    let prepared = match tokio::time::timeout(
        CAPTURE_TIMEOUT,
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<SessionEntry>, u128, u128)> {
            let t_capture = Instant::now();
            let frames = capture::capture(all_monitors, cursor)?;
            let capture_ms = t_capture.elapsed().as_millis();

            // Encode each monitor's backdrop; with several screens this runs on
            // one thread per screen, because encoding is the slowest part of the
            // whole path (it is what the user waits for).
            let t_encode = Instant::now();
            let entries = if frames.len() > 1 {
                std::thread::scope(|scope| -> Result<Vec<SessionEntry>> {
                    let mut handles = Vec::with_capacity(frames.len());
                    for frame in frames {
                        handles.push(scope.spawn(move || build_entry(frame, preview_edge)));
                    }
                    let mut out = Vec::with_capacity(handles.len());
                    for handle in handles {
                        out.push(
                            handle
                                .join()
                                .unwrap_or_else(|_| Err(anyhow!("预览编码线程异常")))?,
                        );
                    }
                    Ok(out)
                })?
            } else {
                frames
                    .into_iter()
                    .map(|frame| build_entry(frame, preview_edge))
                    .collect::<Result<Vec<SessionEntry>>>()?
            };
            Ok((entries, capture_ms, t_encode.elapsed().as_millis()))
        }),
    )
    .await
    {
        Ok(joined) => joined,
        Err(_) => {
            return fail(
                &app,
                &format!(
                    "截取屏幕超过 {} 秒仍未完成，已放弃本次截图。请重试；若一直失败，可尝试关闭占用显卡的程序。",
                    CAPTURE_TIMEOUT.as_secs()
                ),
            )
        }
    };

    let (entries, capture_ms, encode_ms) = match prepared {
        Ok(Ok(parts)) => parts,
        Ok(Err(err)) => return fail(&app, &format!("{err:#}")),
        Err(err) => return fail(&app, &format!("截图线程异常：{err}")),
    };

    let ids: Vec<u32> = entries.iter().map(|entry| entry.frame.id).collect();
    // The overlay to give focus: the first entry, which is the primary screen in
    // multi-monitor mode and the pointer's screen in single-screen mode.
    let focus_id = ids.first().copied();
    *lock(&state.session) = Some(CaptureSession {
        entries,
        started: Instant::now(),
    });

    let mut created = 0usize;
    log::info!("已冻结 {} 个显示器画面，准备框选层", ids.len());
    for id in &ids {
        let frame = {
            let session = lock(&state.session);
            session
                .as_ref()
                .and_then(|s| s.entry(*id))
                .map(|entry| entry.frame.clone())
        };
        let Some(frame) = frame else { continue };
        match windows::create_overlay(&app, &frame) {
            Ok(_) => {
                created += 1;
                // A warm overlay still shows the previous capture, so tell it to
                // pick up the new frame. A freshly built one may not have its
                // listener attached yet — its own boot fetch covers that case.
                //
                // Broadcast (not `window.emit`): listeners registered through the
                // plain `listen()` helper only see app-wide emits, and every
                // overlay filters on its own monitor id anyway.
                let _ = app.emit(SESSION_EVENT, *id);
            }
            Err(err) => log::error!("创建截图覆盖层失败（显示器 {id}）: {err}"),
        }
    }

    if created == 0 {
        *lock(&state.session) = None;
        return fail(&app, "无法创建截图覆盖层。");
    }

    // One line that says where the hotkey's time actually went.
    let total_ms = started.elapsed().as_millis();
    let window_ms = (total_ms as i64 - capture_ms as i64 - encode_ms as i64).max(0);
    log::info!(
        "截图计时：抓屏 {capture_ms} ms · 编码 {encode_ms} ms · 框选层 {window_ms} ms · 合计 {total_ms} ms（框选层就绪时间见下一行）"
    );

    spawn_delayed(app.clone(), OVERLAY_READY_TIMEOUT, move |app| {
        // Anything still invisible gets shown so the user is never stranded.
        let state = app.state::<AppState>();
        if !state.busy.load(Ordering::SeqCst) {
            return;
        }
        // Only this session's monitors: a warm overlay left over from an
        // earlier capture on another monitor must not reappear with a stale
        // frame on top of the user's screen.
        let ids: Vec<u32> = lock(&state.session)
            .as_ref()
            .map(|session| session.entries.iter().map(|entry| entry.frame.id).collect())
            .unwrap_or_default();
        for id in ids {
            if let Some(window) = app.get_webview_window(&windows::overlay_label(id)) {
                if matches!(window.is_visible(), Ok(false)) {
                    let _ = window.show();
                }
            }
        }
        if let Some(id) = focus_id {
            windows::reveal_overlay(&app, id, true);
        }
    });

    // Safety net: an abandoned selection must never leave the app wedged in
    // "capturing" (overlays up, hotkey dead) until it is restarted.
    spawn_delayed(app.clone(), SESSION_TIMEOUT, move |app| {
        let state = app.state::<AppState>();
        if !state.busy.load(Ordering::SeqCst) {
            return;
        }
        let stale = lock(&state.session)
            .as_ref()
            .map(|session| session.started.elapsed() >= SESSION_TIMEOUT)
            .unwrap_or(false);
        if !stale {
            return;
        }
        log::warn!(
            "框选超过 {} 秒没有确认，自动取消本次截图",
            SESSION_TIMEOUT.as_secs()
        );
        cancel_capture(&app);
    });
}

/// Called by an overlay once its frozen frame is on screen.
pub fn overlay_ready(app: &AppHandle, monitor_id: u32) {
    let (focus, since_freeze_ms) = {
        let state = app.state::<AppState>();
        let session = lock(&state.session);
        let focus = session
            .as_ref()
            .and_then(|s| s.entries.first())
            .map(|entry| entry.frame.id == monitor_id)
            .unwrap_or(false);
        let since = session
            .as_ref()
            .map(|s| s.started.elapsed().as_millis())
            .unwrap_or(0);
        (focus, since)
    };
    // `since_freeze_ms` is the part the user feels beyond the timings logged in
    // `begin_capture`: overlay window + page boot + image decode.
    log::info!("框选层已绘制（显示器 {monitor_id}，focus={focus}，冻结后 {since_freeze_ms} ms）");
    windows::reveal_overlay(app, monitor_id, focus);
}

pub fn cancel_capture(app: &AppHandle) {
    windows::hide_overlays(app);
    *lock(&app.state::<AppState>().session) = None;
    app.state::<AppState>().mark_busy(false);
    emit_stage(app, Stage::Idle, None, 0);
}

/// Selection accepted: crop the frozen frame, hide the overlays, and let the
/// job run in the background.
pub fn finish_selection(app: &AppHandle, rect: SelectionRect) {
    let rect = rect.normalized();
    if !rect.is_usable() {
        log::debug!("选区被忽略（太小）");
        cancel_capture(app);
        return;
    }

    let state = app.state::<AppState>();
    let Some(crop) = state.crop_selection(&rect) else {
        log::error!("选区无法从缓存帧裁切（显示器 {}）", rect.monitor_id);
        cancel_capture(app);
        return;
    };

    log::info!(
        "已确认选区：显示器 {} {}×{} @({},{})",
        rect.monitor_id,
        rect.width,
        rect.height,
        rect.x,
        rect.y
    );

    windows::hide_overlays(app);
    *lock(&state.session) = None;
    state.mark_busy(false);

    // Cache the crop before the job starts, so "重试" works even if the job is
    // cut short by a timeout.
    *lock(&state.last_crop) = Some(crop.clone());

    windows::show_result(app);
    emit_stage(app, Stage::Ocr, Some("已截取区域，正在识别…"), 0);

    spawn_job(app, crop);
}

/// Run OCR + translation in the background, always ending in something the user
/// can see. A timeout is reported like any other failure, and the crop has
/// already been cached, so the retry button can pick up from here.
fn spawn_job(app: &AppHandle, crop: RgbaImage) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if tokio::time::timeout(JOB_TIMEOUT, run_job(app.clone(), crop))
            .await
            .is_err()
        {
            fail(
                &app,
                &format!(
                    "处理超过 {} 秒仍未完成，已停止本次任务。可以点「重试」再试一次。",
                    JOB_TIMEOUT.as_secs()
                ),
            );
        }
    });
}

/* --------------------------------------------------------------------- job */

/// Everything the pipeline learned, so a partial failure can still show the
/// half that worked.
struct Job {
    source: Option<String>,
    translation: Option<String>,
    ocr_provider: String,
    model: String,
    error: Option<String>,
}

async fn run_job(app: AppHandle, crop: RgbaImage) {
    let started = Instant::now();
    let mut cfg = app.state::<AppState>().config();
    let providers = app.state::<AppState>().providers.clone();
    let preview_started = Instant::now();
    let preview = imageops::to_preview_data_url(&crop).ok();
    let preview_ms = preview_started.elapsed().as_millis();
    let (ocr_provider, model) = provider_labels(&cfg);
    log::info!(
        "本次任务：{} · 图像 {}×{}",
        providers::describe_run(&cfg),
        crop.width(),
        crop.height()
    );

    // One-step picture translation: Youdao / Baidu / Tencent read the picture
    // themselves, so the local OCR engine is skipped for this capture.
    if cfg.uses_picture_translate() {
        emit_stage(
            &app,
            Stage::Ocr,
            Some("正在识别并翻译图片…"),
            started.elapsed().as_millis() as u64,
        );
        match picture_translate(&cfg, &providers, &crop).await {
            Ok((source, translation)) => {
                // Record the direction actually used, so the result header can
                // show 中文/English instead of the raw `auto`.
                cfg.translate.target_lang =
                    providers::resolve_target(&source, &cfg.translate.target_lang);
                let job = Job {
                    source: Some(source),
                    translation: Some(translation),
                    ocr_provider: "image".into(),
                    model: String::new(),
                    error: None,
                };
                *lock(&app.state::<AppState>().last_source) = job.source.clone();
                *lock(&app.state::<AppState>().last_crop) = Some(crop);
                publish(&app, &cfg, job, preview, started.elapsed());
                return;
            }
            Err(err) if !cfg.is_ocr_ready() => {
                // Nothing to fall back to: report the service error as-is.
                *lock(&app.state::<AppState>().last_crop) = Some(crop);
                let job = Job {
                    source: None,
                    translation: None,
                    ocr_provider: "image".into(),
                    model: String::new(),
                    error: Some(format!("{err:#}")),
                };
                publish(&app, &cfg, job, preview, started.elapsed());
                return;
            }
            Err(err) => {
                // The one-step service refused the job (service not opened,
                // quota, …) but a local OCR path exists: fall through to it so
                // the capture still produces something useful.
                log::error!("一步图片翻译失败（{err:#}），改用本机识别 + 文本翻译");
                emit_stage(
                    &app,
                    Stage::Ocr,
                    Some("图片翻译失败，改用本机识别…"),
                    started.elapsed().as_millis() as u64,
                );
            }
        }
    }

    let ocr_started = Instant::now();
    let recognized = recognize(&app, &cfg, &providers, &crop).await;
    let ocr_ms = ocr_started.elapsed().as_millis();
    match recognized {
        Ok((source, inline_translation)) => {
            // Smart target: decide from the recognised text before translating.
            cfg.translate.target_lang =
                providers::resolve_target(&source, &cfg.translate.target_lang);
            let mut job = Job {
                source: Some(source.clone()),
                translation: inline_translation,
                ocr_provider,
                model,
                error: None,
            };

            let mut translate_ms = 0u128;
            if job.translation.is_none() && cfg.translate.provider != "none" {
                emit_stage(
                    &app,
                    Stage::Translate,
                    Some("正在翻译…"),
                    started.elapsed().as_millis() as u64,
                );
                let translate_started = Instant::now();
                match providers.translate(&cfg.translate, &source).await {
                    Ok(text) => job.translation = Some(text),
                    Err(err) => job.error = Some(format!("{err:#}")),
                }
                translate_ms = translate_started.elapsed().as_millis();
            }

            log::info!(
                "处理计时：识别 {ocr_ms} ms · 翻译 {translate_ms} ms · 预览 {preview_ms} ms · 合计 {} ms",
                started.elapsed().as_millis()
            );

            let state = app.state::<AppState>();
            *lock(&state.last_source) = Some(source);
            *lock(&state.last_crop) = Some(crop);
            publish(&app, &cfg, job, preview, started.elapsed());
        }
        Err(err) => {
            publish(
                &app,
                &cfg,
                Job {
                    source: None,
                    translation: None,
                    ocr_provider,
                    model,
                    error: Some(format!("{err:#}")),
                },
                preview,
                started.elapsed(),
            );
        }
    }
}

/// One-step picture translation: encode the crop and let the selected cloud
/// engine read it and translate it in a single call.
///
/// A smart target (`auto`) cannot be decided up front — the text only exists
/// inside the picture service — so we start towards Chinese and, when the
/// service reports that the source was Chinese after all, run once more towards
/// English. Only that direction costs a second call.
async fn picture_translate(
    cfg: &AppConfig,
    providers: &Providers,
    crop: &RgbaImage,
) -> Result<(String, String)> {
    let payload = ImagePayload::from_jpeg(encode_picture(crop, cfg.capture.jpeg_quality)?);
    let auto = providers::is_auto_target(&cfg.translate.target_lang);
    let first = call_image_translate(cfg, providers, &payload).await?;
    if auto && matches!(providers::detect_source(&first.0), "zh-CN" | "zh-TW") {
        let mut flipped = cfg.clone();
        flipped.translate.target_lang = "en".to_string();
        if let Ok(second) = call_image_translate(&flipped, providers, &payload).await {
            return Ok(second);
        }
    }
    Ok(first)
}

async fn call_image_translate(
    cfg: &AppConfig,
    providers: &Providers,
    payload: &ImagePayload,
) -> Result<(String, String)> {
    match tokio::time::timeout(
        IMAGE_TRANSLATE_TIMEOUT,
        providers.image_translate(&cfg.translate, payload),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => bail!(
            "图片翻译超过 {} 秒没有响应。请点「重试」，或换一个翻译渠道。",
            IMAGE_TRANSLATE_TIMEOUT.as_secs()
        ),
    }
}

/// JPEG bytes of the crop, shrunk until they fit what the cloud services accept.
fn encode_picture(crop: &RgbaImage, quality: u8) -> Result<Vec<u8>> {
    let mut picture = crop.clone();
    let mut quality = quality.clamp(40, 100);
    let mut bytes = imageops::to_jpeg_bytes(&picture, quality)?;
    for _ in 0..3 {
        if bytes.len() <= MAX_PICTURE_BYTES {
            return Ok(bytes);
        }
        quality = quality.saturating_sub(12).max(45);
        if picture.width().max(picture.height()) > 2000 {
            picture = imageops::scale_to_long_edge(&picture, 2000);
        }
        bytes = imageops::to_jpeg_bytes(&picture, quality)?;
    }
    if bytes.len() > MAX_PICTURE_BYTES {
        bail!(
            "这一块区域太大（约 {} MB），图片翻译接口会拒绝。请框选更小的区域。",
            bytes.len() / 1_048_576
        );
    }
    Ok(bytes)
}

/// OCR, either off the network (vision model) or locally (Windows OCR).
/// Returns the recognised text and, when the vision model was asked to do both
/// jobs in one call, the translation with it.
async fn recognize(
    app: &AppHandle,
    cfg: &AppConfig,
    providers: &Providers,
    crop: &RgbaImage,
) -> Result<(String, Option<String>)> {
    if cfg.ocr.provider == "vision" {
        let prepared = imageops::prepare_for_ocr(crop, &cfg.capture);
        let data_url = imageops::encode_for_ocr(&prepared, &cfg.capture)?;
        let one_shot = cfg.ocr.one_shot && cfg.translate.provider != "none";
        return match tokio::time::timeout(
            VISION_TIMEOUT,
            providers.vision(&cfg.ocr, &data_url, &cfg.translate.target_lang, one_shot),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => bail!(
                "云端识别超过 {} 秒没有响应。请重试；如果一直超时，可以换用「Windows 离线 OCR」（无需联网）。",
                VISION_TIMEOUT.as_secs()
            ),
        };
    }

    // A local HTTP OCR service (Umi-OCR and compatible servers).
    if cfg.ocr.provider == "local" {
        let prepared = imageops::prepare_for_ocr(crop, &cfg.capture);
        let png = imageops::to_png_bytes(&prepared)?;
        return match tokio::time::timeout(OCR_TIMEOUT, providers.local_ocr(&cfg.ocr, &png)).await {
            Ok(Ok(text)) => {
                if text.trim().is_empty() {
                    bail!("没有识别到文字。请尝试框选更清晰、更大的区域。");
                }
                Ok((text, None))
            }
            Ok(Err(err)) => Err(err),
            Err(_) => bail!(
                "本地 OCR 服务超过 {} 秒没有响应。请确认服务（例如 Umi-OCR）正在运行，并检查「本地服务地址」。",
                OCR_TIMEOUT.as_secs()
            ),
        };
    }

    // Offline OCR is CPU-bound and synchronous; keep it off the async pool.
    let capture_cfg = cfg.capture.clone();
    let language = cfg.ocr.language.clone();
    let image = crop.clone();
    let text = match tokio::time::timeout(
        OCR_TIMEOUT,
        tauri::async_runtime::spawn_blocking(move || -> Result<String> {
            let prepared = imageops::prepare_for_ocr(&image, &capture_cfg);
            ocr_win::recognize(&prepared, &language)
        }),
    )
    .await
    {
        Ok(joined) => joined.map_err(|err| anyhow!("OCR 线程异常：{err}"))??,
        Err(_) => bail!(
            "文字识别超过 {} 秒仍未完成。请重试，或框选更小的区域。",
            OCR_TIMEOUT.as_secs()
        ),
    };

    if text.trim().is_empty() {
        bail!("没有识别到文字。请尝试框选更清晰、更大的区域。");
    }
    let _ = app;
    Ok((text, None))
}

/// The engine tag plus the model name that go into a `ResultPayload`.
///
/// The tag is a stable identifier (`vision` / `local` / `winocr`, or `image` for
/// the picture path) that the result window maps to its own localised label —
/// never a pre-formatted sentence, which would break every new language and
/// every label tweak. Details that are worth showing go into the model field,
/// which the window renders as a separate badge.
fn provider_labels(cfg: &AppConfig) -> (String, String) {
    match cfg.ocr.provider.as_str() {
        "vision" => ("vision".to_string(), cfg.ocr.active().model),
        // A one-step picture translation is its own channel: the text came back
        // from the vendor, the local OCR engines never ran.
        "picture" => ("image".to_string(), String::new()),
        "local" => ("local".to_string(), host_of(&cfg.ocr.base_url)),
        _ => ("winocr".to_string(), String::new()),
    }
}

/// Bare host of a base URL, for labels.
fn host_of(base_url: &str) -> String {
    let trimmed = base_url
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    trimmed.split('/').next().unwrap_or(trimmed).to_string()
}

/* ------------------------------------------------------------------ output */

fn publish(app: &AppHandle, cfg: &AppConfig, job: Job, preview: Option<String>, elapsed: Duration) {
    let payload = ResultPayload {
        id: new_id(),
        source_text: job.source.clone().unwrap_or_default(),
        translated_text: job.translation.clone(),
        target_lang: cfg.translate.target_lang.clone(),
        ocr_provider: job.ocr_provider,
        translate_provider: cfg.translate.provider.clone(),
        model: job.model,
        elapsed_ms: elapsed.as_millis() as u64,
        created_at: now_ms(),
        preview,
        error: job.error.clone(),
    };

    let state = app.state::<AppState>();
    if cfg.history.enabled && payload.error.is_none() && !payload.source_text.trim().is_empty() {
        state.push_history((&payload).into());
    }
    *lock(&state.last_result) = Some(payload.clone());

    // Keep a breadcrumb in `snaplingo.log` — the window itself can be hidden or
    // dismissed before the user gets to read it.
    match payload.error.as_deref() {
        Some(err) => {
            log::error!("SnapLingo: 处理失败 — {err}");
            notify(app, "error", err);
        }
        None => log::info!(
            "SnapLingo: 识别 {} 字，翻译 {} 字，耗时 {} ms",
            payload.source_text.chars().count(),
            payload
                .translated_text
                .as_deref()
                .map(|text| text.chars().count())
                .unwrap_or(0),
            payload.elapsed_ms
        ),
    }

    windows::show_result(app);
    if let Err(err) = app.emit(RESULT_EVENT, payload.clone()) {
        log::error!("result 事件发送失败: {err}");
    }
    emit_stage(
        app,
        if payload.error.is_some() {
            Stage::Error
        } else {
            Stage::Done
        },
        payload.error.as_deref(),
        payload.elapsed_ms,
    );
}

/// Surface a failure the user can actually act on: the result window carries the
/// message, the log keeps a breadcrumb, and a notification fires so the error is
/// seen even if no window made it to the screen.
fn fail(app: &AppHandle, message: &str) {
    // Whatever went wrong, get out of the capture state completely: no overlay
    // left on screen, no stale session, hotkey live again.
    windows::hide_overlays(app);
    *lock(&app.state::<AppState>().session) = None;
    app.state::<AppState>().mark_busy(false);
    let payload = ResultPayload::error(new_id(), message.to_string(), 0);
    *lock(&app.state::<AppState>().last_result) = Some(payload.clone());
    windows::show_result(app);
    let _ = app.emit(RESULT_EVENT, payload);
    emit_stage(app, Stage::Error, Some(message), 0);
    log::error!("SnapLingo: {message}");
    notify(app, "error", message);
}

/// Show a branded notice, trimmed to something a small window can display.
///
/// Deliberately *not* a Windows toast: those are only attributed to SnapLingo in
/// an installed build (a dev/portable run shows up as “Windows PowerShell”,
/// because Windows has no registered identity to attach), and we also want the
/// notice to link straight to the log page. A watchdog hides it even if the page
/// never gets to run its own timer.
pub fn notify(app: &AppHandle, kind: &str, message: &str) {
    // The card is opt-out: users who find it noisy can switch it off in the
    // settings window (the failure still lands in the result window and the log).
    if !app.state::<AppState>().config().ui.notify {
        return;
    }

    let kind = kind.to_string();
    let message = message.trim();
    let message = if message.chars().count() > 220 {
        message.chars().take(219).collect::<String>() + "…"
    } else {
        message.to_string()
    };
    let notice = (kind, message);

    windows::show_toast(
        app,
        serde_json::json!({ "kind": &notice.0, "message": &notice.1 }),
    );

    let state = app.state::<AppState>();
    *lock(&state.last_toast) = Some(notice.clone());

    let handle = app.clone();
    spawn_delayed(handle, std::time::Duration::from_secs(9), move |app| {
        let state = app.state::<AppState>();
        if lock(&state.last_toast).as_ref() == Some(&notice) {
            windows::hide_toast(&app);
        }
    });
}

/// Tell the user a newer release exists. Unlike [`notify`], this ignores the
/// "notice card" switch: that setting is about error cards, and an update that
/// nobody is told about is the same as no updater at all.
pub fn notify_update(app: &AppHandle, message: &str) {
    let message = message.trim();
    let message = if message.chars().count() > 220 {
        message.chars().take(219).collect::<String>() + "…"
    } else {
        message.to_string()
    };
    let notice = ("update".to_string(), message);

    windows::show_toast(
        app,
        serde_json::json!({ "kind": &notice.0, "message": &notice.1 }),
    );

    let state = app.state::<AppState>();
    *lock(&state.last_toast) = Some(notice.clone());

    let handle = app.clone();
    spawn_delayed(handle, std::time::Duration::from_secs(16), move |app| {
        let state = app.state::<AppState>();
        if lock(&state.last_toast).as_ref() == Some(&notice) {
            windows::hide_toast(&app);
        }
    });
}

/// Re-run translation on the last recognised text, no re-selection needed.
pub async fn retranslate(app: AppHandle) {
    let (source, crop, preview) = {
        let state = app.state::<AppState>();
        let source = lock(&state.last_source).clone();
        let crop = lock(&state.last_crop).clone();
        let preview = crop
            .as_ref()
            .and_then(|crop| imageops::to_preview_data_url(crop).ok());
        (source, crop, preview)
    };

    // Nothing was recognised last time (or nothing has run yet): replay the
    // whole job from the cached crop, so "重试" fixes OCR failures too and not
    // just translation ones.
    let Some(source) = source else {
        match crop {
            Some(crop) => {
                windows::show_result(&app);
                emit_stage(&app, Stage::Ocr, Some("正在重新识别…"), 0);
                spawn_job(&app, crop);
            }
            None => fail(&app, "还没有可以重试的内容。请先截图一次。"),
        }
        return;
    };

    let cfg = app.state::<AppState>().config();
    let providers = app.state::<AppState>().providers.clone();
    let (ocr_provider, model) = provider_labels(&cfg);
    let started = Instant::now();

    windows::show_result(&app);
    emit_stage(&app, Stage::Translate, Some("正在重新翻译…"), 0);

    let (translation, error) = match tokio::time::timeout(
        TRANSLATE_TIMEOUT,
        providers.translate(&cfg.translate, &source),
    )
    .await
    {
        Ok(Ok(text)) => (Some(text), None),
        Ok(Err(err)) => (None, Some(format!("{err:#}"))),
        Err(_) => (
            None,
            Some(format!(
                "翻译超过 {} 秒没有响应。请重试，或检查网络与接口地址。",
                TRANSLATE_TIMEOUT.as_secs()
            )),
        ),
    };

    publish(
        &app,
        &cfg,
        Job {
            source: Some(source),
            translation,
            ocr_provider,
            model,
            error,
        },
        preview,
        started.elapsed(),
    );
}

/* ------------------------------------------------------------------ helpers */

/// Run `f` on a plain thread after a delay — used for the overlay watchdog,
/// which must work even if the async runtime is busy.
pub fn spawn_delayed<F>(app: AppHandle, delay: Duration, f: F)
where
    F: FnOnce(AppHandle) + Send + 'static,
{
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        f(app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn every_engine_gets_a_stable_tag() {
        let mut cfg = AppConfig::default();

        // The default engine has no model name to show — the tag is enough.
        assert_eq!(provider_labels(&cfg), ("winocr".to_string(), String::new()));

        // One-step picture translation keeps its own tag, otherwise a retry of a
        // picture result would relabel itself as Windows OCR.
        cfg.ocr.provider = "picture".into();
        assert_eq!(provider_labels(&cfg).0, "image");

        cfg.ocr.provider = "local".into();
        cfg.ocr.base_url = "http://127.0.0.1:1224".into();
        assert_eq!(provider_labels(&cfg).0, "local");

        cfg.ocr.provider = "vision".into();
        cfg.ocr.model = "stale-flat-model".into();
        cfg.ocr.profiles.insert(
            "acme".into(),
            crate::config::OcrProfile {
                model: "acme-vision".into(),
                ..Default::default()
            },
        );
        cfg.ocr.vendor = "acme".into();
        // The badge shows the active vendor's model, never the migrated flat field.
        assert_eq!(provider_labels(&cfg), ("vision".to_string(), "acme-vision".into()));
    }
}