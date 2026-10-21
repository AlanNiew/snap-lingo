//! Model / API access: OpenAI-compatible chat completions, DeepL, the cloud
//! translation services (Microsoft / Google / Youdao / Baidu / Tencent) and the
//! one-step picture translation of Youdao, Baidu and Tencent.
//!
//! Everything that talks to the network lives here so the pipeline stays a pure
//! orchestration layer. Requests carry an explicit timeout, retry on the
//! retryable class of failures (429 / 5xx / transport), and strip model
//! "thinking" leakage from the reply.

use crate::config::{AppConfig, OcrConfig, TranslateConfig, DEFAULT_OPENAI_BASE_URL};
use crate::model::ConnectionCheck;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use image::RgbaImage;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
/// One initial try plus three retries — the number the docs promise. The
/// pipeline's own deadlines (120 s for translation, 150 s for the whole job)
/// still bound the call, so a slow engine only retries as far as the user is
/// willing to wait.
const MAX_ATTEMPTS: u32 = 4;
/// Translate in slices of roughly this many characters.
const CHUNK_CHARS: usize = 2400;

/// An encoded picture on its way to a cloud translator.
pub struct ImagePayload {
    /// Raw JPEG bytes — Baidu's signature is computed over the file itself.
    pub bytes: Vec<u8>,
    /// The same bytes as base64, without any `data:` prefix.
    pub base64: String,
}

impl ImagePayload {
    pub fn from_jpeg(bytes: Vec<u8>) -> Self {
        let base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Self { bytes, base64 }
    }
}

/* -------------------------------------------------- credential scoping */

/// Fold the active vision vendor's profile into the flat credential fields the
/// engine reads, so a request can never pick up another vendor's key or model.
fn scoped_ocr(cfg: &OcrConfig) -> OcrConfig {
    let picked = cfg.active();
    let mut scoped = cfg.clone();
    scoped.base_url = picked.base_url;
    scoped.api_key = picked.api_key;
    scoped.model = picked.model;
    scoped
}

/// Same idea for the translation engines: the selected engine's own address,
/// keys, region, model and tier replace whatever the flat fields happen to hold.
fn scoped_translate(cfg: &TranslateConfig) -> TranslateConfig {
    let profile = cfg.active();
    let tier = profile.tier(&cfg.provider);
    let mut scoped = cfg.clone();
    scoped.base_url = profile.base_url;
    scoped.api_key = profile.api_key;
    scoped.api_secret = profile.api_secret;
    scoped.region = profile.region;
    scoped.model = profile.model;
    scoped.model_tier = tier;
    scoped.auth = profile.auth;
    scoped.bearer = profile.bearer;
    scoped
}

#[derive(Clone)]
pub struct Providers {
    client: reqwest::Client,
}

impl Providers {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("SnapLingo/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("could not build the HTTP client")?;
        Ok(Self { client })
    }

    /* ------------------------------------------------------------- OCR */

    /// Vision OCR. When `one_shot` is set the model also translates, saving a
    /// whole round trip. Returns `(source_text, translation)`.
    pub async fn vision(
        &self,
        cfg: &OcrConfig,
        image_data_url: &str,
        target_lang: &str,
        one_shot: bool,
    ) -> Result<(String, Option<String>)> {
        // Vision credentials live per vendor; fold the active one into the flat
        // fields so the request always uses its own set.
        let scoped = scoped_ocr(cfg);
        let cfg = &scoped;

        require_key(&cfg.api_key, "OCR / 视觉模型")?;
        if cfg.model.trim().is_empty() {
            bail!("尚未配置 OCR 模型名称。");
        }
        if let Some(url) = ocr_endpoint(cfg) {
            trace_request(
                "识别 · 视觉大模型",
                "POST",
                &url,
                &[
                    ("模型", cfg.model.clone()),
                    ("目标语言", if one_shot { target_lang.to_string() } else { String::new() }),
                    ("模式", if one_shot { "一次调用（原文+译文）".to_string() } else { "只识别".to_string() }),
                    ("图片", format!("{} KB", image_data_url.len() / 1024 / 4 * 3)),
                    ("密钥", mask_secret(&cfg.api_key)),
                ],
            );
        }

        let (system, user_text, json_mode) = if one_shot {
            (
                one_shot_system(target_lang),
                "请识别这张图片并翻译。".to_string(),
                true,
            )
        } else {
            (ocr_system(&cfg.prompt), "请识别这张图片中的文字。".to_string(), false)
        };

        let messages = vec![
            json!({ "role": "system", "content": system }),
            json!({
                "role": "user",
                "content": [
                    { "type": "text", "text": user_text },
                    { "type": "image_url", "image_url": { "url": image_data_url, "detail": "high" } }
                ]
            }),
        ];

        let raw = self
            .chat(&cfg.base_url, &cfg.api_key, &cfg.model, messages, 4096, json_mode)
            .await?;

        if one_shot {
            let reply = parse_one_shot(&raw);
            let source = reply.source.trim().to_string();
            let translation = reply.translation.trim().to_string();
            if source.is_empty() && translation.is_empty() {
                bail!("视觉模型没有返回可用内容。");
            }
            Ok((source, if translation.is_empty() { None } else { Some(translation) }))
        } else {
            let text = clean_model_text(&raw);
            if text.is_empty() {
                bail!("没有识别到文字。请尝试框选更清晰、更大的区域。");
            }
            Ok((text, None))
        }
    }

    /* ------------------------------------------------------- translate */

    pub async fn translate(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        // The engines below read the flat credential fields; scope them to the
        // selected engine first.
        let mut scoped = scoped_translate(cfg);
        // `auto` is resolved against the text itself, so the "test connection"
        // button (and any other caller) never hands a literal `auto` to an
        // engine that would reject it.
        if is_auto_target(&scoped.target_lang) {
            scoped.target_lang = resolve_target(text, &scoped.target_lang);
        }
        let cfg = &scoped;
        if let Some(url) = text_endpoint(&cfg.provider, cfg) {
            let params: Vec<(&str, String)> = vec![
                ("目标语言", cfg.target_lang.clone()),
                ("文本", format!("{} 字", text.chars().count())),
                ("模型", cfg.model.clone()),
                ("档位", cfg.model_tier.clone()),
                ("区域", cfg.region.clone()),
                ("密钥", mask_secret(&cfg.api_key)),
                ("应用密钥", mask_secret(&cfg.api_secret)),
            ];
            trace_request(
                &format!("翻译 · {}", provider_label(&cfg.provider)),
                "POST",
                &url,
                &params,
            );
        }
        match cfg.provider.as_str() {
            "none" => Ok(String::new()),
            "deepl" => self.deepl(cfg, text).await,
            "microsoft" => self.microsoft(cfg, text).await,
            "google" => self.google(cfg, text).await,
            "youdao" => self.youdao(cfg, text).await,
            "baidu" => self.baidu(cfg, text).await,
            "tencent" => self.tencent(cfg, text).await,
            _ => self.translate_chat(cfg, text).await,
        }
    }

    /// One-step picture translation: the cloud service reads the picture itself
    /// and hands back both the text it saw and the translation, so the local OCR
    /// engine is bypassed for this capture.
    ///
    /// Returns `(source_text, translated_text)`.
    pub async fn image_translate(
        &self,
        cfg: &TranslateConfig,
        image: &ImagePayload,
    ) -> Result<(String, String)> {
        let scoped = scoped_translate(cfg);
        let cfg = &scoped;
        if let Some(url) = image_endpoint(&cfg.provider, cfg) {
            let params: Vec<(&str, String)> = vec![
                ("目标语言", cfg.target_lang.clone()),
                ("图片", format!("{} KB", image.bytes.len() / 1024)),
                ("档位", cfg.model_tier.clone()),
                ("区域", cfg.region.clone()),
                ("密钥", mask_secret(&cfg.api_key)),
                ("应用密钥", mask_secret(&cfg.api_secret)),
            ];
            trace_request(
                &format!("一步图片翻译 · {}", provider_label(&cfg.provider)),
                "POST",
                &url,
                &params,
            );
        }
        let (source, translation) = match cfg.provider.as_str() {
            "youdao" => self.youdao_image(cfg, image).await?,
            "baidu" => self.baidu_image(cfg, image).await?,
            "tencent" => self.tencent_image(cfg, image).await?,
            other => bail!("{other} 不支持一步翻译图片，请改用「先识别、再翻译」模式。"),
        };
        if translation.trim().is_empty() {
            bail!("图片里没有识别到可以翻译的文字。");
        }
        Ok((source, translation))
    }

