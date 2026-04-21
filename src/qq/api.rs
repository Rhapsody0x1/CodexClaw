use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use futures_util::{
    StreamExt,
    stream::{self, TryStreamExt},
};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::Mutex,
};
use tracing::{info, warn};

use crate::config::QqConfig;
use crate::qq::types::GatewayInfo;

const CHUNKED_UPLOAD_THRESHOLD_BYTES: u64 = 5 * 1024 * 1024;
const MD5_10M_SIZE: u64 = 10_002_432;
const DEFAULT_CHUNK_UPLOAD_CONCURRENCY: usize = 1;
const MAX_CHUNK_UPLOAD_CONCURRENCY: usize = 10;
const MAX_PART_FINISH_RETRY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const PART_UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const PART_UPLOAD_MAX_RETRIES: u32 = 2;
const COMPLETE_UPLOAD_MAX_RETRIES: u32 = 2;
const PART_FINISH_MAX_RETRIES: u32 = 2;
const PART_FINISH_RETRYABLE_CODE: i64 = 40093001;
const PART_FINISH_RETRYABLE_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_PART_FINISH_RETRY_TIMEOUT: Duration = Duration::from_secs(2 * 60);

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

#[derive(Debug, Clone, Deserialize)]
struct UploadFileResponse {
    file_info: String,
}

#[derive(Debug, Clone, Serialize)]
struct UploadPrepareBody<'a> {
    file_type: u8,
    file_name: &'a str,
    file_size: u64,
    md5: &'a str,
    sha1: &'a str,
    md5_10m: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct UploadPartFinishBody<'a> {
    upload_id: &'a str,
    part_index: u64,
    block_size: u64,
    md5: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct UploadCompleteBody<'a> {
    upload_id: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
