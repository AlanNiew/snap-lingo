//! Image preprocessing and encoding.
//!
//! The pipeline is: the capture session keeps a full-resolution RGBA frame per
//! monitor; a selection is cropped out of that frame, optionally rescaled, and
//! encoded either losslessly (PNG, best for OCR) or as a small JPEG preview.

use crate::config::CaptureConfig;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

/// Vision APIs get billed per pixel and reject oversized payloads.
const OCR_TARGET_LONG_EDGE: u32 = 1400;
const OCR_MAX_UPSCALE: f32 = 3.0;
/// Below this the upscale is not worth the tokens.
const OCR_MIN_UPSCALE: f32 = 1.2;
/// PNG larger than this falls back to JPEG.
const PNG_SOFT_LIMIT: usize = 3_500_000;

/// Crop `rect` (in monitor-local physical pixels) out of `frame`.
pub fn crop(frame: &RgbaImage, x: i32, y: i32, w: i32, h: i32) -> RgbaImage {
    let (fw, fh) = frame.dimensions();
    let x0 = x.clamp(0, fw as i32) as u32;
    let y0 = y.clamp(0, fh as i32) as u32;
    let x1 = (x + w).clamp(0, fw as i32) as u32;
    let y1 = (y + h).clamp(0, fh as i32) as u32;
    let cw = x1.saturating_sub(x0).max(1);
    let ch = y1.saturating_sub(y0).max(1);
    image::imageops::crop_imm(frame, x0, y0, cw, ch).to_image()
}

/// Scale the crop so the vision model sees readable text without wasting tokens.
pub fn prepare_for_ocr(img: &RgbaImage, cfg: &CaptureConfig) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return img.clone();
    }
    let long = w.max(h) as f32;

    let factor = if cfg.upscale_small && long < OCR_TARGET_LONG_EDGE as f32 {
        (OCR_TARGET_LONG_EDGE as f32 / long).min(OCR_MAX_UPSCALE)
    } else if long > cfg.max_image_side as f32 {
        cfg.max_image_side as f32 / long
    } else {
        1.0
    };

    if (factor - 1.0).abs() < 0.01 || (factor > 1.0 && factor < OCR_MIN_UPSCALE) {
        return img.clone();
    }

    // Round to integers >= 1px, and never exceed 4x the source pixel budget.
    let nw = ((w as f32 * factor).round() as u32).max(1);
    let nh = ((h as f32 * factor).round() as u32).max(1);
    let filter = if factor > 1.0 { FilterType::Lanczos3 } else { FilterType::CatmullRom };
    image::imageops::resize(img, nw, nh, filter)
}

/// PNG data URL. Lossless, so thin glyphs survive; the OCR path prefers this.
pub fn to_png_data_url(img: &RgbaImage) -> anyhow::Result<String> {
    let (w, h) = img.dimensions();
    let mut buf = Vec::with_capacity((w * h) as usize / 2 + 1024);
    PngEncoder::new(&mut buf).write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8)?;
    Ok(format!("data:image/png;base64,{}", B64.encode(&buf)))
}

/// JPEG data URL, for previews and for pathological PNG sizes.
pub fn to_jpeg_data_url(img: &RgbaImage, quality: u8) -> anyhow::Result<String> {
    let (w, h) = img.dimensions();
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut buf = Vec::with_capacity((w * h) as usize / 4 + 1024);
    JpegEncoder::new_with_quality(&mut buf, quality)
        .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)?;
    Ok(format!("data:image/jpeg;base64,{}", B64.encode(&buf)))
}

/// Encoder chosen for the OCR request: PNG when it stays reasonable, else JPEG.
pub fn encode_for_ocr(img: &RgbaImage, cfg: &CaptureConfig) -> anyhow::Result<String> {
    let png = to_png_data_url(img)?;
    if png.len() <= PNG_SOFT_LIMIT {
        return Ok(png);
    }
    log::debug!("png payload is {} bytes; falling back to jpeg", png.len());
    to_jpeg_data_url(img, cfg.jpeg_quality)
}

/// Downscale so the long edge fits `max`, never upscaling.
pub fn scale_to_long_edge(img: &RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= max || long == 0 {
        return img.clone();
    }
    let f = max as f32 / long as f32;
    image::imageops::resize(
        img,
        ((w as f32 * f).round() as u32).max(1),
        ((h as f32 * f).round() as u32).max(1),
        FilterType::Triangle,
    )
}

/// Tiny inline thumbnail shown in the result window and history list.
pub fn to_preview_data_url(img: &RgbaImage) -> anyhow::Result<String> {
    const PREVIEW_LONG_EDGE: u32 = 260;
    let (w, h) = img.dimensions();
    let long = w.max(h);
    let small = if long > PREVIEW_LONG_EDGE {
        let f = PREVIEW_LONG_EDGE as f32 / long as f32;
        image::imageops::resize(
            img,
            ((w as f32 * f).round() as u32).max(1),
            ((h as f32 * f).round() as u32).max(1),
            FilterType::Triangle,
        )
    } else {
        img.clone()
    };
    to_jpeg_data_url(&small, 72)
}

/// Flatten RGBA onto an opaque background. Needed by the Windows OCR engine,
/// which ignores alpha and would otherwise read transparent pixels as black.
pub fn flattened_bgra(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in img.pixels() {
        let [r, g, b, a] = px.0;
        if a == 255 {
            out.extend_from_slice(&[b, g, r, 255]);
        } else {
            let af = a as u32;
            let blend = |c: u8| (((c as u32 * af) + (255 * (255 - af))) / 255) as u8;
            out.extend_from_slice(&[blend(b), blend(g), blend(r), 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn checker(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            if (x / 4 + y / 4) % 2 == 0 {
                Rgba([250, 250, 250, 255])
            } else {
                Rgba([10, 10, 10, 255])
            }
        })
    }

    #[test]
    fn crop_clamps_to_frame_bounds() {
        let frame = checker(100, 50);
        // Overhanging the right/bottom edge must clamp, not panic.
        let c = crop(&frame, 80, 40, 100, 100);
        assert_eq!(c.dimensions(), (20, 10));
        let c = crop(&frame, -20, -20, 30, 30);
        assert_eq!(c.dimensions(), (10, 10));
    }

    #[test]
    fn prepare_upscales_small_and_clamps_large() {
        let cfg = CaptureConfig::default();
        let small = prepare_for_ocr(&checker(120, 40), &cfg);
        assert!(small.width() > 120, "small crop should be upscaled");

        let mut big_cfg = cfg.clone();
        big_cfg.max_image_side = 500;
        big_cfg.upscale_small = false;
        let big = prepare_for_ocr(&checker(2000, 1000), &big_cfg);
        assert_eq!(big.width().max(big.height()), 500);
    }

    #[test]
    fn encoders_produce_valid_data_urls() {
        let img = checker(64, 32);
        assert!(to_png_data_url(&img).unwrap().starts_with("data:image/png;base64,"));
        assert!(to_jpeg_data_url(&img, 80).unwrap().starts_with("data:image/jpeg;base64,"));
        assert!(to_preview_data_url(&img).unwrap().starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn flatten_drops_alpha() {
        let img = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 0]));
        let bytes = flattened_bgra(&img);
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[0..3], &[255, 255, 255], "transparent → white");
    }
}