    /// Offline OCR through a local HTTP service (Umi-OCR and compatible
    /// servers). The service runs on the user's own machine, so the picture
    /// never leaves it.
    pub async fn local_ocr(&self, cfg: &OcrConfig, png: &[u8]) -> Result<String> {
        let url = local_ocr_url(cfg)?;
        trace_request(
            "识别 · 本地 OCR 服务",
            "POST",
            &url,
            &[
                ("图片", format!("{} KB", png.len() / 1024)),
                ("选项", "data.format=text · tbpu.parser=multi_para".to_string()),
            ],
        );
        let body = json!({
            "base64": base64::engine::general_purpose::STANDARD.encode(png),
            "options": {
                "data.format": "text",
                "tbpu.parser": "multi_para",
            }
        });
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|err| anyhow!("连接本地 OCR 服务失败：{}", classify_transport(&err)))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        log::info!(
            "识别 · 本地 OCR 服务 响应 {} · {} 字节",
            status.as_u16(),
            text.len()
        );
        if !status.is_success() {
            bail!(
                "本地 OCR 服务返回 HTTP {}。请确认服务已经启动、地址端口正确。",
                status.as_u16()
            );
        }
        parse_local_ocr(&text)
    }

    /// Round-trip check for the local OCR service. A blank image is expected to
    /// come back as "no text" (Umi-OCR answers code 101), which still proves the
    /// service, the address and the image pipeline all work.
    pub async fn local_ocr_probe(&self, cfg: &OcrConfig) -> Result<()> {
        let blank = RgbaImage::from_pixel(72, 24, image::Rgba([255, 255, 255, 255]));
        let png = crate::imageops::to_png_bytes(&blank)?;
        self.local_ocr(cfg, &png).await.map(|_| ())
    }

    /// Same probe, in the shape the settings page shows.
    pub async fn local_ocr_check(&self, cfg: &OcrConfig) -> ConnectionCheck {
        let started = Instant::now();
        let outcome = self.local_ocr_probe(cfg).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(()) => ConnectionCheck {
                ok: true,
                message: format!("本地 OCR 服务可用（{}）", host_of(&cfg.base_url)),
                elapsed_ms,
                sample: None,
            },
            Err(err) => ConnectionCheck {
                ok: false,
                message: format!("{err:#}"),
                elapsed_ms,
                sample: None,
            },
        }
    }

    async fn translate_chat(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "翻译")?;
        if cfg.model.trim().is_empty() {
            bail!("尚未配置翻译模型名称。");
        }
            let target = target_phrase(&cfg.target_lang);
        let system = translate_system(&cfg.prompt, &target);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let messages = vec![
                json!({ "role": "system", "content": system }),
                json!({ "role": "user", "content": chunk }),
            ];
            let reply = self
                .chat(&cfg.base_url, &cfg.api_key, &cfg.model, messages, 4096, false)
                .await?;
            let cleaned = clean_model_text(&reply);
            if cleaned.is_empty() {
                bail!("模型返回了空译文。");
            }
            out.push(cleaned);
        }
        Ok(out.join("\n"))
    }

    async fn deepl(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "DeepL")?;
        let endpoint = if cfg.deepl_plan == "pro" {
            "https://api.deepl.com/v2/translate"
        } else {
            "https://api-free.deepl.com/v2/translate"
        };
        let target = deepl_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let body = json!({
                "text": [chunk],
                "target_lang": target,
                "preserve_formatting": true,
            });
            let resp = self
                .send_with_retry(
                    || {
                        self.client
                            .post(endpoint)
                            .header("Authorization", format!("DeepL-Auth-Key {}", cfg.api_key.trim()))
                            .json(&body)
                    },
                    "DeepL",
                )
                .await?;
            let parsed: Value = serde_json::from_str(&resp)?;
            let translated = parsed
                .get("translations")
                .and_then(|t| t.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.get("text").and_then(|s| s.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if translated.trim().is_empty() {
                return Err(empty_reply("DeepL", &resp));
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /* ------------------------------------ dedicated translation services */

    /// Youdao picture translation (`ocrtransapi`): the service reads the picture
    /// and translates every region it finds, in one call — no local OCR needed.
    async fn youdao_image(
        &self,
        cfg: &TranslateConfig,
        image: &ImagePayload,
    ) -> Result<(String, String)> {
        require_key(&cfg.api_key, "有道智云")?;
        if cfg.api_secret.trim().is_empty() {
            bail!("有道智云需要两项凭据：应用 ID 与应用密钥。");
        }
        let endpoint = if cfg.base_url.trim().is_empty() {
            "https://openapi.youdao.com/ocrtransapi".to_string()
        } else {
            cfg.base_url.trim().to_string()
        };
        let app_key = cfg.api_key.trim().to_string();
        let key = cfg.api_secret.trim().to_string();
        let to = youdao_target(&cfg.target_lang);
        let salt = crate::model::new_id();
        let curtime = (crate::model::now_ms() / 1000).to_string();
        let input = youdao_input(&image.base64);
        let sign = sha256_hex(&format!("{app_key}{input}{salt}{curtime}{key}"));
        // 1 selects the LLM engine, 0 the general one; every LLM tier uses it.
        let option = if cfg.model_tier.trim().is_empty() || cfg.model_tier.trim() == "standard" {
            "0"
        } else {
            "1"
        };
        let body = form_encode(&[
            ("type", "1"),
            ("from", "auto"),
            ("to", to.as_str()),
            ("appKey", app_key.as_str()),
            ("salt", salt.as_str()),
            ("sign", sign.as_str()),
            ("signType", "v3"),
            ("curtime", curtime.as_str()),
            ("q", image.base64.as_str()),
            ("docType", "json"),
            ("render", "0"),
            ("translateOption", option),
        ]);
        let resp = self
            .send_with_retry(
                || {
                    self.client
                        .post(&endpoint)
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(body.clone())
                },
                "有道图片翻译",
            )
            .await?;
        let parsed: Value =
            serde_json::from_str(&resp).context("有道图片翻译返回的内容不是 JSON")?;
        if let Some(code) = parsed.get("errorCode") {
            let code = code_text(code);
            if code != "0" {
                bail!("有道图片翻译报错 {code}：{}", youdao_error(&code));
            }
        }
        let regions = parsed
            .get("resRegions")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let collect = |field: &str| {
            regions
                .iter()
                .filter_map(|region| region.get(field).and_then(|v| v.as_str()))
                .filter(|line| !line.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let (source, translation) = (collect("context"), collect("tranContent"));
        if source.trim().is_empty() && translation.trim().is_empty() {
            return Err(empty_reply("有道图片翻译", &resp));
        }
        Ok((source, translation))
    }

    /// Baidu picture translation (`api/trans/sdk/picture`): a multipart upload
    /// whose signature covers the md5 of the file bytes themselves.
    async fn baidu_image(
        &self,
        cfg: &TranslateConfig,
        image: &ImagePayload,
    ) -> Result<(String, String)> {
        require_key(&cfg.api_key, "百度翻译")?;
        if cfg.api_secret.trim().is_empty() {
            // The picture endpoint only signs; a Bearer key cannot be used there.
            if cfg.auth.trim() == "key" {
                bail!("百度图片翻译只支持签名鉴权：请把「鉴权方式」改成「签名」并填写应用密钥（API Key 鉴权只适用于大模型文本翻译）。");
            }
            bail!("百度翻译需要两项凭据：APP ID 与密钥。");
        }
        let endpoint = if cfg.base_url.trim().is_empty() {
            "https://fanyi-api.baidu.com/api/trans/sdk/picture".to_string()
        } else {
            cfg.base_url.trim().to_string()
        };
        let app_id = cfg.api_key.trim().to_string();
        let key = cfg.api_secret.trim().to_string();
        let to = baidu_target(&cfg.target_lang);
        let salt = crate::model::new_id();
        let cuid = "SnapLingo".to_string();
        let mac = "SnapLingo".to_string();
        let file_hash = md5_hex_bytes(&image.bytes);
        let sign = md5_hex(&format!("{app_id}{file_hash}{salt}{cuid}{mac}{key}"));
        let resp = self
            .send_with_retry(
                || {
                    // Rebuilt per attempt: a multipart body is consumed by the
                    // request that sends it.
                    let form = reqwest::multipart::Form::new()
                        .part(
                            "image",
                            reqwest::multipart::Part::bytes(image.bytes.clone())
                                .file_name("snapping.jpg"),
                        )
                        .text("from", "auto")
                        .text("to", to.clone())
                        .text("appid", app_id.clone())
                        .text("salt", salt.clone())
                        .text("cuid", cuid.clone())
                        .text("mac", mac.clone())
                        .text("version", "3")
                        .text("paste", "0")
                        .text("sign", sign.clone());
                    self.client.post(&endpoint).multipart(form)
                },
                "百度图片翻译",
            )
            .await?;
        let parsed: Value =
            serde_json::from_str(&resp).context("百度图片翻译返回的内容不是 JSON")?;
        if let Some(error) = parsed.get("error_code") {
            let code = code_text(error);
            if code != "0" && code != "52000" {
                bail!("百度图片翻译报错 {code}：{}", baidu_error(&code));
            }
        }
        let data = parsed.get("data").cloned().unwrap_or(Value::Null);
        let text_of = |field: &str, fallback: &str| {
            let direct = data
                .get(field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !direct.trim().is_empty() {
                return direct;
            }
            data.get("content")
                .and_then(|c| c.as_array())
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(|line| line.get(fallback).and_then(|v| v.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };
        let (source, translation) = (text_of("sumSrc", "src"), text_of("sumDst", "dst"));
        if source.trim().is_empty() && translation.trim().is_empty() {
            return Err(empty_reply("百度图片翻译", &resp));
        }
        Ok((source, translation))
    }

    /// Tencent Cloud end-to-end picture translation (`ImageTranslateLLM`).
    /// Returns the text inside the picture and its translation.
    async fn tencent_image(
        &self,
        cfg: &TranslateConfig,
        image: &ImagePayload,
    ) -> Result<(String, String)> {
        require_key(&cfg.api_key, "腾讯翻译")?;
        if cfg.api_secret.trim().is_empty() {
            bail!("腾讯翻译需要两项凭据：SecretId 与 SecretKey。");
        }
        let host = if cfg.base_url.trim().is_empty() {
            "tmt.tencentcloudapi.com".to_string()
        } else {
            host_of(cfg.base_url.trim())
        };
        let url = format!("https://{host}/");
        let region = if cfg.region.trim().is_empty() {
            "ap-guangzhou".to_string()
        } else {
            cfg.region.trim().to_string()
        };
        let secret_id = cfg.api_key.trim().to_string();
        let secret_key = cfg.api_secret.trim().to_string();
        let target = tencent_target(&cfg.target_lang);
        let mode = if cfg.model_tier.trim() == "lite" { 1 } else { 0 };
        let payload = json!({
            "Data": image.base64,
            "Target": target,
            "Mode": mode,
        })
        .to_string();
        let timestamp = crate::model::now_ms() / 1000;
        let date = utc_date(timestamp);
        let authorization =
            tc3_authorization(&secret_id, &secret_key, &host, &payload, timestamp, &date);
        let resp = self
            .send_with_retry(
                || {
                    self.client
                        .post(&url)
                        .header("Authorization", authorization.as_str())
                        .header("Content-Type", "application/json; charset=utf-8")
                        .header("X-TC-Action", "ImageTranslateLLM")
                        .header("X-TC-Version", "2018-03-21")
                        .header("X-TC-Timestamp", timestamp.to_string())
                        .header("X-TC-Region", region.as_str())
                        .body(payload.clone())
                },
                "腾讯图片翻译",
            )
            .await?;
        let parsed: Value = serde_json::from_str(&resp).context("腾讯图片翻译返回的不是 JSON")?;
        let response = parsed.get("Response").cloned().unwrap_or(Value::Null);
        if let Some(err) = response.get("Error") {
            let code = err.get("Code").and_then(|c| c.as_str()).unwrap_or("未知错误");
            let message = err.get("Message").and_then(|m| m.as_str()).unwrap_or("");
            bail!("腾讯图片翻译报错 {code}：{message}");
        }
        let source = response
            .get("SourceText")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        let translated = response
            .get("TargetText")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        if source.trim().is_empty() && translated.trim().is_empty() {
            return Err(empty_reply("腾讯图片翻译", &resp));
        }
        Ok((source, translated))
    }

    /// Azure AI Translator (Microsoft). A subscription key plus an optional
    /// region — required when the key belongs to a multi-service resource.
    async fn microsoft(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "微软翻译")?;
        let endpoint = if cfg.base_url.trim().is_empty() {
            "https://api.cognitive.microsofttranslator.com/translate".to_string()
        } else {
            format!("{}/translate", cfg.base_url.trim().trim_end_matches('/'))
        };
        let url = format!(
            "{endpoint}?api-version=3.0&to={}",
            microsoft_target(&cfg.target_lang)
        );
        let key = cfg.api_key.trim().to_string();
        let region = cfg.region.trim().to_string();

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let body = json!([{ "Text": chunk }]);
            let resp = self
                .send_with_retry(
                    || {
                        let mut req = self
                            .client
                            .post(&url)
                            .header("Content-Type", "application/json; charset=utf-8")
                            .header("Ocp-Apim-Subscription-Key", key.as_str())
                            .json(&body);
                        if !region.is_empty() {
                            req = req.header("Ocp-Apim-Subscription-Region", region.as_str());
                        }
                        req
                    },
                    "微软翻译",
                )
                .await?;
            let parsed: Value =
                serde_json::from_str(&resp).context("微软翻译返回的不是 JSON")?;
            let translated = parsed
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("translations"))
                .and_then(|t| t.as_array())
                .map(|arr| join_strings(arr, "text"))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                return Err(empty_reply("微软翻译", &resp));
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Google Cloud Translation v2. The key travels in the `X-Goog-Api-Key`
    /// header, never in the query string: reqwest prints the whole URL on a
    /// transport error, so a `?key=…` URL would leak the secret into the log.
    async fn google(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "谷歌翻译")?;
        let url = google_url(cfg);
        let key = cfg.api_key.trim().to_string();
        // A header value only accepts visible ASCII. A stray space or a
        // full-width character copied off a web page would surface as a builder
        // error, which reads like a network failure and gets retried four
        // times. Name the real problem instead.
        if !key.is_ascii() || key.chars().any(char::is_whitespace) {
            bail!("谷歌翻译的 API Key 含空格或非 ASCII 字符，请检查是否复制完整。");
        }
        let target = google_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let body = json!({ "q": [chunk], "target": target, "format": "text" });
            let resp = self
                .send_with_retry(
                    || {
                        self.client
                            .post(&url)
                            .header("X-Goog-Api-Key", key.as_str())
                            .json(&body)
                    },
                    "谷歌翻译",
                )
                .await?;
            let parsed: Value = serde_json::from_str(&resp).context("谷歌翻译返回的不是 JSON")?;
            let translated = parsed
                .pointer("/data/translations")
                .and_then(|t| t.as_array())
                .map(|arr| join_strings(arr, "translatedText"))
                .map(|s| unescape_html(&s))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                return Err(empty_reply("谷歌翻译", &resp));
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Youdao Zhiyun text translation. v3 signature over a form body.
    async fn youdao(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        // The `llm` / `lite` tiers are a different product of the same console:
        // the streaming 子曰 LLM endpoint instead of the classic one.
        if matches!(cfg.model_tier.trim(), "llm" | "lite") {
            return self.youdao_llm(cfg, text).await;
        }
        require_key(&cfg.api_key, "有道翻译")?;
        if cfg.api_secret.trim().is_empty() {
            bail!("有道翻译需要两项凭据：应用 ID 与应用密钥。");
        }
        let endpoint = if cfg.base_url.trim().is_empty() {
            "https://openapi.youdao.com/api".to_string()
        } else {
            cfg.base_url.trim().to_string()
        };
        let app_key = cfg.api_key.trim().to_string();
        let app_secret = cfg.api_secret.trim().to_string();
        let to = youdao_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let salt = crate::model::new_id();
            let curtime = (crate::model::now_ms() / 1000).to_string();
            let input = youdao_input(&chunk);
            let sign = sha256_hex(&format!("{app_key}{input}{salt}{curtime}{app_secret}"));
            let body = form_encode(&[
                ("q", chunk.as_str()),
                ("from", "auto"),
                ("to", to.as_str()),
                ("appKey", app_key.as_str()),
                ("salt", salt.as_str()),
                ("sign", sign.as_str()),
                ("signType", "v3"),
                ("curtime", curtime.as_str()),
            ]);
            let resp = self
                .send_with_retry(
                    || {
                        self.client
                            .post(&endpoint)
                            .header("Content-Type", "application/x-www-form-urlencoded")
                            .body(body.clone())
                    },
                    "有道翻译",
                )
                .await?;
            let parsed: Value = serde_json::from_str(&resp).context("有道翻译返回的不是 JSON")?;
            let code = parsed.get("errorCode").map(code_text).unwrap_or_default();
            if code != "0" {
                bail!("有道翻译报错 {code}：{}", youdao_error(&code));
            }
            let translated = parsed
                .get("translation")
                .and_then(|t| t.as_array())
                .map(|arr| join_strings(arr, ""))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                return Err(empty_reply("有道翻译", &resp));
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Youdao's LLM translation channel (`proxy/http/llm-trans`). The service
    /// answers as a Server-Sent Events stream; `streamType=full` makes every
    /// event carry the text so far, so the last one holds the finished
    /// translation. `handleOption` picks the model (0 = 子曰 pro, 3 = lite).
    async fn youdao_llm(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "有道大模型翻译")?;
        if cfg.api_secret.trim().is_empty() {
            bail!("有道大模型翻译需要两项凭据：应用 ID 与应用密钥。");
        }
        let endpoint = default_or(&cfg.base_url, YOUDAO_LLM_ENDPOINT);
        let app_key = cfg.api_key.trim().to_string();
        let app_secret = cfg.api_secret.trim().to_string();
        let to = youdao_target(&cfg.target_lang);
        let handle = if cfg.model_tier.trim() == "lite" { "3" } else { "0" };

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let salt = crate::model::new_id();
            let curtime = (crate::model::now_ms() / 1000).to_string();
            let input = youdao_input(&chunk);
            let sign = sha256_hex(&format!("{app_key}{input}{salt}{curtime}{app_secret}"));
            let body = form_encode(&[
                ("i", chunk.as_str()),
                ("from", "auto"),
                ("to", to.as_str()),
                ("appKey", app_key.as_str()),
                ("salt", salt.as_str()),
                ("sign", sign.as_str()),
                ("signType", "v3"),
                ("curtime", curtime.as_str()),
                ("streamType", "full"),
                ("handleOption", handle),
            ]);
            let resp = self
                .send_with_retry(
                    || {
                        self.client
                            .post(&endpoint)
                            .header("Content-Type", "application/x-www-form-urlencoded")
                            .header("Accept", "text/event-stream")
                            .body(body.clone())
                    },
                    "有道大模型翻译",
                )
                .await?;
            out.push(parse_youdao_llm(&resp)?);
        }
        Ok(out.join("\n"))
    }

    /// Baidu's LLM translation (`ait/api/aiTextTranslate`). Two ways to
    /// authenticate: the md5 signature shared with the classic API, or an
    /// `Authorization: Bearer <API Key>` header created in the console.
    async fn baidu_llm(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        let endpoint = if cfg.base_url.trim().is_empty() {
            "https://fanyi-api.baidu.com/ait/api/aiTextTranslate".to_string()
        } else {
            cfg.base_url.trim().to_string()
        };
        let app_id = cfg.api_key.trim().to_string();
        let key = cfg.api_secret.trim().to_string();
        let bearer = cfg.bearer.trim().to_string();
        // `key` is the explicit choice; a filled-in Bearer key also implies it.
        let key_auth = cfg.auth.trim() == "key" || (key.is_empty() && !bearer.is_empty());
        if key_auth {
            if bearer.is_empty() {
                bail!("百度大模型翻译选择了「API Key」鉴权，请在设置里填写控制台创建的 API Key。");
            }
        } else if key.is_empty() {
            bail!("百度大模型翻译需要应用密钥，或把鉴权方式改为「API Key」。");
        }
        let to = baidu_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let salt = crate::model::new_id();
            let body = if key_auth {
                json!({
                    "appid": app_id,
                    "from": "auto",
                    "to": to,
                    "q": chunk,
                    "model_type": "llm",
                })
            } else {
                let sign = md5_hex(&format!("{app_id}{chunk}{salt}{key}"));
                json!({
                    "appid": app_id,
                    "from": "auto",
                    "to": to,
                    "q": chunk,
                    "salt": salt,
                    "sign": sign,
                    "model_type": "llm",
                })
            };
            let resp = self
                .send_with_retry(
                    || {
                        let request = self.client.post(&endpoint).json(&body);
                        if key_auth {
                            request.header("Authorization", format!("Bearer {bearer}"))
                        } else {
                            request
                        }
                    },
                    "百度大模型翻译",
                )
                .await?;
            let parsed: Value =
                serde_json::from_str(&resp).context("百度大模型翻译返回的内容不是 JSON")?;
            // 52000 is success for this endpoint; 0 and a missing code are fine too.
            if let Some(code) = baidu_error_code(&parsed) {
                bail!("百度大模型翻译报错 {code}：{}", baidu_error(&code));
            }
            let translated = trans_segments(&parsed)
                .map(|arr| join_strings(arr, "dst"))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                return Err(empty_reply("百度大模型翻译", &resp));
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Baidu Translate open API. md5 signature over a form body.
    async fn baidu(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "百度翻译")?;
        // 「大模型翻译」 is a separate service on its own endpoint, and it can also
        // authenticate with a Bearer API Key — so it must be dispatched *before*
        // the signature-only requirement below, otherwise a Bearer setup is
        // rejected for a missing app secret it does not need.
        if cfg.model_tier.trim() == "llm" {
            return self.baidu_llm(cfg, text).await;
        }
        if cfg.api_secret.trim().is_empty() {
            bail!("百度翻译需要两项凭据：APP ID 与密钥。");
        }
        let endpoint = if cfg.base_url.trim().is_empty() {
            "https://fanyi-api.baidu.com/api/trans/vip/translate".to_string()
        } else {
            cfg.base_url.trim().to_string()
        };
        let app_id = cfg.api_key.trim().to_string();
        let key = cfg.api_secret.trim().to_string();
        let to = baidu_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let salt = crate::model::new_id();
            let sign = md5_hex(&format!("{app_id}{chunk}{salt}{key}"));
            let body = form_encode(&[
                ("q", chunk.as_str()),
                ("from", "auto"),
                ("to", to.as_str()),
                ("appid", app_id.as_str()),
                ("salt", salt.as_str()),
                ("sign", sign.as_str()),
            ]);
            let resp = self
                .send_with_retry(
                    || {
                        self.client
                            .post(&endpoint)
                            .header("Content-Type", "application/x-www-form-urlencoded")
                            .body(body.clone())
                    },
                    "百度翻译",
                )
                .await?;
            let parsed: Value = serde_json::from_str(&resp).context("百度翻译返回的不是 JSON")?;
            if let Some(code) = baidu_error_code(&parsed) {
                bail!("百度翻译报错 {code}：{}", baidu_error(&code));
            }
            let translated = trans_segments(&parsed)
                .map(|arr| join_strings(arr, "dst"))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                return Err(empty_reply("百度翻译", &resp));
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Tencent Cloud machine translation (TMT). TC3-HMAC-SHA256 signed.
    async fn tencent(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "腾讯翻译")?;
        if cfg.api_secret.trim().is_empty() {
            bail!("腾讯翻译需要两项凭据：SecretId 与 SecretKey。");
        }
        let host = if cfg.base_url.trim().is_empty() {
            "tmt.tencentcloudapi.com".to_string()
        } else {
            host_of(cfg.base_url.trim())
        };
        let url = format!("https://{host}/");
        let region = if cfg.region.trim().is_empty() {
            "ap-guangzhou".to_string()
        } else {
            cfg.region.trim().to_string()
        };
        let secret_id = cfg.api_key.trim().to_string();
        let secret_key = cfg.api_secret.trim().to_string();
        let to = tencent_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let payload = json!({
                "SourceText": chunk,
                "Source": "auto",
                "Target": to,
                "ProjectId": 0,
            })
            .to_string();
            let timestamp = crate::model::now_ms() / 1000;
            let date = utc_date(timestamp);
            let authorization =
                tc3_authorization(&secret_id, &secret_key, &host, &payload, timestamp, &date);
            let resp = self
                .send_with_retry(
                    || {
                        self.client
                            .post(&url)
                            .header("Authorization", authorization.as_str())
                            .header("Content-Type", "application/json; charset=utf-8")
                            .header("X-TC-Action", "TextTranslate")
                            .header("X-TC-Version", "2018-03-21")
                            .header("X-TC-Timestamp", timestamp.to_string())
                            .header("X-TC-Region", region.as_str())
                            .body(payload.clone())
                    },
                    "腾讯翻译",
                )
                .await?;
            let parsed: Value = serde_json::from_str(&resp).context("腾讯翻译返回的不是 JSON")?;
            let response = parsed.get("Response").cloned().unwrap_or(Value::Null);
            if let Some(err) = response.get("Error") {
                let code = err
                    .get("Code")
                    .and_then(|c| c.as_str())
                    .unwrap_or("未知错误");
                let message = err.get("Message").and_then(|m| m.as_str()).unwrap_or("");
                bail!("腾讯翻译报错 {code}：{message}");
            }
            let translated = response
                .get("TargetText")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            if translated.trim().is_empty() {
                return Err(empty_reply("腾讯翻译", &resp));
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /* ------------------------------------------------------------ misc */

    pub async fn list_models(&self, base_url: &str, api_key: &str) -> Result<Vec<String>> {
        let url = format!("{}/models", base_url.trim_end_matches('/'));
        let resp = self
            .send_with_retry(
                || {
                    let mut req = self.client.get(&url);
                    if !api_key.trim().is_empty() {
                        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
                    }
                    req
                },
                "模型列表",
            )
            .await?;
        let parsed: Value = serde_json::from_str(&resp)?;
        let mut models: Vec<String> = parsed
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if models.is_empty() {
            bail!("该接口没有返回任何模型。请确认 Base URL 是否正确（应指向 /v1）。");
        }
        models.sort();
        Ok(models)
    }

    /// Prove the endpoint + key + model work. Never panics; the failure text is
    /// what the settings UI shows.
    pub async fn test(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        kind: &str,
    ) -> ConnectionCheck {
        let started = Instant::now();
        let messages = vec![json!({ "role": "user", "content": "Reply with the single word: ok" })];
        let result = self.chat(base_url, api_key, model, messages, 16, false).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(text) => ConnectionCheck {
                ok: true,
                message: if kind == "translate" {
                    "翻译接口连接正常".to_string()
                } else {
                    "OCR 接口连接正常".to_string()
                },
                elapsed_ms,
                sample: Some(text.chars().take(120).collect()),
            },
            Err(err) => ConnectionCheck {
                ok: false,
                message: format!("{err:#}"),
                elapsed_ms,
                sample: None,
            },
        }
    }

    /// Prove the configured translation engine works, whichever one it is.
    pub async fn test_translate(&self, cfg: &TranslateConfig) -> ConnectionCheck {
        let started = Instant::now();
        if cfg.provider == "none" {
            return ConnectionCheck {
                ok: true,
                message: "当前设置为「不翻译」，截图只显示识别到的原文。".to_string(),
                elapsed_ms: 0,
                sample: None,
            };
        }
        let result = self.translate(cfg, "Hello, world!").await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(text) if !text.trim().is_empty() => ConnectionCheck {
                ok: true,
                message: format!("{}连接正常", provider_label(&cfg.provider)),
                elapsed_ms,
                sample: Some(text.trim().chars().take(120).collect()),
            },
            Ok(_) => ConnectionCheck {
                ok: false,
                message: format!("{}返回了空译文。", provider_label(&cfg.provider)),
                elapsed_ms,
                sample: None,
            },
            Err(err) => ConnectionCheck {
                ok: false,
                message: format!("{err:#}"),
                elapsed_ms,
                sample: None,
            },
        }
    }

    /* -------------------------------------------------------- internals */

    /// One chat completion. `json_mode` asks for a JSON object when the provider
    /// supports it, and is dropped silently when it does not.
    async fn chat(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: Vec<Value>,
        max_tokens: u32,
        json_mode: bool,
    ) -> Result<String> {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        let build = |with_json_mode: bool, with_max_tokens: bool| {
            let mut body = json!({
                "model": model,
                "messages": messages,
                "temperature": 0.2,
                "stream": false,
            });
            if with_max_tokens {
                body["max_tokens"] = json!(max_tokens);
            }
            if with_json_mode {
                body["response_format"] = json!({ "type": "json_object" });
            }
            body
        };

        // Local servers (Ollama, LM Studio) need no key at all, and an empty
        // `Bearer ` header makes strict gateways reject the call outright.
        let authorized = |req: reqwest::RequestBuilder| {
            if api_key.trim().is_empty() {
                req
            } else {
                req.header("Authorization", format!("Bearer {}", api_key.trim()))
            }
        };

        let raw = match self
            .send_with_retry(
                || authorized(self.client.post(&url)).json(&build(json_mode, true)),
                "聊天补全",
            )
            .await
        {
            Ok(v) => v,
            Err(err) if is_param_rejection(&err) => {
                // Provider dislikes response_format -> retry as plain text.
                log::warn!("provider rejected optional params, retrying plain: {err:#}");
                match self
                    .send_with_retry(
                        || authorized(self.client.post(&url)).json(&build(false, false)),
                        "聊天补全",
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(_) => return Err(err),
                }
            }
            Err(err) => return Err(err),
        };

        let parsed: Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(err) => {
                log::error!("聊天补全 响应体：{}", truncate(raw.trim(), 1200));
                return Err(anyhow!("响应不是合法 JSON：{err}"));
            }
        };
        let message = match parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
        {
            Some(message) => message,
            None => {
                log::error!("聊天补全 响应体：{}", truncate(raw.trim(), 1200));
                return Err(anyhow!("响应缺少 choices[0].message。"));
            }
        };

        // `content` may be a string or an array of parts depending on provider.
        let content = match message.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        };

        if content.trim().is_empty() {
            // Reasoning models sometimes only fill `reasoning_content`.
            if let Some(rc) = message.get("reasoning_content").and_then(|r| r.as_str()) {
                if !rc.trim().is_empty() {
                    return Ok(rc.to_string());
                }
            }
            let finish = parsed
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("finish_reason"))
                .and_then(|f| f.as_str())
                .unwrap_or("unknown");
            log::error!("聊天补全 响应体：{}", truncate(raw.trim(), 1200));
            bail!("模型返回了空内容（finish_reason={finish}）。");
        }
        Ok(content)
    }

    /// POST with bounded exponential backoff on transient failures.
    async fn send_with_retry<F>(&self, build: F, what: &str) -> Result<String>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let mut last: Option<anyhow::Error> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            let started = Instant::now();
            let outcome = match build().send().await {
                Err(err) => {
                    log::warn!(
                        "{what} 第 {attempt}/{MAX_ATTEMPTS} 次请求失败（{} ms）：{}",
                        started.elapsed().as_millis(),
                        redact_error(&err)
                    );
                    Err(anyhow!(classify_transport(&err)))
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    log::info!(
                        "{what} 响应 {} · {} ms · {} 字节",
                        status.as_u16(),
                        started.elapsed().as_millis(),
                        body.len()
                    );
                    if status.is_success() {
                        return Ok(body);
                    }
                    // The log is the only place a user can see what an engine
                    // really answered, so make the body part of it whenever the
                    // request failed.
                    log::error!(
                        "{what} 响应体（HTTP {}）：{}",
                        status.as_u16(),
                        truncate(body.trim(), 1200)
                    );
                    Err(anyhow!(explain_http_error(status.as_u16(), &body)))
                }
            };

            match outcome {
                Ok(v) => return Ok(v),
                Err(err) => {
                    let retryable = is_retryable(&err);
                    log::debug!("{what} attempt {attempt}/{MAX_ATTEMPTS} failed: {err}");
                    last = Some(err);
                    if !retryable || attempt == MAX_ATTEMPTS {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(400 * 2u64.pow(attempt - 1))).await;
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("{what} 失败")))
    }
}

/* --------------------------------------------------------------- helpers */

/* ---------------------------------------------- 日志与「实际调用」端点展示 */

/// 日志里出现的端点（与各引擎里的默认值保持一致）。
const DEEPL_FREE_ENDPOINT: &str = "https://api-free.deepl.com/v2/translate";
const DEEPL_PRO_ENDPOINT: &str = "https://api.deepl.com/v2/translate";
const MICROSOFT_ENDPOINT: &str = "https://api.cognitive.microsofttranslator.com/translate";
const GOOGLE_ENDPOINT: &str = "https://translation.googleapis.com/language/translate/v2";
const YOUDAO_ENDPOINT: &str = "https://openapi.youdao.com/api";
const YOUDAO_LLM_ENDPOINT: &str = "https://openapi.youdao.com/proxy/http/llm-trans";
const YOUDAO_IMAGE_ENDPOINT: &str = "https://openapi.youdao.com/ocrtransapi";
const BAIDU_ENDPOINT: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
const BAIDU_LLM_ENDPOINT: &str = "https://fanyi-api.baidu.com/ait/api/aiTextTranslate";
const BAIDU_IMAGE_ENDPOINT: &str = "https://fanyi-api.baidu.com/api/trans/sdk/picture";
const TENCENT_HOST: &str = "tmt.tencentcloudapi.com";

fn default_or(base: &str, fallback: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// 这个翻译渠道实际会请求的地址（`base_url` 为空时用官方默认值）。
pub fn text_endpoint(provider: &str, cfg: &TranslateConfig) -> Option<String> {
    let profile = cfg.profile(provider);
    let base = default_or(&profile.base_url, "");
    let own = |fallback: &str| {
        if base.is_empty() {
            fallback.to_string()
        } else {
            base.clone()
        }
    };
    Some(match provider {
        "openai" => {
            let root = if base.is_empty() {
                DEFAULT_OPENAI_BASE_URL.trim_end_matches('/').to_string()
            } else {
                base.clone()
            };
            format!("{root}/chat/completions")
        }
        "deepl" => {
            if cfg.deepl_plan.trim() == "pro" {
                DEEPL_PRO_ENDPOINT.to_string()
            } else {
                DEEPL_FREE_ENDPOINT.to_string()
            }
        }
        "microsoft" => own(MICROSOFT_ENDPOINT),
        "google" => own(GOOGLE_ENDPOINT),
        "youdao" => {
            if matches!(profile.tier("youdao").as_str(), "llm" | "lite") {
                YOUDAO_LLM_ENDPOINT.to_string()
            } else {
                own(YOUDAO_ENDPOINT)
            }
        }
        "baidu" => {
            if profile.tier("baidu") == "llm" {
                BAIDU_LLM_ENDPOINT.to_string()
            } else {
                own(BAIDU_ENDPOINT)
            }
        }
        "tencent" => format!("https://{}/", own(TENCENT_HOST)),
        _ => return None,
    })
}

/// 一步图片翻译实际会请求的地址。
pub fn image_endpoint(provider: &str, cfg: &TranslateConfig) -> Option<String> {
    let profile = cfg.profile(provider);
    let base = default_or(&profile.base_url, "");
    let own = |fallback: &str| {
        if base.is_empty() {
            fallback.to_string()
        } else {
            base.clone()
        }
    };
    Some(match provider {
        "youdao" => own(YOUDAO_IMAGE_ENDPOINT),
        "baidu" => own(BAIDU_IMAGE_ENDPOINT),
        "tencent" => format!("https://{}/", own(TENCENT_HOST)),
        _ => return None,
    })
}

/// OCR 渠道实际会请求的地址；Windows 内置 OCR 完全在本机，没有地址。
pub fn ocr_endpoint(ocr: &OcrConfig) -> Option<String> {
    match ocr.provider.as_str() {
        "local" => local_ocr_url(ocr).ok(),
        "vision" => {
            let root = default_or(&ocr.active().base_url, DEFAULT_OPENAI_BASE_URL);
            Some(format!("{root}/chat/completions"))
        }
        _ => None,
    }
}

/// 档位（通用 / 大模型…）到底改变了什么：有的换端点，有的只是换一个请求参数。
/// `image` 为真时描述一步图片翻译那条路径。
pub fn tier_note(provider: &str, tier: &str, image: bool) -> Option<String> {
    Some(match (provider, image) {
        ("baidu", false) => {
            if tier == "llm" {
                "大模型翻译走 /ait/api/aiTextTranslate".to_string()
            } else {
                "通用翻译走 /api/trans/vip/translate".to_string()
            }
        }
        ("baidu", true) => "图片翻译接口固定，档位只影响文本翻译".to_string(),
        ("youdao", false) => match tier {
            "llm" => "大模型翻译走 /proxy/http/llm-trans（handleOption=0，子曰翻译大模型 Pro）"
                .to_string(),
            "lite" => "大模型翻译走 /proxy/http/llm-trans（handleOption=3，子曰轻量版 Lite）"
                .to_string(),
            _ => "通用文本翻译走 /api（v3 签名）".to_string(),
        },
        ("youdao", true) => format!(
            "translateOption={}",
            if tier.is_empty() || tier == "standard" {
                "0（传统翻译）"
            } else {
                "1（大模型译后编辑）"
            }
        ),
        ("tencent", false) => "档位只影响一步图片翻译（Mode）".to_string(),
        ("tencent", true) => format!(
            "Mode={}",
            if tier == "lite" {
                "1（极速版）"
            } else {
                "0（大模型版）"
            }
        ),
        _ => return None,
    })
}

/// 一行摘要：这次会走哪个识别渠道、哪个翻译接口。写进日志方便自查。
pub fn describe_run(cfg: &AppConfig) -> String {
    let ocr = match cfg.ocr.provider.as_str() {
        "winocr" => "Windows 内置 OCR（离线，本机）".to_string(),
        "local" => format!(
            "本地 OCR 服务（{}）",
            ocr_endpoint(&cfg.ocr).unwrap_or_else(|| "地址未填".into())
        ),
        "vision" => format!(
            "视觉大模型（{}）",
            ocr_endpoint(&cfg.ocr).unwrap_or_else(|| "地址未填".into())
        ),
        other => match cfg.picture_provider() {
            Some(id) => format!("一步图片翻译（{}）", provider_label(id)),
            None => format!("{other}（未配置）"),
        },
    };
    let translate = match cfg.translate.provider.as_str() {
        "none" => "不翻译".to_string(),
        id => format!(
            "{}（{}）",
            provider_label(id),
            text_endpoint(id, &cfg.translate).unwrap_or_else(|| "地址未填".into())
        ),
    };
    format!("OCR = {ocr} · 翻译 = {translate}")
}

/// 密钥的脱敏展示：只露头尾，绝不把完整密钥写进日志。
pub fn mask_secret(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "未设置".to_string();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 10 {
        return "已设置".to_string();
    }
    let head: String = chars.iter().take(3).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}…{tail}")
}

fn is_secret_query(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "key" | "api_key" | "apikey" | "access_token" | "token" | "sign" | "signature" | "q"
    )
}

