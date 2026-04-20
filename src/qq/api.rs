use std::{collections::HashMap, path::Path, time::Duration};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::QqConfig;
use crate::qq::types::GatewayInfo;

#[derive(Debug, Clone)]
pub struct QqApiClient {
    client: Client,
    config: QqConfig,
    token_cache: std::sync::Arc<Mutex<Option<CachedToken>>>,
    msg_seq: std::sync::Arc<Mutex<HashMap<String, u32>>>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    expires_at: std::time::Instant,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: String,
    #[serde(
        default = "default_expires_in",
        deserialize_with = "deserialize_expires_in"
    )]
    expires_in: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct MessageReference<'a> {
    message_id: &'a str,
}

#[derive(Debug, Serialize)]
struct SendTextBody<'a> {
    content: &'a str,
    msg_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg_id: Option<&'a str>,
    msg_seq: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_reference: Option<MessageReference<'a>>,
}

#[derive(Debug, Serialize)]
struct SendMarkdownBody<'a> {
    msg_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg_id: Option<&'a str>,
    msg_seq: u32,
    markdown: MarkdownPayload<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_reference: Option<MessageReference<'a>>,
}

#[derive(Debug, Serialize)]
struct MarkdownPayload<'a> {
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct UploadFileBody<'a> {
    file_type: u8,
    file_data: &'a str,
    file_name: &'a str,
    srv_send_msg: bool,
}

#[derive(Debug, Deserialize)]
struct UploadFileResponse {
    file_info: String,
}

#[derive(Debug, Serialize)]
struct SendMediaBody<'a> {
    msg_type: u8,
    msg_id: &'a str,
    msg_seq: u32,
    media: MediaFileInfo<'a>,
}

#[derive(Debug, Serialize)]
struct MediaFileInfo<'a> {
    file_info: &'a str,
}

