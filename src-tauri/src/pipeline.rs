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

use crate::capture;
use crate::config::AppConfig;
use crate::imageops;
use crate::model::{new_id, now_ms, ResultPayload, SelectionRect, Stage, StageEvent};
use crate::ocr_win;
use crate::providers::Providers;
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

/// If a webview has not reported that it painted within this window, reveal the
/// overlay anyway. Without it a failed page load would leave the app stuck in
/// "capturing" forever.
const OVERLAY_READY_TIMEOUT: Duration = Duration::from_millis(1400);

fn emit_stage(app: &AppHandle, stage: Stage, message: Option<&str>, elapsed_ms: u64) {
    let event = StageEvent {
        stage,
        message: message.map(|m| m.to_string()),
        elapsed_ms,
    };
    if let Err(err) = app.emit(STAGE_EVENT, event) {
        log::warn!("stage 事件发送失败: {err}");
    }
}

/* ------------------------------------------------------------------ capture */

/// Hotkey / tray / button entry point. Freezes every screen, then hands control
/// to the overlays.
pub async fn begin_capture(app: AppHandle) {
    let state = app.state::<AppState>();

    if state.busy.swap(true, Ordering::SeqCst) {
        // Already selecting: just bring the overlays back to the front.
        for label in windows::overlay_labels(&app) {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.set_focus();
            }
        }
        return;
    }

    let cfg = state.config();
    if !cfg.is_ready() {
        fail(
            &app,
            "还没有配置可用的 OCR / 翻译服务。请在设置中填写接口地址、密钥和模型，或把 OCR 切换为「Windows 离线 OCR」。",
        );
        return;
    }

    emit_stage(&app, Stage::Capturing, Some("正在截取屏幕…"), 0);

    // Our own windows must never appear inside the shot.
    windows::hide_transient(&app);
    // Let the compositor actually remove them before reading the screen.
    std::thread::sleep(Duration::from_millis(80));

    let all_monitors = cfg.capture.all_monitors;
    let preview_edge = windows::PREVIEW_LONG_EDGE;
    let prepared = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<SessionEntry>> {
        let frames = capture::capture(all_monitors)?;
        let mut entries = Vec::with_capacity(frames.len());
        for frame in frames {
            // Encoded once, here, so opening an overlay is instant.
            let preview = imageops::scale_to_long_edge(&frame.frame, preview_edge);
            let preview_data_url = imageops::to_jpeg_data_url(&preview, 82)?;
            entries.push(SessionEntry {
                frame,
                preview,
                preview_data_url,
            });
        }
        Ok(entries)
    })
    .await;

    let entries = match prepared {
        Ok(Ok(entries)) => entries,
        Ok(Err(err)) => return fail(&app, &format!("{err:#}")),
        Err(err) => return fail(&app, &format!("截图线程异常：{err}")),
    };

    let ids: Vec<u32> = entries.iter().map(|entry| entry.frame.id).collect();
    let primary = ids.first().copied();
    *lock(&state.session) = Some(CaptureSession {
        entries,
        started: Instant::now(),
    });

    let mut created = 0usize;
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
            Ok(_) => created += 1,
            Err(err) => log::warn!("创建截图覆盖层失败（显示器 {id}）: {err}"),
        }
    }

    if created == 0 {
        *lock(&state.session) = None;
        return fail(&app, "无法创建截图覆盖层。");
    }

    spawn_delayed(app.clone(), OVERLAY_READY_TIMEOUT, move |app| {
        // Anything still invisible gets shown so the user is never stranded.
        let state = app.state::<AppState>();
        if !state.busy.load(Ordering::SeqCst) {
            return;
        }
        for label in windows::overlay_labels(&app) {
            if let Some(window) = app.get_webview_window(&label) {
                if matches!(window.is_visible(), Ok(false)) {
                    let _ = window.show();
                }
            }
        }
        if let Some(id) = primary {
            windows::reveal_overlay(&app, id, true);
        }
    });
}

/// Called by an overlay once its frozen frame is on screen.
pub fn overlay_ready(app: &AppHandle, monitor_id: u32) {
    let focus = {
        let state = app.state::<AppState>();
        let session = lock(&state.session);
        session
            .as_ref()
            .and_then(|s| s.entries.first())
            .map(|entry| entry.frame.id == monitor_id)
            .unwrap_or(false)
    };
    windows::reveal_overlay(app, monitor_id, focus);
}

pub fn cancel_capture(app: &AppHandle) {
    windows::close_overlays(app);
    *lock(&app.state::<AppState>().session) = None;
    app.state::<AppState>().mark_busy(false);
    emit_stage(app, Stage::Idle, None, 0);
}