/// URL 里的敏感查询参数打码。请求地址可能带着 `?key=…`，任何要落盘的地方都
/// 必须先过这一层：日志对用户（以及任何分享日志的人）都是可读的。
fn scrub_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let scrubbed: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_secret_query(k) => format!("{k}=***"),
            _ => pair.to_string(),
        })
        .collect();
    format!("{base}?{}", scrubbed.join("&"))
}

/// Scrub one known URL out of any text it was embedded in. Split out from
/// `redact_error` so the rule itself is testable without a live transport error.
fn redact_url_in(text: &str, url: &str) -> String {
    text.replace(url, &scrub_url(url))
}

/// The Display form of a reqwest error embeds the request URL — which may carry
/// a credential in the query string. Scrub it before it reaches the log.
fn redact_error(err: &reqwest::Error) -> String {
    match err.url() {
        Some(url) => redact_url_in(&err.to_string(), url.as_str()),
        None => err.to_string(),
    }
}

/// 每发出一个请求就留一行：走了哪个渠道、哪个地址、带了什么参数。
pub fn trace_request(engine: &str, method: &str, url: &str, params: &[(&str, String)]) {
    let mut line = format!("调用 {engine} · {method} {}", scrub_url(url));
    let pairs: Vec<String> = params
        .iter()
        .filter(|(_, value)| !value.trim().is_empty() && value != "未设置")
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if !pairs.is_empty() {
        line.push_str(" · ");
        line.push_str(&pairs.join(" · "));
    }
    log::info!("{line}");
}