impl QqApiClient {
    pub fn new(config: QqConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self {
            client,
            config,
            token_cache: std::sync::Arc::new(Mutex::new(None)),
            msg_seq: std::sync::Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn send_text(
        &self,
        openid: &str,
        message_id: &str,
        text: &str,
        reference_message_id: Option<&str>,
    ) -> Result<()> {
        self.send_text_inner(openid, Some(message_id), text, reference_message_id)
            .await
    }

    async fn send_text_inner(
        &self,
        openid: &str,
        message_id: Option<&str>,
        text: &str,
        reference_message_id: Option<&str>,
    ) -> Result<()> {
        info!(
            openid = %openid,
            reply_to = message_id.unwrap_or(""),
            reference = reference_message_id.unwrap_or(""),
            text_len = text.len(),
            "sending qq text message"
        );
        for chunk in split_text(text, 4500) {
            let msg_seq = self.next_msg_seq(message_id.unwrap_or(openid)).await;
            let message_reference =
                reference_message_id.map(|value| MessageReference { message_id: value });
            let markdown_body = SendMarkdownBody {
                msg_type: 2,
                msg_id: message_id,
                msg_seq,
                markdown: MarkdownPayload { content: &chunk },
                message_reference,
            };
            let url = format!("{}/v2/users/{openid}/messages", self.config.api_base_url);
            match self
                .post_json::<serde_json::Value, _>(url.clone(), &markdown_body)
                .await
            {
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        error = %err,
                        "qq markdown message rejected; falling back to plain text"
                    );
                    let text_body = SendTextBody {
                        content: &chunk,
                        msg_type: 0,
                        msg_id: message_id,
                        msg_seq,
                        message_reference,
                    };
                    let _: serde_json::Value = self.post_json(url, &text_body).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn upload_file(&self, openid: &str, path: &Path, file_type: u8) -> Result<String> {
        info!(
            openid = %openid,
            path = %path.display(),
            file_type,
            "uploading qq media"
        );
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("invalid file name for {}", path.display()))?;
        let body = UploadFileBody {
            file_type,
            file_data: &encoded,
            file_name: filename,
            srv_send_msg: false,
        };
        let response: UploadFileResponse = self
            .post_json(
                format!("{}/v2/users/{openid}/files", self.config.api_base_url),
                &body,
            )
            .await?;
        Ok(response.file_info)
    }

    pub async fn send_media(&self, openid: &str, message_id: &str, file_info: &str) -> Result<()> {
        info!(
            openid = %openid,
            reply_to = %message_id,
            "sending qq media message"
        );
        let body = SendMediaBody {
            msg_type: 7,
            msg_id: message_id,
            msg_seq: self.next_msg_seq(message_id).await,
            media: MediaFileInfo { file_info },
        };
        let _: serde_json::Value = self
            .post_json(
                format!("{}/v2/users/{openid}/messages", self.config.api_base_url),
                &body,
            )
            .await?;
        Ok(())
    }

    pub async fn get_gateway_url(&self) -> Result<String> {
        let response: GatewayInfo = self
            .request_json(
                Method::GET,
                format!("{}/gateway", self.config.api_base_url),
                Option::<&serde_json::Value>::None,
            )
            .await?;
        Ok(response.url)
    }

    pub async fn download_attachment(&self, source_url: &str, destination: &Path) -> Result<()> {
        let normalized_url = if source_url.starts_with("//") {
            format!("https:{source_url}")
        } else {
            source_url.to_string()
        };
        let response = self
            .client
            .get(&normalized_url)
            .send()
            .await
            .with_context(|| format!("failed to download {normalized_url}"))?;
        let status = response.status();
        let bytes = response.bytes().await?;
        anyhow::ensure!(status.is_success(), "download failed with status {status}");
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(destination, bytes).await?;
        info!(
            source_url = %normalized_url,
            destination = %destination.display(),
            "downloaded incoming attachment"
        );
        Ok(())
    }

    pub async fn get_access_token(&self) -> Result<String> {
        let mut cache = self.token_cache.lock().await;
        if let Some(current) = cache.as_ref() {
            if current.expires_at > std::time::Instant::now() + Duration::from_secs(60) {
                return Ok(current.value.clone());
            }
        }
        let response = self
            .client
            .post(&self.config.token_url)
            .json(&serde_json::json!({
                "appId": self.config.app_id,
                "clientSecret": self.config.app_secret,
            }))
            .send()
            .await
            .context("failed to request QQ access token")?;
        let status = response.status();
        let body = response.text().await?;
        anyhow::ensure!(
            status.is_success(),
            "QQ access token request failed with status {status}: {body}"
        );
        info!("retrieved qq access token successfully");
        let parsed: AccessTokenResponse = serde_json::from_str(&body)
            .with_context(|| format!("invalid QQ access token response: {body}"))?;
        *cache = Some(CachedToken {
            value: parsed.access_token.clone(),
            expires_at: std::time::Instant::now() + Duration::from_secs(parsed.expires_in),
        });
        Ok(parsed.access_token)
    }

    pub async fn invalidate_access_token(&self) {
        let mut cache = self.token_cache.lock().await;
        *cache = None;
    }

    async fn post_json<T, B>(&self, url: String, body: &B) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        self.request_json(Method::POST, url, Some(body)).await
    }

    async fn request_json<T, B>(&self, method: Method, url: String, body: Option<&B>) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        let token = self.get_access_token().await?;
        let mut request = self
            .client
            .request(method, url)
            .header("Authorization", format!("QQBot {token}"))
            .header("X-Union-Appid", &self.config.app_id);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        let raw = response.text().await?;
        if !status.is_success() {
            warn!("qq api request failed with status {}: {}", status, raw);
        }
        anyhow::ensure!(
            status.is_success(),
            "QQ API request failed with status {status}: {raw}"
        );
        if raw.is_empty() && StatusCode::NO_CONTENT == status {
            return serde_json::from_str("null").context("failed to parse empty response");
        }
        serde_json::from_str(&raw).with_context(|| format!("invalid QQ API response: {raw}"))
    }

    async fn next_msg_seq(&self, msg_id: &str) -> u32 {
        let mut map = self.msg_seq.lock().await;
        let seq = map.entry(msg_id.to_string()).or_insert(0);
        *seq += 1;
        *seq
    }
}

fn default_expires_in() -> u64 {
    7200
}

fn deserialize_expires_in<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;

    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| D::Error::custom("expires_in number is not u64")),
        serde_json::Value::String(text) => text
            .parse::<u64>()
            .map_err(|err| D::Error::custom(format!("invalid expires_in string: {err}"))),
        other => Err(D::Error::custom(format!(
            "invalid expires_in value: {other}"
        ))),
    }
}

fn split_text(text: &str, limit: usize) -> Vec<String> {
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let candidate_len = current.chars().count() + line.chars().count() + 1;
        if !current.is_empty() && candidate_len > limit {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

pub fn estimate_text_chunk_count(text: &str) -> usize {
    split_text(text, 4500).len()
}
