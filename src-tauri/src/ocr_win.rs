//! Offline OCR through the Windows Runtime `Windows.Media.Ocr` engine.
//!
//! Ships with the OS, costs nothing, needs no key and no network — which makes
//! it the default OCR path and the one that keeps the promise of "lightweight
//! and resident". Quality trails a multimodal LLM on dense or stylised text, so
//! the vision provider stays available as an upgrade.
//!
//! The engine's only input format is a BGRA8 `SoftwareBitmap`, and it enforces a
//! minimum edge length; both are handled here.

use crate::imageops;
use anyhow::{anyhow, Context, Result};
use image::{Rgba, RgbaImage};
use std::borrow::Cow;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

/// The recognizer rejects images with an edge shorter than this.
const MIN_EDGE: u32 = 40;
/// Conservative cap; `MaxImageDimension()` is queried at runtime too.
const FALLBACK_MAX_EDGE: u32 = 8000;

/// Recognised languages installed on this machine, as BCP-47 tags.
pub fn available_languages() -> Vec<String> {
    let _ = init_apartment();
    match OcrEngine::AvailableRecognizerLanguages() {
        Ok(list) => {
            let size = list.Size().unwrap_or(0);
            (0..size)
                .filter_map(|i| list.GetAt(i).ok())
                .filter_map(|l| l.LanguageTag().ok())
                .map(|t| t.to_string_lossy())
                .collect()
        }
        Err(err) => {
            log::warn!("could not enumerate Windows OCR languages: {err}");
            Vec::new()
        }
    }
}

/// Human-readable name for a language tag, for the settings dropdown.
pub fn describe_language(tag: &str) -> Option<String> {
    let _ = init_apartment();
    let lang = Language::CreateLanguage(&HSTRING::from(tag)).ok()?;
    lang.DisplayName().ok().map(|s| s.to_string_lossy())
}

/// Run OCR over `img`. `language` is a BCP-47 tag; empty means "whatever the
/// user profile prefers".
pub fn recognize(img: &RgbaImage, language: &str) -> Result<String> {
    // WinRT requires an initialized apartment on the calling thread.
    init_apartment();

    let engine = create_engine(language)?;
    let bitmap = to_software_bitmap(img)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .context("Windows OCR: RecognizeAsync failed")?
        .join()
        .context("Windows OCR: recognition did not complete")?;

    // `Text()` preserves line breaks; it is exactly what we want to translate.
    let text = result.Text().context("Windows OCR: reading result failed")?.to_string_lossy();
    Ok(tighten_spaces(text.trim()))
}

/// Windows OCR emits one token per space, which for CJK text means a space
/// between every single character ("神 威 无 敌") and occasionally inside a
/// number ("35 0"). Both look broken in the result window and confuse a
/// translation model, so glue them back together.
fn tighten_spaces(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());

    for (i, &c) in chars.iter().enumerate() {
        if !is_spacer(c) {
            out.push(c);
            continue;
        }
        let prev = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
        let next = chars.get(i + 1).copied();
        let glue = match (prev, next) {
            (Some(p), Some(n)) => {
                (is_cjk(p) && is_cjk(n))
                    || is_cjk_punct(p)
                    || is_cjk_punct(n)
                    || (p.is_ascii_digit() && n.is_ascii_digit())
            }
            _ => false,
        };
        if !glue {
            out.push(c);
        }
    }

    out
}

fn is_spacer(c: char) -> bool {
    c == ' ' || c == '\u{3000}'
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF      // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF  // CJK Unified Ideographs
            | 0xF900..=0xFAFF  // CJK Compatibility Ideographs
            | 0x3040..=0x30FF  // Hiragana + Katakana
            | 0xAC00..=0xD7AF  // Hangul syllables
            | 0x20000..=0x2FA1F // Extensions B..F
    )
}

fn is_cjk_punct(c: char) -> bool {
    matches!(
        c as u32,
        0x3000..=0x303F      // CJK symbols and punctuation
            | 0xFF01..=0xFF0F  // Fullwidth ASCII punctuation
            | 0xFF1A..=0xFF20
            | 0xFF3B..=0xFF40
            | 0xFF5B..=0xFF65
    )
}

fn init_apartment() {
    // SAFETY: WinRT requires an apartment; MTA is correct for background work.
    // Already-initialized threads return S_FALSE (not an error) or
    // RPC_E_CHANGED_MODE, which is harmless for our usage.
    match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        Ok(()) => {}
        Err(err) => log::debug!("RoInitialize: {err} (continuing)"),
    }
}