fn require_key(key: &str, what: &str) -> Result<()> {
    if key.trim().is_empty() {
        bail!("{what} 尚未配置 API Key。请打开设置填写。");
    }
    Ok(())
}

fn is_retryable(err: &anyhow::Error) -> bool {
    let msg = format!("{err}");
    msg.starts_with("RETRY:")
}

fn classify_transport(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "RETRY: 请求超时（网络较慢或模型响应过久），请重试。".to_string()
    } else if err.is_connect() {
        "RETRY: 无法连接到服务端点，请检查网络或 Base URL。".to_string()
    } else {
        format!("RETRY: 网络请求失败：{}", redact_error(err))
    }
}

fn explain_http_error(status: u16, body: &str) -> String {
    let detail = extract_error_message(body).unwrap_or_else(|| truncate(body, 300));

    // Gateways answer an unauthenticated call in their own way; NVIDIA's router
    // (axum) reports it as a generic 500 about a missing Authorization extension.
    if body.to_ascii_lowercase().contains("authorization") {
        return format!(
            "接口没有收到有效的 Authorization（API Key 为空或无效）。请在设置里填写 API Key；本地服务（Ollama / LM Studio）可以留空。 HTTP {status}: {detail}"
        );
    }

    let prefix = match status {
        401 => "鉴权失败（401）——请检查 API Key 是否正确、是否已过期。",
        403 => {
            // OpenRouter and friends use 403 for region locks, which is not a
            // key problem at all and reads very differently to the user.
            let lower = body.to_ascii_lowercase();
            if lower.contains("region") || lower.contains("country") || lower.contains("territor") {
                "服务端拒绝访问（403）：所选模型在你的地区不可用。请换一个模型，或改用其他服务商。"
            } else {
                "鉴权失败（403）——请检查 API Key 是否有该模型的权限。"
            }
        }
        429 => "RETRY: 触发限流（429）——稍后自动重试。",
        503 => {
            "RETRY: 服务端过载（503）——正在自动重试；如果一直失败，通常是所选模型当前负载过高，可稍后再试，或换用更轻量的模型。"
        }
        500..=599 => "RETRY: 服务端错误——稍后自动重试。",
        _ => "请求被拒绝：",
    };
    format!("{prefix} HTTP {status}: {detail}")
}