/// Selection accepted: crop the frozen frame, drop the overlays, and let the
/// job run in the background.
pub fn finish_selection(app: &AppHandle, rect: SelectionRect) {
    let rect = rect.normalized();
    if !rect.is_usable() {
        cancel_capture(app);
        return;
    }

    let state = app.state::<AppState>();
    let Some(crop) = state.crop_selection(&rect) else {
        cancel_capture(app);
        return;
    };

    windows::close_overlays(app);
    *lock(&state.session) = None;
    state.mark_busy(false);

    windows::show_result(app);
    emit_stage(app, Stage::Ocr, Some("已截取区域，正在识别…"), 0);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_job(app, crop).await;
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
    let cfg = app.state::<AppState>().config();
    let providers = app.state::<AppState>().providers.clone();
    let preview = imageops::to_preview_data_url(&crop).ok();

    let (ocr_provider, model) = provider_labels(&cfg);

    match recognize(&app, &cfg, &providers, &crop).await {
        Ok((source, inline_translation)) => {
            let mut job = Job {
                source: Some(source.clone()),
                translation: inline_translation,
                ocr_provider,
                model,
                error: None,
            };

            if job.translation.is_none() && cfg.translate.provider != "none" {
                emit_stage(
                    &app,
                    Stage::Translate,
                    Some("正在翻译…"),
                    started.elapsed().as_millis() as u64,
                );
                match providers.translate(&cfg.translate, &source).await {
                    Ok(text) => job.translation = Some(text),
                    Err(err) => job.error = Some(format!("{err:#}")),
                }
            }

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
        return providers
            .vision(&cfg.ocr, &data_url, &cfg.translate.target_lang, one_shot)
            .await;
    }

    // Offline OCR is CPU-bound and synchronous; keep it off the async pool.
    let capture_cfg = cfg.capture.clone();
    let language = cfg.ocr.language.clone();
    let image = crop.clone();
    let text = tauri::async_runtime::spawn_blocking(move || -> Result<String> {
        let prepared = imageops::prepare_for_ocr(&image, &capture_cfg);
        ocr_win::recognize(&prepared, &language)
    })
    .await
    .map_err(|err| anyhow!("OCR 线程异常：{err}"))??;

    if text.trim().is_empty() {
        bail!("没有识别到文字。请尝试框选更清晰、更大的区域。");
    }
    let _ = app;
    Ok((text, None))
}

fn provider_labels(cfg: &AppConfig) -> (String, String) {
    if cfg.ocr.provider == "vision" {
        (format!("vision:{}", short(&cfg.ocr.model)), cfg.ocr.model.clone())
    } else {
        let language = if cfg.ocr.language.trim().is_empty() {
            "自动".to_string()
        } else {
            cfg.ocr.language.clone()
        };
        (format!("winocr:{language}"), format!("Windows OCR ({language})"))
    }
}

fn short(model: &str) -> String {
    model.rsplit('/').next().unwrap_or(model).to_string()
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
        Some(err) => log::warn!("SnapLingo: 处理失败 — {err}"),
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
        log::warn!("result 事件发送失败: {err}");
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

/// Surface a failure the user can actually act on.
fn fail(app: &AppHandle, message: &str) {
    app.state::<AppState>().mark_busy(false);
    let payload = ResultPayload::error(new_id(), message.to_string(), 0);
    *lock(&app.state::<AppState>().last_result) = Some(payload.clone());
    windows::show_result(app);
    let _ = app.emit(RESULT_EVENT, payload);
    emit_stage(app, Stage::Error, Some(message), 0);
    log::warn!("SnapLingo: {message}");
}

/// Re-run translation on the last recognised text, no re-selection needed.
pub async fn retranslate(app: AppHandle) {
    let (source, preview) = {
        let state = app.state::<AppState>();
        let source = lock(&state.last_source).clone();
        let preview = lock(&state.last_crop)
            .as_ref()
            .and_then(|crop| imageops::to_preview_data_url(crop).ok());
        (source, preview)
    };

    let Some(source) = source else {
        return;
    };

    let cfg = app.state::<AppState>().config();
    let providers = app.state::<AppState>().providers.clone();
    let (ocr_provider, model) = provider_labels(&cfg);
    let started = Instant::now();

    windows::show_result(&app);
    emit_stage(&app, Stage::Translate, Some("正在重新翻译…"), 0);

    let (translation, error) = match providers.translate(&cfg.translate, &source).await {
        Ok(text) => (Some(text), None),
        Err(err) => (None, Some(format!("{err:#}"))),
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