struct UploadPrepareResponse {
    upload_id: String,
    #[serde(deserialize_with = "deserialize_u64_value")]
    block_size: u64,
    parts: Vec<UploadPart>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_value")]
    concurrency: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_value")]
    retry_timeout: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct UploadPart {
    #[serde(deserialize_with = "deserialize_u64_value")]
    index: u64,
    presigned_url: String,
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

#[derive(Debug, Clone)]
struct FileHashes {
    md5: String,
    sha1: String,
    md5_10m: String,
}

#[derive(Debug, Deserialize)]
struct QqErrorBody {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    msg: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("QQ API request failed with status {status}: {message}")]
struct QqApiError {
    status: StatusCode,
    code: Option<i64>,
    message: String,
    raw: String,
}

impl QqApiError {
    fn from_response(status: StatusCode, raw: String) -> Self {
        let parsed = serde_json::from_str::<QqErrorBody>(&raw).ok();
        let message = parsed
            .as_ref()
            .and_then(|value| value.message.as_deref().or(value.msg.as_deref()))
            .unwrap_or(raw.as_str())
            .to_string();
        Self {
            status,
            code: parsed.and_then(|value| value.code),
            message,
            raw,
        }
    }
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

    pub async fn upload_file(
        &self,
        openid: &str,
        path: &Path,
        file_type: u8,
        file_name_override: Option<&str>,
    ) -> Result<String> {
        info!(
            openid = %openid,
            path = %path.display(),
            file_type,
            "uploading qq media"
        );
        let file_name = normalized_upload_file_name(path, file_name_override)?;
        let file_size = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("failed to stat {}", path.display()))?
            .len();
        ensure_upload_size_allowed(file_type, file_size)?;

        if should_use_chunked_upload(file_size) {
            return self
                .upload_file_chunked(openid, path, file_type, &file_name, file_size)
                .await;
        }

        self.upload_file_direct(path, openid, file_type, &file_name)
            .await
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

    async fn upload_file_direct(
        &self,
        path: &Path,
        openid: &str,
        file_type: u8,
        file_name: &str,
    ) -> Result<String> {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let body = UploadFileBody {
            file_type,
            file_data: &encoded,
            file_name,
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

    async fn upload_file_chunked(
        &self,
        openid: &str,
        path: &Path,
        file_type: u8,
        file_name: &str,
        file_size: u64,
    ) -> Result<String> {
        let hashes = self.compute_file_hashes(path, file_size).await?;
        let prepare = self
            .prepare_chunked_upload(openid, file_type, file_name, file_size, &hashes)
            .await?;
        anyhow::ensure!(
            !prepare.parts.is_empty(),
            "QQ upload_prepare returned no upload parts"
        );

        let block_size = prepare.block_size;
        let retry_timeout = prepare
            .retry_timeout
            .map(Duration::from_secs)
            .map(|timeout| timeout.min(MAX_PART_FINISH_RETRY_TIMEOUT));
        let concurrency = prepare
            .concurrency
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_CHUNK_UPLOAD_CONCURRENCY)
            .clamp(1, MAX_CHUNK_UPLOAD_CONCURRENCY);
        let upload_id = prepare.upload_id.clone();

        stream::iter(prepare.parts.into_iter().map(|part| {
            let upload_id = upload_id.clone();
            async move {
                self.upload_single_part(
                    openid,
                    path,
                    file_size,
                    block_size,
                    &upload_id,
                    &part,
                    retry_timeout,
                )
                .await
            }
        }))
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;

        let response = self.complete_chunked_upload(openid, &upload_id).await?;
        Ok(response.file_info)
    }

    async fn prepare_chunked_upload(
        &self,
        openid: &str,
        file_type: u8,
        file_name: &str,
        file_size: u64,
        hashes: &FileHashes,
    ) -> Result<UploadPrepareResponse> {
        let body = UploadPrepareBody {
            file_type,
            file_name,
            file_size,
            md5: &hashes.md5,
            sha1: &hashes.sha1,
            md5_10m: &hashes.md5_10m,
        };
        self.post_json(
            format!(
                "{}/v2/users/{openid}/upload_prepare",
                self.config.api_base_url
            ),
            &body,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_single_part(
        &self,
        openid: &str,
        path: &Path,
        file_size: u64,
        block_size: u64,
        upload_id: &str,
        part: &UploadPart,
        retry_timeout: Option<Duration>,
    ) -> Result<()> {
        let offset = part.index.saturating_sub(1).saturating_mul(block_size);
        let length = block_size.min(file_size.saturating_sub(offset));
        let bytes = read_file_chunk(path, offset, length).await?;
        let part_md5 = format!("{:x}", md5::compute(&bytes));
        self.put_presigned_part(&part.presigned_url, bytes).await?;
        self.finish_chunked_part(
            openid,
            upload_id,
            part.index,
            length,
            &part_md5,
            retry_timeout,
        )
        .await
    }

    async fn finish_chunked_part(
        &self,
        openid: &str,
        upload_id: &str,
        part_index: u64,
        block_size: u64,
        md5: &str,
        retry_timeout: Option<Duration>,
    ) -> Result<()> {
        let body = UploadPartFinishBody {
            upload_id,
            part_index,
            block_size,
            md5,
        };
        let url = format!(
            "{}/v2/users/{openid}/upload_part_finish",
            self.config.api_base_url
        );
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..=PART_FINISH_MAX_RETRIES {
            match self
                .post_json::<serde_json::Value, _>(url.clone(), &body)
                .await
            {
                Ok(_) => return Ok(()),
                Err(err) => {
                    if qq_api_error_code(&err) == Some(PART_FINISH_RETRYABLE_CODE) {
                        return self
                            .persistent_retry_part_finish(url, &body, retry_timeout)
                            .await;
                    }
                    last_error = Some(err);
                    if attempt < PART_FINISH_MAX_RETRIES {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("failed to finish chunk upload part")))
    }

    async fn persistent_retry_part_finish(
        &self,
        url: String,
        body: &UploadPartFinishBody<'_>,
        retry_timeout: Option<Duration>,
    ) -> Result<()> {
        let timeout = retry_timeout.unwrap_or(DEFAULT_PART_FINISH_RETRY_TIMEOUT);
        let deadline = Instant::now() + timeout;
        let mut attempts = 0u32;

        loop {
            match self
                .post_json::<serde_json::Value, _>(url.clone(), body)
                .await
            {
                Ok(_) => return Ok(()),
                Err(err) => {
                    if qq_api_error_code(&err) != Some(PART_FINISH_RETRYABLE_CODE) {
                        return Err(err);
                    }
                    attempts = attempts.saturating_add(1);
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(anyhow!(
                            "upload_part_finish 持续重试超时（{} 秒，{} 次重试）",
                            timeout.as_secs(),
                            attempts
                        ));
                    }
                    tokio::time::sleep(PART_FINISH_RETRYABLE_INTERVAL.min(deadline - now)).await;
                }
            }
        }
    }

    async fn complete_chunked_upload(
        &self,
        openid: &str,
        upload_id: &str,
    ) -> Result<UploadFileResponse> {
        let body = UploadCompleteBody { upload_id };
        let url = format!("{}/v2/users/{openid}/files", self.config.api_base_url);
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..=COMPLETE_UPLOAD_MAX_RETRIES {
            match self
                .post_json::<UploadFileResponse, _>(url.clone(), &body)
                .await
            {
                Ok(response) => return Ok(response),
                Err(err) => {
                    last_error = Some(err);
                    if attempt < COMPLETE_UPLOAD_MAX_RETRIES {
                        tokio::time::sleep(Duration::from_secs(2 * (1 << attempt))).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("failed to complete chunked upload")))
    }

    async fn compute_file_hashes(&self, path: &Path, file_size: u64) -> Result<FileHashes> {
        let mut file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        let mut md5_ctx = md5::Context::new();
        let mut md5_10m_ctx = md5::Context::new();
        let mut sha1_ctx = Sha1::new();
        let mut remaining_10m = MD5_10M_SIZE;
        let mut buffer = vec![0u8; 64 * 1024];

        loop {
            let read = file
                .read(&mut buffer)
                .await
                .with_context(|| format!("failed to read {}", path.display()))?;
            if read == 0 {
                break;
            }
            let chunk = &buffer[..read];
            md5_ctx.consume(chunk);
            sha1_ctx.update(chunk);
            if remaining_10m > 0 {
                let take = remaining_10m.min(read as u64) as usize;
                md5_10m_ctx.consume(&chunk[..take]);
                remaining_10m = remaining_10m.saturating_sub(take as u64);
            }
        }

        let md5 = format!("{:x}", md5_ctx.compute());
        let md5_10m = if file_size <= MD5_10M_SIZE {
            md5.clone()
        } else {
            format!("{:x}", md5_10m_ctx.compute())
        };
        let sha1 = format!("{:x}", sha1_ctx.finalize());

        Ok(FileHashes { md5, sha1, md5_10m })
    }

    async fn put_presigned_part(&self, presigned_url: &str, bytes: Vec<u8>) -> Result<()> {
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..=PART_UPLOAD_MAX_RETRIES {
            let response = self
                .client
                .put(presigned_url)
                .timeout(PART_UPLOAD_TIMEOUT)
                .header("Content-Length", bytes.len())
                .body(bytes.clone())
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    let status = response.status();
                    let raw = response.text().await.unwrap_or_default();
                    last_error = Some(anyhow!(
                        "QQ presigned upload failed with status {status}: {raw}"
                    ));
                }
                Err(err) => {
                    last_error = Some(err.into());
                }
            }
            if attempt < PART_UPLOAD_MAX_RETRIES {
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("failed to upload chunk to presigned url")))
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
            return Err(QqApiError::from_response(status, raw).into());
        }
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

fn should_use_chunked_upload(file_size: u64) -> bool {
    file_size >= CHUNKED_UPLOAD_THRESHOLD_BYTES
}

fn normalized_upload_file_name(path: &Path, override_name: Option<&str>) -> Result<String> {
    let raw = override_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| path.file_name().and_then(|name| name.to_str()))
        .ok_or_else(|| anyhow!("invalid file name for {}", path.display()))?;
    let name = Path::new(raw)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\') {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim();
    anyhow::ensure!(
        !trimmed.is_empty(),
        "invalid file name for {}",
        path.display()
    );
    Ok(trimmed.to_string())
}

fn ensure_upload_size_allowed(file_type: u8, file_size: u64) -> Result<()> {
    let limit = match file_type {
        1 => 30 * 1024 * 1024,
        2 => 100 * 1024 * 1024,
        3 => 20 * 1024 * 1024,
        4 => 100 * 1024 * 1024,
        _ => 100 * 1024 * 1024,
    };
    anyhow::ensure!(
        file_size <= limit,
        "QQ 文件过大：{file_size} bytes exceeds limit {limit} bytes for file_type {file_type}"
    );
    Ok(())
}

async fn read_file_chunk(path: &Path, offset: u64, length: u64) -> Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .with_context(|| format!("failed to seek {}", path.display()))?;
    let mut buffer = vec![0u8; length as usize];
    let mut read = 0usize;
    while read < buffer.len() {
        let n = file
            .read(&mut buffer[read..])
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        if n == 0 {
            break;
        }
        read += n;
    }
    buffer.truncate(read);
    Ok(buffer)
}

fn qq_api_error_code(err: &anyhow::Error) -> Option<i64> {
    err.downcast_ref::<QqApiError>()
        .and_then(|value| value.code)
}

fn default_expires_in() -> u64 {
    7200
}

fn deserialize_expires_in<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    parse_u64_value::<D::Error>(value)
}

fn deserialize_u64_value<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    parse_u64_value::<D::Error>(value)
}

fn deserialize_optional_u64_value<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    value.map(parse_u64_value::<D::Error>).transpose()
}

fn parse_u64_value<E>(value: Value) -> std::result::Result<u64, E>
where
    E: serde::de::Error,
{
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| E::custom("numeric value is not u64")),
        Value::String(text) => text
            .parse::<u64>()
            .map_err(|err| E::custom(format!("invalid numeric string: {err}"))),
        other => Err(E::custom(format!("invalid numeric value: {other}"))),
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, method, path},
    };

    use super::{
        CHUNKED_UPLOAD_THRESHOLD_BYTES, QqApiClient, QqConfig, estimate_text_chunk_count,
        normalized_upload_file_name, should_use_chunked_upload,
    };

    #[test]
    fn chunked_upload_threshold_matches_large_files() {
        assert!(!should_use_chunked_upload(
            CHUNKED_UPLOAD_THRESHOLD_BYTES - 1
        ));
        assert!(should_use_chunked_upload(CHUNKED_UPLOAD_THRESHOLD_BYTES));
    }

    #[test]
    fn normalizes_override_file_name() {
        let name =
            normalized_upload_file_name(std::path::Path::new("/tmp/report.bin"), Some("../a.txt"))
                .unwrap();
        assert_eq!(name, "a.txt");
    }

    #[test]
    fn estimates_text_chunks() {
        let text = format!("{}\n{}", "a".repeat(3000), "b".repeat(3000));
        assert_eq!(estimate_text_chunk_count(&text), 2);
    }

    #[tokio::test]
    async fn uploads_large_file_with_chunked_flow() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "token-1",
                "expires_in": 7200
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/users/u1/upload_prepare"))
            .and(body_partial_json(serde_json::json!({
                "file_type": 4,
                "file_name": "report.bin"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "upload_id": "upload-1",
                "block_size": 3145728,
                "parts": [
                    {"index": 1, "presigned_url": format!("{}/upload/1", server.uri())},
                    {"index": 2, "presigned_url": format!("{}/upload/2", server.uri())}
                ],
                "concurrency": 2,
                "retry_timeout": 60
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/1"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/2"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/users/u1/upload_part_finish"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/users/u1/files"))
            .and(body_partial_json(serde_json::json!({
                "upload_id": "upload-1"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_info": "file-info"
            })))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("report.bin");
        fs::write(
            &path,
            vec![b'x'; (CHUNKED_UPLOAD_THRESHOLD_BYTES + 128) as usize],
        )
        .unwrap();

        let client = QqApiClient::new(QqConfig {
            app_id: "app".into(),
            app_secret: "secret".into(),
            api_base_url: server.uri(),
            token_url: format!("{}/token", server.uri()),
        })
        .unwrap();

        let file_info = client
            .upload_file("u1", &path, 4, Some("report.bin"))
            .await
            .unwrap();
        assert_eq!(file_info, "file-info");
    }
}