fn extract_error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let err = v.get("error")?;
    if let Some(s) = err.as_str() {
        return Some(s.to_string());
    }
    err.get("message")
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .or_else(|| Some(err.to_string()))
}

/// True when the error looks like "you passed an unsupported parameter".
fn is_param_rejection(err: &anyhow::Error) -> bool {
    let msg = format!("{err}").to_lowercase();
    !msg.starts_with("retry: ")
        && (msg.contains("response_format")
            || msg.contains("max_tokens")
            || msg.contains("unsupported parameter")
            || msg.contains("unrecognized"))
}

fn truncate(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

/// Strip thinking blocks, code fences and "译文：" style prefixes that models
/// like to add even when told not to.
pub fn clean_model_text(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    for (open, close) in [
        ("<think>", "</think>"),
        ("<thinking>", "</thinking>"),
        ("<reasoning>", "</reasoning>"),
    ] {
        while let Some(a) = s.find(open) {
            match s[a..].find(close) {
                Some(b) => {
                    let end = a + b + close.len();
                    s.replace_range(a..end, "");
                }
                None => {
                    s.truncate(a);
                    break;
                }
            }
        }
    }

    // ``` … ``` fences, possibly with a language tag.
    if s.starts_with("```") {
        let rest = &s[3..];
        let rest = match rest.find('\n') {
            Some(nl) if !rest[..nl].contains('`') => &rest[nl + 1..],
            _ => rest,
        };
        s = rest.trim_end().to_string();
        if let Some(pos) = s.rfind("```") {
            s.truncate(pos);
        }
    }

    // Leading labels such as "译文：" / "Translation:".
    for label in ["译文：", "译文:", "翻译：", "翻译:", "Translation:", "translation:"] {
        if let Some(rest) = s.strip_prefix(label) {
            s = rest.trim_start().to_string();
            break;
        }
    }

    // Whole-answer quotes.
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') && !t[1..t.len() - 1].contains('"') {
        s = t[1..t.len() - 1].to_string();
    }

    s.trim().to_string()
}

/// Parse the one-shot JSON reply, tolerating surrounding prose and fences.
pub fn parse_one_shot(raw: &str) -> crate::model::OneShotReply {
    let cleaned = clean_model_text(raw);
    if let Ok(v) = serde_json::from_str::<crate::model::OneShotReply>(&cleaned) {
        return v;
    }
    // Sometimes the object is embedded in prose; grab the outermost braces.
    if let (Some(a), Some(b)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if b > a {
            if let Ok(v) = serde_json::from_str::<crate::model::OneShotReply>(&cleaned[a..=b]) {
                return v;
            }
        }
    }
    crate::model::OneShotReply { source: cleaned, translation: String::new() }
}

/// Split on paragraph / sentence boundaries so each API call stays small.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;

    for line in text.split_inclusive('\n') {
        let line_len = line.chars().count();
        if count > 0 && count + line_len > limit {
            chunks.push(std::mem::take(&mut current).trim_end().to_string());
            count = 0;
        }
        if line_len > limit {
            // A single monster line: hard-split it.
            let mut piece = String::new();
            for ch in line.chars() {
                piece.push(ch);
                if piece.chars().count() >= limit {
                    if !current.is_empty() {
                        chunks.push(std::mem::take(&mut current).trim_end().to_string());
                    }
                    chunks.push(std::mem::take(&mut piece));
                }
            }
            current.push_str(&piece);
            count = current.chars().count();
            continue;
        }
        current.push_str(line);
        count += line_len;
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim_end().to_string());
    }
    chunks.retain(|c| !c.trim().is_empty());
    chunks
}

