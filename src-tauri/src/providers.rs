//! Model / API access: OpenAI-compatible chat completions, DeepL, the cloud
//! translation services (Microsoft / Google / Youdao / Baidu / Tencent) and the
//! one-step picture translation of Youdao, Baidu and Tencent.
//!
//! Everything that talks to the network lives here so the pipeline stays a pure
//! orchestration layer. Requests carry an explicit timeout, retry on the
//! retryable class of failures (429 / 5xx / transport), and strip model
//! "thinking" leakage from the reply.

use crate::config::{OcrConfig, TranslateConfig};
use crate::model::ConnectionCheck;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_ATTEMPTS: u32 = 3;
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
        require_key(&cfg.api_key, "OCR / 视觉模型")?;
        if cfg.model.trim().is_empty() {
            bail!("尚未配置 OCR 模型名称。");
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
        // The engines below read the flat credential fields. Fold the selected
        // engine's own profile into them so no engine can ever pick up another
        // engine's key, region or endpoint.
        let profile = cfg.active();
        let tier = profile.tier(&cfg.provider);
        let mut scoped = cfg.clone();
        scoped.base_url = profile.base_url;
        scoped.api_key = profile.api_key;
        scoped.api_secret = profile.api_secret;
        scoped.region = profile.region;
        scoped.model = profile.model;
        scoped.model_tier = tier;
        let cfg = &scoped;
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
        let profile = cfg.active();
        let tier = profile.tier(&cfg.provider);
        let mut scoped = cfg.clone();
        scoped.base_url = profile.base_url;
        scoped.api_key = profile.api_key;
        scoped.api_secret = profile.api_secret;
        scoped.region = profile.region;
        scoped.model = profile.model;
        scoped.model_tier = tier;
        let cfg = &scoped;
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

    async fn translate_chat(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "翻译")?;
        if cfg.model.trim().is_empty() {
            bail!("尚未配置翻译模型名称。");
        }
        let target = language_name(&cfg.target_lang);
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
                bail!("DeepL 返回了空译文（请检查目标语言代码与套餐类型）。");
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
        // 1 selects the LLM engine, 0 the general one.
        let option = if cfg.model_tier.trim() == "llm" {
            "1"
        } else {
            "0"
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
        if let Some(code) = parsed.get("errorCode").and_then(|c| c.as_str()) {
            if code != "0" {
                bail!("有道图片翻译报错 {code}：{}", youdao_error(code));
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
        Ok((collect("context"), collect("tranContent")))
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
            let code = error
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string());
            if code != "0" {
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
        Ok((text_of("sumSrc", "src"), text_of("sumDst", "dst")))
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
                bail!("微软翻译返回了空译文：请检查密钥、区域（Region）与目标语言是否匹配。");
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Google Cloud Translation v2, API key in the query string.
    async fn google(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "谷歌翻译")?;
        let base = if cfg.base_url.trim().is_empty() {
            "https://translation.googleapis.com/language/translate/v2".to_string()
        } else {
            cfg.base_url.trim().trim_end_matches('/').to_string()
        };
        let url = format!("{base}?key={}", cfg.api_key.trim());
        let target = google_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let body = json!({ "q": [chunk], "target": target, "format": "text" });
            let resp = self
                .send_with_retry(|| self.client.post(&url).json(&body), "谷歌翻译")
                .await?;
            let parsed: Value = serde_json::from_str(&resp).context("谷歌翻译返回的不是 JSON")?;
            let translated = parsed
                .pointer("/data/translations")
                .and_then(|t| t.as_array())
                .map(|arr| join_strings(arr, "translatedText"))
                .map(|s| unescape_html(&s))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                bail!("谷歌翻译返回了空译文：请确认该 API Key 已开通 Cloud Translation API。");
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Youdao Zhiyun text translation. v3 signature over a form body.
    async fn youdao(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
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
            let code = parsed
                .get("errorCode")
                .and_then(|c| c.as_str())
                .unwrap_or_default();
            if code != "0" {
                bail!("有道翻译报错 {code}：{}", youdao_error(code));
            }
            let translated = parsed
                .get("translation")
                .and_then(|t| t.as_array())
                .map(|arr| join_strings(arr, ""))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                bail!("有道翻译返回了空译文。");
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Baidu's LLM translation (`ait/api/aiTextTranslate`): same credentials and
    /// md5 signature as the standard API, a JSON body instead of a form.
    async fn baidu_llm(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        let endpoint = if cfg.base_url.trim().is_empty() {
            "https://fanyi-api.baidu.com/ait/api/aiTextTranslate".to_string()
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
            let body = json!({
                "appid": app_id,
                "from": "auto",
                "to": to,
                "q": chunk,
                "salt": salt,
                "sign": sign,
            });
            let resp = self
                .send_with_retry(
                    || self.client.post(&endpoint).json(&body),
                    "百度大模型翻译",
                )
                .await?;
            let parsed: Value =
                serde_json::from_str(&resp).context("百度大模型翻译返回的内容不是 JSON")?;
            if let Some(code) = parsed.get("error_code").and_then(|c| c.as_str()) {
                bail!("百度大模型翻译报错 {code}：{}", baidu_error(code));
            }
            let translated = parsed
                .get("trans_result")
                .and_then(|t| t.as_array())
                .map(|arr| join_strings(arr, "dst"))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                bail!("百度大模型翻译返回了空译文。");
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /// Baidu Translate open API. md5 signature over a form body.
    async fn baidu(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "百度翻译")?;
        if cfg.api_secret.trim().is_empty() {
            bail!("百度翻译需要两项凭据：APP ID 与密钥。");
        }
        // 「大模型翻译」 is a separate service on its own endpoint; it shares the
        // app id + md5 signature contract, so only the URL and the body change.
        if cfg.model_tier.trim() == "llm" {
            return self.baidu_llm(cfg, text).await;
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
            if let Some(code) = parsed.get("error_code").and_then(|c| c.as_str()) {
                bail!("百度翻译报错 {code}：{}", baidu_error(code));
            }
            let translated = parsed
                .get("trans_result")
                .and_then(|t| t.as_array())
                .map(|arr| join_strings(arr, "dst"))
                .unwrap_or_default();
            if translated.trim().is_empty() {
                bail!("百度翻译返回了空译文。");
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
                bail!("腾讯翻译返回了空译文。");
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

        let parsed: Value = serde_json::from_str(&raw).context("响应不是合法 JSON")?;
        let message = parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .ok_or_else(|| anyhow!("响应缺少 choices[0].message：{}", truncate(&raw, 400)))?;

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
            let outcome = match build().send().await {
                Err(err) => Err(anyhow!(classify_transport(&err))),
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        return Ok(body);
                    }
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
        format!("RETRY: 网络请求失败：{err}")
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
    format!(
        "你是一个截图翻译引擎。请识别图片中的文字，并翻译成{target}。\n\
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
        "52003" => "未授权用户：该 APP ID 没有开通「图片翻译」服务。图片翻译需要在百度控制台单独申请；也可以关闭这个渠道的「一步翻译图片」，改用本机识别 + 文本翻译",
        "54000" => "必填参数为空",
        "54001" => "签名错误：请核对密钥",
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
}