fn create_engine(language: &str) -> Result<OcrEngine> {
    if !language.trim().is_empty() {
        let tag = HSTRING::from(language.trim());
        match Language::CreateLanguage(&tag).and_then(|l| OcrEngine::TryCreateFromLanguage(&l)) {
            Ok(engine) => return Ok(engine),
            Err(err) => {
                log::warn!("Windows OCR has no recognizer for '{language}': {err}");
            }
        }
    }

    OcrEngine::TryCreateFromUserProfileLanguages().map_err(|err| {
        let langs = available_languages();
        if langs.is_empty() {
            anyhow!(
                "Windows 未安装任何 OCR 语言包（{err}）。请在「设置 → 时间和语言 → 语言」中添加语言并安装「光学字符识别」可选功能。"
            )
        } else {
            anyhow!(
                "无法创建 Windows OCR 引擎（{err}）。可用语言：{}。请在设置中指定其中一种。",
                langs.join(", ")
            )
        }
    })
}

/// Fit the image into the engine's accepted size window and hand back a
/// BGRA8 premultiplied-less `SoftwareBitmap`.
fn to_software_bitmap(img: &RgbaImage) -> Result<SoftwareBitmap> {
    let fitted = fit_to_limits(img)?;
    let (w, h) = fitted.dimensions();
    let bgra = imageops::flattened_bgra(&fitted);

    let writer = DataWriter::new().context("Windows OCR: DataWriter::new failed")?;
    writer
        .WriteBytes(&bgra)
        .context("Windows OCR: writing pixels failed")?;
    let buffer = writer
        .DetachBuffer()
        .context("Windows OCR: detach_buffer failed")?;

    SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w as i32, h as i32)
        .context("Windows OCR: creating SoftwareBitmap failed")
}

/// Downscale when too large, pad with white when too small — never distort.
///
/// Returns a borrowed frame in the common case (a selection already inside the
/// engine's limits): the previous version cloned it, which cost another
/// full-resolution RGBA copy before the BGRA conversion copied it again.
fn fit_to_limits(img: &RgbaImage) -> Result<Cow<'_, RgbaImage>> {
    let max_edge = OcrEngine::MaxImageDimension().unwrap_or(FALLBACK_MAX_EDGE).min(16_384);
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(anyhow!("捕捉到的区域是空的，请重新框选。"));
    }

    let long = w.max(h);
    let mut out: Cow<'_, RgbaImage> = if long > max_edge {
        let f = max_edge as f32 / long as f32;
        Cow::Owned(image::imageops::resize(
            img,
            ((w as f32 * f).round() as u32).max(1),
            ((h as f32 * f).round() as u32).max(1),
            image::imageops::FilterType::CatmullRom,
        ))
    } else {
        Cow::Borrowed(img)
    };

    let (w, h) = out.dimensions();
    if w < MIN_EDGE || h < MIN_EDGE {
        let pad_w = w.max(MIN_EDGE);
        let pad_h = h.max(MIN_EDGE);
        let mut padded = RgbaImage::from_pixel(pad_w, pad_h, Rgba([255, 255, 255, 255]));
        image::imageops::overlay(&mut padded, &*out, 0, 0);
        out = Cow::Owned(padded);
    }
    Ok(out)
}

/// Diagnostic for the settings window: which languages can we actually use?
pub fn probe(language: &str) -> Result<()> {
    init_apartment();
    create_engine(language).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_images_get_padded_not_scaled() {
        let img = RgbaImage::from_pixel(30, 12, Rgba([0, 0, 255, 255]));
        let out = fit_to_limits(&img).unwrap();
        assert_eq!(out.dimensions(), (40, 40), "padded to the minimum edge");
        // Original pixels survive at the origin, padding is white.
        assert_eq!(out.get_pixel(5, 5).0, [0, 0, 255, 255]);
        assert_eq!(out.get_pixel(35, 35).0, [255, 255, 255, 255]);
    }

    #[test]
    fn oversized_images_are_downscaled() {
        let img = RgbaImage::from_pixel(20_000, 100, Rgba([1, 2, 3, 255]));
        let out = fit_to_limits(&img).unwrap();
        assert!(out.width() <= 16_384);
    }

    #[test]
    fn empty_image_is_an_error() {
        let img = RgbaImage::new(0, 0);
        assert!(fit_to_limits(&img).is_err());
    }

    #[test]
    fn cjk_spaces_are_glued_back_together() {
        assert_eq!(tighten_spaces("神 威 无 敌"), "神威无敌");
        assert_eq!(tighten_spaces("小 时 前 ： 35 0 UP 主"), "小时前：350 UP 主");
        assert_eq!(tighten_spaces("拉 赫 玛 尼 诺 夫 7"), "拉赫玛尼诺夫 7");
    }

    #[test]
    fn latin_spacing_is_left_alone() {
        assert_eq!(tighten_spaces("hello world"), "hello world");
        // A space between CJK and Latin is legitimate: keep it.
        assert_eq!(tighten_spaces("使用 Windows 系统"), "使用 Windows 系统");
        assert_eq!(tighten_spaces("line one\nline two"), "line one\nline two");
    }
}