/* -------------------------------------------------------------- prompts */

/// Endpoint of the local OCR service, validated.
fn local_ocr_url(cfg: &OcrConfig) -> Result<String> {
    let base = cfg.base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        bail!("还没有填写本地 OCR 服务地址（例如 http://127.0.0.1:1224）。");
    }
    if !base.starts_with("http://") && !base.starts_with("https://") {
        bail!("本地 OCR 服务地址要以 http:// 或 https:// 开头，例如 http://127.0.0.1:1224。");
    }
    Ok(format!("{base}/api/ocr"))
}

/// Umi-OCR answers with `{"code": 100, "data": …}`: 100 = 识别成功,
/// 101 = 没有文字, 其他为错误（原因在 data 里）。
fn parse_local_ocr(body: &str) -> Result<String> {
    let parsed: Value = serde_json::from_str(body).map_err(|err| {
        anyhow!(
            "本地 OCR 服务返回的不是预期的 JSON（{err}）：{}",
            truncate(body, 160)
        )
    })?;
    let code = parsed.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let data = parsed.get("data").cloned().unwrap_or(Value::Null);
    match code {
        100 => {
            let text = match data {
                Value::String(text) => text,
                Value::Array(items) => join_strings(&items, "text"),
                other => other.to_string(),
            };
            Ok(unescape_ocr_newlines(text))
        }
        101 => Ok(String::new()),
        _ => {
            let reason = match data {
                Value::String(text) => text,
                other => other.to_string(),
            };
            bail!(
                "本地 OCR 服务返回错误（code {code}）：{}",
                truncate(&reason, 200)
            )
        }
    }
}

/// Some services hand paragraph breaks back as a literal backslash-n. Rewrite
/// only when there is no real newline that could be lost.
fn unescape_ocr_newlines(text: String) -> String {
    if !text.contains('\n') && text.contains("\\n") {
        return text.replace("\\n", "\n");
    }
    text
}

fn ocr_system(custom: &str) -> String {
    if !custom.trim().is_empty() {
        return custom.to_string();
    }
    "你是一个高精度的 OCR 引擎。请提取图片中的所有可见文字。\n\
     规则：\n\
     1. 原样输出文字，不要翻译、不要解释、不要总结。\n\
     2. 保留原有的换行与分段；表格用制表符分隔列。\n\
     3. 忽略纯装饰性元素（图标、边框、背景图案）。\n\
     4. 若图片中没有文字，只输出：[NO_TEXT]\n\
     5. 直接输出文字，不要使用代码块包裹。"
        .to_string()
}

fn one_shot_system(target: &str) -> String {
    let phrase = target_phrase(target);
    format!(
        "你是一个截图翻译引擎。请识别图片中的文字，并翻译成{phrase}。\n\
         只输出一个 JSON 对象，不要输出任何其他内容，格式为：\n\
         {{\"source\":\"识别出的原文\",\"translation\":\"译文\"}}\n\
         要求：原文保留换行；译文准确、自然、符合目标语言习惯；\
         专有名词、代码、命令、URL、变量名保持原样不翻译。"
    )
}

fn translate_system(custom: &str, target: &str) -> String {
    if !custom.trim().is_empty() {
        return format!("{custom}\n\n目标语言：{target}");
    }
    format!(
        "你是一个专业的翻译引擎。请将用户提供的内容翻译成{target}。\n\
         规则：\n\
         1. 只输出译文，不要输出原文、解释、注释或任何前言后语。\n\
         2. 保留原有的换行、缩进与段落结构。\n\
         3. 代码、命令、文件路径、URL、变量名、专有名词保持原样。\n\
         4. 若输入已经是{target}，原样输出即可。\n\
         5. 不要使用代码块包裹译文。"
    )
}

/// Human-readable target language name used inside prompts.
/// True when the configured target asks the pipeline to decide for us.
pub fn is_auto_target(target: &str) -> bool {
    let wanted = target.trim();
    wanted.is_empty() || wanted.eq_ignore_ascii_case("auto")
}

/// Share of tokens that must be Han before we treat a snippet as Chinese.
const HAN_RATIO: f32 = 0.15;

/// Cheap guess at the language of a recognised snippet.
///
/// Every engine detects the *source* language itself (we send `auto`); this
/// exists only so [`resolve_target`] can pick a sensible target, which a
/// character-class count handles well enough — kana and hangul first, then Han.
pub fn detect_source(text: &str) -> &'static str {
    let mut han = 0usize;
    let mut kana = 0usize;
    let mut hangul = 0usize;
    let mut letters = 0usize;
    for ch in text.chars() {
        let code = ch as u32;
        if (0x3040..=0x30FF).contains(&code) {
            kana += 1;
        } else if (0x1100..=0x11FF).contains(&code) || (0xAC00..=0xD7AF).contains(&code) {
            hangul += 1;
        } else if (0x3400..=0x4DBF).contains(&code)
            || (0x4E00..=0x9FFF).contains(&code)
            || (0xF900..=0xFAFF).contains(&code)
            || (0x20000..=0x2FA1F).contains(&code)
        {
            han += 1;
        }
        if ch.is_alphanumeric() {
            letters += 1;
        }
    }
    if kana > 0 {
        return "ja";
    }
    if hangul > 0 {
        return "ko";
    }
    if han > 0 && (letters == 0 || han as f32 / letters as f32 >= HAN_RATIO) {
        return "zh-CN";
    }
    "en"
}

/// Resolve the target language for one run.
///
/// A concrete tag passes straight through. `auto` (or empty) means the smart
/// direction: English when the text is already Chinese, Chinese otherwise — so
/// 中译英 and 英译中 both work without touching the settings.
pub fn resolve_target(text: &str, target: &str) -> String {
    let wanted = target.trim();
    if !is_auto_target(wanted) {
        return wanted.to_string();
    }
    match detect_source(text) {
        "zh-CN" | "zh-TW" => "en".to_string(),
        _ => "zh-CN".to_string(),
    }
}

/// Human phrase for a target tag, safe for prompts when the tag is `auto`.
fn target_phrase(target: &str) -> String {
    if is_auto_target(target) {
        "自动判断的语言（识别出的原文主要是中文就译成英文，否则译成中文）".to_string()
    } else {
        language_name(target)
    }
}

pub fn language_name(tag: &str) -> String {
    match tag.trim() {
        "zh-CN" | "zh" | "zh-Hans" | "zh-SG" => "简体中文",
        "zh-TW" | "zh-Hant" | "zh-HK" => "繁体中文",
        "en" | "en-US" | "en-GB" => "英文",
        "ja" | "ja-JP" => "日文",
        "ko" | "ko-KR" => "韩文",
        "fr" => "法文",
        "de" => "德文",
        "es" => "西班牙文",
        "ru" => "俄文",
        "pt" | "pt-BR" => "葡萄牙文",
        "it" => "意大利文",
        "ar" => "阿拉伯文",
        "th" => "泰文",
        "vi" => "越南文",
        other => other,
    }
    .to_string()
}

/// DeepL expects upper-cased language codes with its own quirks.
pub fn deepl_target(tag: &str) -> String {
    match tag.trim() {
        "zh-CN" | "zh" | "zh-Hans" | "zh-SG" => "ZH".to_string(),
        "zh-TW" | "zh-Hant" | "zh-HK" => "ZH-HANT".to_string(),
        "en" | "en-US" | "en-GB" => "EN-US".to_string(),
        "pt" | "pt-BR" => "PT-BR".to_string(),
        // Anything else: DeepL wants the bare, upper-cased primary subtag.
        other => other.split('-').next().unwrap_or(other).to_uppercase(),
    }
}

/* ------------------------------------------- provider-specific plumbing */

/// Human-facing name of a provider, used on the result badge and in the
/// "connection ok" message.
pub fn provider_label(provider: &str) -> &'static str {
    match provider {
        "openai" => "大模型",
        "deepl" => "DeepL",
        "microsoft" => "微软翻译",
        "google" => "谷歌翻译",
        "youdao" => "有道翻译",
        "baidu" => "百度翻译",
        "tencent" => "腾讯翻译",
        "winocr" => "Windows 离线 OCR",
        "vision" => "视觉模型",
        "none" => "不翻译",
        _ => "翻译服务",
    }
}

/// Fold a language tag to `zh-CN` / `zh-TW` / a bare primary subtag.
fn base_tag(tag: &str) -> String {
    let t = tag.trim();
    match t {
        "zh" | "zh-Hans" | "zh-CN" | "zh-SG" => "zh-CN".to_string(),
        "zh-TW" | "zh-Hant" | "zh-HK" => "zh-TW".to_string(),
        "en" | "en-US" | "en-GB" => "en".to_string(),
        other => other.split('-').next().unwrap_or(other).to_string(),
    }
}

pub fn youdao_target(tag: &str) -> String {
    match base_tag(tag).as_str() {
        "zh-CN" => "zh-CHS".to_string(),
        "zh-TW" => "zh-CHT".to_string(),
        other => other.to_string(),
    }
}

pub fn baidu_target(tag: &str) -> String {
    match base_tag(tag).as_str() {
        "zh-CN" => "zh".to_string(),
        "zh-TW" => "cht".to_string(),
        "ja" => "jp".to_string(),
        "ko" => "kor".to_string(),
        "fr" => "fra".to_string(),
        "es" => "spa".to_string(),
        "ar" => "ara".to_string(),
        "vi" => "vie".to_string(),
        other => other.to_string(),
    }
}

pub fn tencent_target(tag: &str) -> String {
    match base_tag(tag).as_str() {
        "zh-CN" => "zh".to_string(),
        other => other.to_string(),
    }
}

pub fn microsoft_target(tag: &str) -> String {
    match base_tag(tag).as_str() {
        "zh-CN" => "zh-Hans".to_string(),
        "zh-TW" => "zh-Hant".to_string(),
        other => other.to_string(),
    }
}

pub fn google_target(tag: &str) -> String {
    base_tag(tag)
}

/// Google's translation endpoint, resolved from the configured base URL.
///
/// Kept beside `google_target` so the "no credential in the URL" rule has one
/// place to look: the key travels in the `X-Goog-Api-Key` header instead.
pub fn google_url(cfg: &TranslateConfig) -> String {
    if cfg.base_url.trim().is_empty() {
        "https://translation.googleapis.com/language/translate/v2".to_string()
    } else {
        cfg.base_url.trim().trim_end_matches('/').to_string()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex_encode(&hasher.finalize())
}

fn md5_hex(data: &str) -> String {
    format!("{:x}", md5::compute(data.as_bytes()))
}

/// md5 of raw bytes — Baidu signs the picture file itself, not a string.
fn md5_hex_bytes(data: &[u8]) -> String {
    format!("{:x}", md5::compute(data))
}

fn hmac_sha256(key: &[u8], message: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encode a value for an `application/x-www-form-urlencoded` body.
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Join an array of plain strings (`key` empty) or of objects holding `key`.
fn join_strings(items: &[Value], key: &str) -> String {
    items
        .iter()
        .filter_map(|item| {
            if key.is_empty() {
                item.as_str()
            } else {
                item.get(key).and_then(|v| v.as_str())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Google escapes its translations as HTML entities.
fn unescape_html(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Youdao v3 signs a trimmed form of the query.
/// The textual form of an error code — engines send it as a string or a number.
fn code_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        other => other.to_string().trim_matches('"').trim().to_string(),
    }
}

/// Text segments wherever an engine buried them: `fanyi-api` answers at the top
/// level, the 智能云 flavour nests them in `result`, and some LLM replies in
/// `data`. Returns the first list that exists.
fn trans_segments(value: &Value) -> Option<&Vec<Value>> {
    for path in ["trans_result", "result.trans_result", "data.trans_result"] {
        let mut node = value;
        let mut walked = true;
        for key in path.split('.') {
            match node.get(key) {
                Some(next) => node = next,
                None => {
                    walked = false;
                    break;
                }
            }
        }
        if walked {
            if let Some(list) = node.as_array() {
                return Some(list);
            }
        }
    }
    None
}

/// The error code of a Baidu answer with their success codes normalised away:
/// the classic API omits `error_code`, the LLM endpoint answers `52000`, and
/// older docs use `0`.
fn baidu_error_code(value: &Value) -> Option<String> {
    let code = match value.get("error_code") {
        Some(code) => code_text(code),
        None => return None,
    };
    if code.is_empty() || code == "0" || code == "52000" {
        None
    } else {
        Some(code)
    }
}

/// Funnel for every "HTTP 200 but unusable answer": the body always ends up in
/// the log, because the log is the only place a user can see what came back.
fn empty_reply(what: &str, body: &str) -> anyhow::Error {
    log::error!("{what} 响应体：{}", truncate(body.trim(), 1200));
    match extract_error_message(body) {
        Some(message) => anyhow!("{what}没有返回译文（接口提示：{message}）。"),
        None => anyhow!("{what}返回了空译文。"),
    }
}

/// Pull the finished translation out of Youdao's LLM (SSE) answer. Events look
/// like `data: {"code":"0","data":{"transFull":"…","langType":"…"}}`; with
/// `streamType=full` the last event carries the whole translation. A plain JSON
/// body (some deployments) is accepted too.
fn parse_youdao_llm(body: &str) -> Result<String> {
    let mut last = String::new();
    for line in body.lines() {
        let line = line.trim();
        let json = match line.strip_prefix("data:") {
            Some(rest) => rest.trim(),
            None => line,
        };
        if json.is_empty() || json == "[DONE]" || !json.starts_with('{') {
            continue;
        }
        let parsed: Value = match serde_json::from_str(json) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let code = parsed.get("code").map(code_text).unwrap_or_default();
        if !code.is_empty() && code != "0" {
            bail!("有道大模型翻译报错 {code}：{}", youdao_error(&code));
        }
        let data = parsed.get("data").unwrap_or(&parsed);
        // `transFull` is the finished text; `transIncre` only the newest bit.
        for key in ["transFull", "transResult", "translation", "result"] {
            if let Some(text) = data.get(key).and_then(|t| t.as_str()) {
                if !text.trim().is_empty() {
                    last = text.trim().to_string();
                }
                break;
            }
        }
    }
    if last.trim().is_empty() {
        return Err(empty_reply("有道大模型翻译", body));
    }
    Ok(last)
}

fn youdao_input(query: &str) -> String {
    let chars: Vec<char> = query.chars().collect();
    if chars.len() <= 20 {
        query.to_string()
    } else {
        let head: String = chars.iter().take(10).collect();
        let tail: String = chars.iter().skip(chars.len() - 10).collect();
        format!("{head}{}{tail}", chars.len())
    }
}

fn youdao_error(code: &str) -> &'static str {
    match code {
        "101" => "缺少必填参数",
        "102" => "不支持的语言类型",
        "103" => "翻译文本过长",
        "104" => "不支持的 API 类型",
        "105" => "不支持的签名类型",
        "106" => "不支持的响应类型",
        "107" => "不支持的传输加密类型",
        "108" => "应用 ID 无效：请核对有道智云控制台里的应用 ID",
        "110" => "无相关服务的有效实例",
        "111" => "开发者账号无效",
        "113" => "待翻译内容不能为空",
        "201" => "解密失败，请检查加密方式",
        "202" => "签名校验失败：请核对应用密钥（appSecret）",
        "203" => "访问 IP 不在可访问列表内",
        "401" => "账户已欠费",
        "411" => "访问频率受限",
        _ => "请核对应用 ID 与应用密钥",
    }
}

fn baidu_error(code: &str) -> &'static str {
    match code {
        "52001" => "请求超时，请重试",
        "52002" => "系统错误，请重试",
        "52003" => "未授权用户：该 APP ID 没有开通这个服务（大模型翻译、图片翻译都需要在百度控制台单独申请）。开通后重试；若只想用图片翻译，也可以改回本机识别",
        "54000" => "必填参数为空",
        "54001" => "签名或 API Key 错误：请核对凭据（签名方式用应用密钥，API Key 方式用控制台创建的 Key）",
        "54003" => "访问频率受限",
        "54004" => "账户余额不足",
        "54005" => "长文本请求过于频繁",
        "58000" => "客户端 IP 非法：请在百度控制台配置 IP 白名单",
        "58001" => "译文语言方向不支持",
        "58002" => "服务当前已关闭",
        "90107" => "认证未通过或未生效",
        _ => "请核对 APP ID 与密钥",
    }
}

fn host_of(url: &str) -> String {
    let without_scheme = url.split("://").last().unwrap_or(url);
    without_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

/// `YYYY-MM-DD` in UTC (Tencent's TC3 credential scope needs it).
fn utc_date(epoch_seconds: u64) -> String {
    let days = (epoch_seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year_base = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year_base + 1 } else { year_base };
    format!("{year:04}-{month:02}-{day:02}")
}

/// TC3-HMAC-SHA256 as specified by Tencent Cloud.
fn tc3_authorization(
    secret_id: &str,
    secret_key: &str,
    host: &str,
    payload: &str,
    timestamp: u64,
    date: &str,
) -> String {
    let signed_headers = "content-type;host";
    let canonical_headers = format!("content-type:application/json; charset=utf-8\nhost:{host}\n");
    let canonical_request = format!(
        "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{}",
        sha256_hex(payload)
    );
    let credential_scope = format!("{date}/tmt/tc3_request");
    let string_to_sign = format!(
        "TC3-HMAC-SHA256\n{timestamp}\n{credential_scope}\n{}",
        sha256_hex(&canonical_request)
    );
    let secret_date = hmac_sha256(format!("TC3{secret_key}").as_bytes(), date);
    let secret_service = hmac_sha256(&secret_date, "tmt");
    let secret_signing = hmac_sha256(&secret_service, "tc3_request");
    let signature = hex_encode(&hmac_sha256(&secret_signing, &string_to_sign));
    format!(
        "TC3-HMAC-SHA256 Credential={secret_id}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_thinking_and_fences() {
        assert_eq!(clean_model_text("<think>hmm</think>你好"), "你好");
        assert_eq!(clean_model_text("```\n你好\n```"), "你好");
        assert_eq!(clean_model_text("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(clean_model_text("译文： 你好"), "你好");
        assert_eq!(clean_model_text("\"你好\""), "你好");
        // An unclosed thinking block must not leak its contents.
        assert_eq!(clean_model_text("<think>secret 你好"), "");
    }

    #[test]
    fn language_codes_are_mapped_per_provider() {
        assert_eq!(youdao_target("zh-CN"), "zh-CHS");
        assert_eq!(youdao_target("zh-TW"), "zh-CHT");
        assert_eq!(youdao_target("en-US"), "en");
        assert_eq!(baidu_target("zh-CN"), "zh");
        assert_eq!(baidu_target("zh-TW"), "cht");
        assert_eq!(baidu_target("ja"), "jp");
        assert_eq!(baidu_target("ko"), "kor");
        assert_eq!(tencent_target("zh-CN"), "zh");
        assert_eq!(tencent_target("zh-TW"), "zh-TW");
        assert_eq!(microsoft_target("zh-CN"), "zh-Hans");
        assert_eq!(microsoft_target("zh-TW"), "zh-Hant");
        assert_eq!(google_target("zh-CN"), "zh-CN");
        assert_eq!(google_target("de"), "de");
    }

    #[test]
    fn hashes_match_known_vectors() {
        // A typo in the plumbing then shows up here instead of as a 403 from
        // the provider.
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex_encode(&hmac_sha256(b"key", "The quick brown fox jumps over the lazy dog")),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn tencent_signature_is_deterministic() {
        let a = tc3_authorization(
            "AKID",
            "SECRET",
            "tmt.tencentcloudapi.com",
            "{}",
            1_700_000_000,
            "2023-11-14",
        );
        let b = tc3_authorization(
            "AKID",
            "SECRET",
            "tmt.tencentcloudapi.com",
            "{}",
            1_700_000_000,
            "2023-11-14",
        );
        assert_eq!(a, b);
        assert!(a.starts_with("TC3-HMAC-SHA256 Credential=AKID/2023-11-14/tmt/tc3_request"));
        assert!(a.contains("SignedHeaders=content-type;host"));
        assert!(a.contains("Signature="));
    }

    #[test]
    fn utc_date_uses_civil_calendar() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_700_000_000), "2023-11-14");
        assert_eq!(utc_date(1_702_987_200), "2023-12-19");
    }

    #[test]
    fn youdao_signs_a_trimmed_query() {
        assert_eq!(youdao_input("short"), "short");
        let long_query = "一二三四五六七八九十一二三四五六七八九十一";
        assert_eq!(
            youdao_input(long_query),
            "一二三四五六七八九十21二三四五六七八九十一"
        );
    }

    #[test]
    fn bodies_are_form_encoded() {
        assert_eq!(url_encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(form_encode(&[("q", "你好")]), "q=%E4%BD%A0%E5%A5%BD");
        assert_eq!(unescape_html("&quot;hi&quot; &amp; &lt;b&gt;"), "\"hi\" & <b>");
        assert_eq!(host_of("https://tmt.tencentcloudapi.com/"), "tmt.tencentcloudapi.com");
        assert_eq!(host_of("api.cognitive.microsofttranslator.com"), "api.cognitive.microsofttranslator.com");
    }

    #[test]
    fn one_shot_tolerates_prose_wrappers() {
        let r = parse_one_shot("好的：\n{\"source\":\"Hello\",\"translation\":\"你好\"}");
        assert_eq!(r.source, "Hello");
        assert_eq!(r.translation, "你好");

        // A non-JSON answer degrades to "source only" rather than failing.
        let r = parse_one_shot("Hello world");
        assert_eq!(r.source, "Hello world");
        assert!(r.translation.is_empty());
    }

    #[test]
    fn chunking_respects_limit_and_loses_nothing() {
        let text = "line one\nline two\nline three\nline four";
        let chunks = chunk_text(text, 20);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 20, "chunk too long: {c:?}");
        }
        let joined: String = chunks.join("\n");
        assert!(joined.contains("line one") && joined.contains("line four"));

        // A single oversized line is hard-split, not dropped.
        let long = "x".repeat(50);
        let chunks = chunk_text(&long, 10);
        assert_eq!(chunks.iter().map(|c| c.chars().count()).sum::<usize>(), 50);
    }

    #[test]
    fn local_ocr_replies_are_decoded() {
        // Umi-OCR: 100 = 识别成功, 101 = 没有文字, 其它 = 出错（原因在 data）。
        assert_eq!(
            parse_local_ocr(r#"{"code":100,"data":"你好"}"#).unwrap(),
            "你好"
        );
        assert_eq!(parse_local_ocr(r#"{"code":101,"data":"no text"}"#).unwrap(), "");
        // A dict-style answer still yields text (joined per line).
        assert_eq!(
            parse_local_ocr(r#"{"code":100,"data":[{"text":"a"},{"text":"b"}]}"#).unwrap(),
            "a\nb"
        );
        // Escaped newlines that some builds hand back are unescaped.
        assert_eq!(
            parse_local_ocr(r#"{"code":100,"data":"a\\nb"}"#).unwrap(),
            "a\nb"
        );
        assert!(parse_local_ocr(r#"{"code":9,"data":"boom"}"#).is_err());
        assert!(parse_local_ocr("<html>nope</html>").is_err());
    }

    #[test]
    fn local_ocr_address_is_validated() {
        let mut cfg = OcrConfig::default();
        cfg.base_url = "127.0.0.1:1224".into();
        assert!(local_ocr_url(&cfg).is_err());
        cfg.base_url = "http://127.0.0.1:1224/".into();
        assert_eq!(
            local_ocr_url(&cfg).unwrap(),
            "http://127.0.0.1:1224/api/ocr"
        );
    }

    #[test]
    fn short_text_is_a_single_chunk() {
        assert_eq!(chunk_text("  hi  ", 100), vec!["hi".to_string()]);
        assert!(chunk_text("   ", 100).is_empty());
    }

    #[test]
    fn http_errors_classify_retryability() {
        let e = anyhow!(explain_http_error(429, r#"{"error":{"message":"slow down"}}"#));
        assert!(is_retryable(&e));
        assert!(format!("{e}").contains("slow down"));

        let e = anyhow!(explain_http_error(400, r#"{"error":{"message":"bad model"}}"#));
        assert!(!is_retryable(&e));
    }

    #[test]
    fn overloaded_service_is_retryable_and_actionable() {
        let e = anyhow!(explain_http_error(503, "Service temporarily overloaded"));
        assert!(is_retryable(&e), "503 should be retried");
        assert!(format!("{e}").contains("过载"));
    }

    #[test]
    fn missing_credentials_are_not_retried() {
        // A real 401 from an OpenAI-compatible gateway.
        let e = anyhow!(explain_http_error(
            401,
            r#"{"error":{"message":"invalid api key"}}"#
        ));
        assert!(!is_retryable(&e));
        assert!(format!("{e}").contains("API Key"));

        // NVIDIA NIM reports a missing Authorization header as a 500.
        let e = anyhow!(explain_http_error(
            500,
            "Missing request extension: Extension of type `headers::common::authorization::Authorization<Bearer>` was not found."
        ));
        assert!(!is_retryable(&e), "a missing key must not be retried");
        assert!(format!("{e}").contains("API Key"));
    }

    #[test]
    fn language_and_deepl_mapping() {
        assert_eq!(language_name("zh-CN"), "简体中文");
        assert_eq!(deepl_target("zh-CN"), "ZH");
        assert_eq!(deepl_target("zh-TW"), "ZH-HANT");
        assert_eq!(deepl_target("en-US"), "EN-US");
        assert_eq!(deepl_target("ja"), "JA");
    }

    #[test]
    fn param_rejection_detection_ignores_retryable() {
        assert!(is_param_rejection(&anyhow!(
            "请求被拒绝： HTTP 400: unsupported parameter response_format"
        )));
        assert!(!is_param_rejection(&anyhow!("RETRY: 触发限流（429）")));
    }

    #[test]
    fn baidu_segments_are_found_wherever_they_are_buried() {
        // The classic API answers at the top level…
        let flat = serde_json::json!({ "trans_result": [{ "src": "a", "dst": "甲" }] });
        assert_eq!(
            trans_segments(&flat).map(|list| join_strings(list, "dst")),
            Some("甲".to_string())
        );
        // …the 智能云 flavour nests them in `result`…
        let nested = serde_json::json!({ "result": { "trans_result": [{ "dst": "乙" }] } });
        assert_eq!(
            trans_segments(&nested).map(|list| join_strings(list, "dst")),
            Some("乙".to_string())
        );
        // …and some replies wrap them in `data`.
        let data = serde_json::json!({ "data": { "trans_result": [{ "dst": "丙" }] } });
        assert_eq!(
            trans_segments(&data).map(|list| join_strings(list, "dst")),
            Some("丙".to_string())
        );
        assert!(trans_segments(&serde_json::json!({ "code": "0" })).is_none());
    }

    #[test]
    fn baidu_success_codes_are_not_mistaken_for_errors() {
        // No code at all (the classic API), 0 and 52000 (the LLM endpoint) are
        // all successes; anything else has to surface as an error.
        assert!(baidu_error_code(&serde_json::json!({})).is_none());
        assert!(baidu_error_code(&serde_json::json!({ "error_code": "0" })).is_none());
        assert!(baidu_error_code(&serde_json::json!({ "error_code": "52000" })).is_none());
        assert!(baidu_error_code(&serde_json::json!({ "error_code": 52000 })).is_none());
        assert_eq!(
            baidu_error_code(&serde_json::json!({ "error_code": "54001" })),
            Some("54001".to_string())
        );
        assert_eq!(
            baidu_error_code(&serde_json::json!({ "error_code": 54004 })),
            Some("54004".to_string())
        );
    }

    #[test]
    fn baidu_llm_bearer_rejections_are_readable() {
        // Real reply shape from `POST /ait/api/aiTextTranslate` with a Bearer
        // header: HTTP 200, a Baidu error code and `data: null`.
        let body = serde_json::json!({
            "error_code": "54001",
            "error_msg": "invalid token",
            "data": null
        });
        assert_eq!(baidu_error_code(&body), Some("54001".to_string()));
        assert!(trans_segments(&body).is_none());
        assert_eq!(baidu_error("54001"), "签名或 API Key 错误：请核对凭据（签名方式用应用密钥，API Key 方式用控制台创建的 Key）");
    }

    #[test]
    fn youdao_llm_stream_yields_the_finished_translation() {
        // `streamType=full` repeats the whole text in every event, so the last
        // non-empty `transFull` wins.
        let stream = concat!(
            "data: {\"code\":\"0\",\"data\":{\"transFull\":\"你\",\"langType\":\"en2zh-CHS\"}}\n\n",
            "data: {\"code\":\"0\",\"data\":{\"transFull\":\"你好\"}}\n\n",
            "data: [DONE]\n",
        );
        assert_eq!(parse_youdao_llm(stream).unwrap(), "你好");
        // A plain JSON body works as well.
        assert_eq!(
            parse_youdao_llm("{\"code\":\"0\",\"data\":{\"transResult\":\"直接返回\"}}").unwrap(),
            "直接返回"
        );
        // An error code becomes the error message.
        let failed = "data: {\"code\":\"207\",\"data\":{}}";
        assert!(parse_youdao_llm(failed)
            .unwrap_err()
            .to_string()
            .contains("207"));
        // Nothing usable is an error too.
        assert!(parse_youdao_llm("data: {\"code\":\"0\",\"data\":{}}").is_err());
    }

    #[test]
    fn smart_target_follows_the_source_language() {
        // Chinese in → English out; everything else → Chinese.
        assert_eq!(resolve_target("你好世界", "auto"), "en");
        assert_eq!(resolve_target("拉赫玛尼诺夫 7", "auto"), "en");
        assert_eq!(resolve_target("使用 Windows 系统 的同学", "auto"), "en");
        assert_eq!(resolve_target("Hello world", "auto"), "zh-CN");
        assert_eq!(resolve_target("Bonjour tout le monde", "auto"), "zh-CN");
        assert_eq!(resolve_target("こんにちは", "auto"), "zh-CN");
        assert_eq!(resolve_target("안녕하세요", "auto"), "zh-CN");
        // An empty target means the same as `auto`.
        assert_eq!(resolve_target("你好", ""), "en");
        // A concrete choice is never overridden.
        assert_eq!(resolve_target("你好", "ja"), "ja");
        assert_eq!(resolve_target("Hello", "zh-TW"), "zh-TW");
        // Detection on its own.
        assert_eq!(detect_source("Hello"), "en");
        assert_eq!(detect_source("中文"), "zh-CN");
        assert_eq!(detect_source("ひらがな"), "ja");
        assert_eq!(detect_source("한글"), "ko");
        assert!(is_auto_target("AUTO"));
        assert!(!is_auto_target("en"));
    }

    #[test]
    fn log_lines_never_carry_a_credential() {
        // Google used to pass its key as `?key=…`; whatever an engine does now,
        // anything headed for the log goes through `scrub_url` first.
        assert_eq!(
            scrub_url("https://translate.googleapis.com/v2?key=AIzaSySECRET&alt=json"),
            "https://translate.googleapis.com/v2?key=***&alt=json"
        );
        assert_eq!(
            scrub_url("https://api.deepl.com/v2/translate"),
            "https://api.deepl.com/v2/translate"
        );
        // `sign` and `token` are the query credentials the guide names; an
        // ordinary parameter keeps its value so a log line stays useful.
        assert_eq!(
            scrub_url("https://x.test/?sign=abc&token=t&text=hi"),
            "https://x.test/?sign=***&token=***&text=hi"
        );
        // A display string that embeds the URL is scrubbed where it is embedded.
        let url = "https://api.example.test/v1?api_key=sk-live";
        let message = format!("error sending request for {url}: connection refused");
        let scrubbed = redact_url_in(&message, url);
        assert!(!scrubbed.contains("sk-live"));
        assert!(scrubbed.contains("api_key=***"));
    }

    #[test]
    fn google_never_puts_the_key_in_the_url() {
        let mut cfg = TranslateConfig::default();
        assert_eq!(
            google_url(&cfg),
            "https://translation.googleapis.com/language/translate/v2"
        );
        // A custom endpoint is used verbatim, minus a trailing slash.
        cfg.base_url = "https://proxy.example.test/translate/".into();
        assert_eq!(google_url(&cfg), "https://proxy.example.test/translate");
        assert!(!google_url(&cfg).contains("key="));
    }

    #[test]
    fn scoped_credentials_follow_the_active_vendor() {
        let mut cfg = TranslateConfig::default();
        // Vendor slots only apply to the OpenAI-compatible channel.
        cfg.provider = "openai".into();
        cfg.vendor = "deepseek".into();
        cfg.api_key = "stale-flat-key".into();
        cfg.profiles.insert(
            "deepseek".into(),
            crate::config::TranslateProfile {
                api_key: "vendor-key".into(),
                ..Default::default()
            },
        );
        // The flat fields are replaced by the active vendor's own set, never
        // merged with whatever happened to be lying around.
        assert_eq!(scoped_translate(&cfg).api_key, "vendor-key");
    }
}
