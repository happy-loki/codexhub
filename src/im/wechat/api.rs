use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aes::Aes128;
use anyhow::{Context, Result, anyhow};
use axum::http::{HeaderMap, HeaderName, HeaderValue, header::CONTENT_TYPE};
use base64::{Engine as _, engine::general_purpose};
use cipher::{BlockEncryptMut, KeyInit, block_padding::Pkcs7};
use ecb::Encryptor as Aes128EcbEnc;
use reqwest::{Client, Url};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::chain_log;

use super::types::{
    DEFAULT_WECHAT_API_BASE, WechatGetUpdatesResponse, WechatQrCodeResponse,
    WechatQrStatusResponse, WechatSettings,
};

const QR_LONG_POLL_TIMEOUT_MS: u64 = 35_000;
const LONG_POLL_TIMEOUT_MS: u64 = 5_000;
const API_TIMEOUT_MS: u64 = 15_000;
const CONFIG_TIMEOUT_MS: u64 = 10_000;
const ILINK_APP_ID: &str = "bot";
const MESSAGE_TYPE_BOT: i64 = 2;
const MESSAGE_STATE_FINISH: i64 = 2;
const MESSAGE_ITEM_TYPE_TEXT: i64 = 1;
const MESSAGE_ITEM_TYPE_IMAGE: i64 = 2;
const WECHAT_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

static WECHAT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct WechatApi {
    settings: WechatSettings,
    client: Client,
}

impl WechatApi {
    pub fn new(settings: WechatSettings) -> Self {
        Self {
            settings,
            client: Client::new(),
        }
    }

    pub fn settings(&self) -> &WechatSettings {
        &self.settings
    }

    pub fn is_configured(&self) -> bool {
        self.settings.is_configured()
    }

    pub async fn start_qr_login(&self, local_tokens: &[String]) -> Result<WechatQrCodeResponse> {
        let bot_type = self.settings.bot_type();
        let endpoint = format!(
            "ilink/bot/get_bot_qrcode?bot_type={}",
            url_escape(&bot_type)
        );
        self.post_json_at_base(
            DEFAULT_WECHAT_API_BASE,
            &endpoint,
            json!({ "local_token_list": local_tokens }),
            None,
            Duration::from_millis(API_TIMEOUT_MS),
            "wechat_qr_start",
        )
        .await
    }

    pub async fn poll_qr_status(
        &self,
        base_url: &str,
        qrcode: &str,
        verify_code: Option<&str>,
    ) -> Result<WechatQrStatusResponse> {
        let mut endpoint = format!("ilink/bot/get_qrcode_status?qrcode={}", url_escape(qrcode));
        if let Some(verify_code) = verify_code.map(str::trim).filter(|value| !value.is_empty()) {
            endpoint.push_str("&verify_code=");
            endpoint.push_str(&url_escape(verify_code));
        }
        match self
            .get_json_at_base(
                base_url,
                &endpoint,
                Duration::from_millis(QR_LONG_POLL_TIMEOUT_MS),
                "wechat_qr_poll",
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(err) if is_timeout_error(&err) => Ok(WechatQrStatusResponse {
                status: "wait".to_string(),
                bot_token: None,
                ilink_bot_id: None,
                baseurl: None,
                ilink_user_id: None,
                redirect_host: None,
            }),
            Err(err) => Err(err),
        }
    }

    pub async fn get_updates(
        &self,
        get_updates_buf: &str,
        timeout_ms: u64,
    ) -> Result<WechatGetUpdatesResponse> {
        if !self.is_configured() {
            return Err(anyhow!("wechat bot_token is empty"));
        }
        match self
            .post_json(
                "ilink/bot/getupdates",
                json!({
                    "get_updates_buf": get_updates_buf,
                    "base_info": base_info(),
                }),
                Duration::from_millis(timeout_ms.max(1)),
                "wechat_get_updates",
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(err) if is_timeout_error(&err) => Ok(WechatGetUpdatesResponse {
                ret: Some(0),
                msgs: Some(Vec::new()),
                get_updates_buf: Some(get_updates_buf.to_string()),
                ..Default::default()
            }),
            Err(err) => Err(err),
        }
    }

    pub async fn send_text(
        &self,
        to_user_id: &str,
        context_token: Option<&str>,
        text: &str,
    ) -> Result<String> {
        if !self.is_configured() {
            return Err(anyhow!("wechat bot_token is empty"));
        }
        let client_id = next_client_id("codex-remote-gateway-wechat");
        let mut msg = json!({
            "from_user_id": "",
            "to_user_id": to_user_id,
            "client_id": client_id,
            "message_type": MESSAGE_TYPE_BOT,
            "message_state": MESSAGE_STATE_FINISH,
            "item_list": [{
                "type": MESSAGE_ITEM_TYPE_TEXT,
                "text_item": { "text": text },
            }],
        });
        if let Some(context_token) = context_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            msg["context_token"] = json!(context_token);
        }
        let _: Value = self
            .post_json(
                "ilink/bot/sendmessage",
                json!({
                    "msg": msg,
                    "base_info": base_info(),
                }),
                Duration::from_millis(API_TIMEOUT_MS),
                "wechat_send_message",
            )
            .await?;
        Ok(client_id)
    }

    pub async fn send_image_file(
        &self,
        to_user_id: &str,
        context_token: Option<&str>,
        path: &Path,
    ) -> Result<String> {
        if !self.is_configured() {
            return Err(anyhow!("wechat bot_token is empty"));
        }
        let context_token = context_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("wechat image message context_token is missing"))?;
        let uploaded = self.upload_image(to_user_id, path).await?;
        self.send_uploaded_image(to_user_id, context_token, &uploaded)
            .await
    }

    pub async fn notify_start(&self) -> Result<()> {
        if !self.is_configured() {
            return Ok(());
        }
        let _: Value = self
            .post_json(
                "ilink/bot/msg/notifystart",
                json!({ "base_info": base_info() }),
                Duration::from_millis(CONFIG_TIMEOUT_MS),
                "wechat_notify_start",
            )
            .await?;
        Ok(())
    }

    async fn upload_image(&self, to_user_id: &str, path: &Path) -> Result<UploadedImage> {
        let plaintext = fs::read(path)
            .with_context(|| format!("wechat image file read failed: {}", path.display()))?;
        let raw_size = plaintext.len();
        let raw_md5 = format!("{:x}", md5::compute(&plaintext));
        let file_size_ciphertext = aes_ecb_padded_size(raw_size);
        let filekey = Uuid::new_v4().simple().to_string();
        let aes_key = *Uuid::new_v4().as_bytes();
        let aes_key_hex = hex::encode(aes_key);
        let upload_param = self
            .get_upload_url(
                to_user_id,
                &filekey,
                raw_size,
                &raw_md5,
                file_size_ciphertext,
                &aes_key_hex,
            )
            .await?;
        let ciphertext = encrypt_aes_128_ecb_pkcs7(&plaintext, &aes_key)?;
        let encrypt_query_param =
            upload_encrypted_media_to_cdn(&upload_param, &filekey, ciphertext)
                .await
                .context("wechat image cdn upload failed")?;
        Ok(UploadedImage {
            encrypt_query_param,
            aes_key_hex,
            file_size_ciphertext,
        })
    }

    async fn get_upload_url(
        &self,
        to_user_id: &str,
        filekey: &str,
        raw_size: usize,
        raw_md5: &str,
        file_size_ciphertext: usize,
        aes_key_hex: &str,
    ) -> Result<String> {
        let response: UploadUrlResponse = self
            .post_json(
                "ilink/bot/getuploadurl",
                json!({
                    "filekey": filekey,
                    "media_type": 1,
                    "to_user_id": to_user_id,
                    "rawsize": raw_size,
                    "rawfilemd5": raw_md5,
                    "filesize": file_size_ciphertext,
                    "no_need_thumb": true,
                    "aeskey": aes_key_hex,
                    "base_info": base_info(),
                }),
                Duration::from_millis(20_000),
                "wechat_get_upload_url",
            )
            .await?;
        response
            .upload_param
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("wechat getuploadurl missing upload_param"))
    }

    async fn send_uploaded_image(
        &self,
        to_user_id: &str,
        context_token: &str,
        uploaded: &UploadedImage,
    ) -> Result<String> {
        let client_id = next_client_id("codex-remote-gateway-wechat-image");
        let msg = ImageMessage {
            from_user_id: "",
            to_user_id,
            client_id: client_id.clone(),
            message_type: MESSAGE_TYPE_BOT,
            message_state: MESSAGE_STATE_FINISH,
            context_token,
            item_list: vec![ImageMessageItem {
                item_type: MESSAGE_ITEM_TYPE_IMAGE,
                image_item: ImagePayload {
                    media: ImageMediaPayload {
                        encrypt_query_param: &uploaded.encrypt_query_param,
                        aes_key: general_purpose::STANDARD.encode(uploaded.aes_key_hex.as_bytes()),
                        encrypt_type: 1,
                    },
                    mid_size: uploaded.file_size_ciphertext,
                },
            }],
        };
        let _: Value = self
            .post_json(
                "ilink/bot/sendmessage",
                json!({
                    "msg": msg,
                    "base_info": base_info(),
                }),
                Duration::from_millis(20_000),
                "wechat_send_image",
            )
            .await?;
        Ok(client_id)
    }

    async fn post_json<T>(
        &self,
        endpoint: &str,
        body: Value,
        timeout: Duration,
        label: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let base_url = self.settings.api_base_url();
        self.post_json_at_base(
            &base_url,
            endpoint,
            body,
            self.settings.token(),
            timeout,
            label,
        )
        .await
    }

    async fn post_json_at_base<T>(
        &self,
        base_url: &str,
        endpoint: &str,
        body: Value,
        token: Option<&str>,
        timeout: Duration,
        label: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = endpoint_url(base_url, endpoint)?;
        let body_text = serde_json::to_string(&body)?;
        chain_log::write_line(format!(
            "[wechat_api] event=request method=POST label={} url={} body_len={}",
            label,
            redact_url(url.as_str()),
            body_text.len()
        ));
        let response = self
            .client
            .post(url)
            .headers(build_headers(token)?)
            .body(body_text)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("wechat api {label} request failed"))?;
        decode_response(response, label).await
    }

    async fn get_json_at_base<T>(
        &self,
        base_url: &str,
        endpoint: &str,
        timeout: Duration,
        label: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = endpoint_url(base_url, endpoint)?;
        chain_log::write_line(format!(
            "[wechat_api] event=request method=GET label={} url={}",
            label,
            redact_url(url.as_str())
        ));
        let response = self
            .client
            .get(url)
            .headers(build_common_headers()?)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("wechat api {label} request failed"))?;
        decode_response(response, label).await
    }
}

#[derive(Debug, serde::Deserialize)]
struct UploadUrlResponse {
    upload_param: Option<String>,
}

#[derive(Debug)]
struct UploadedImage {
    encrypt_query_param: String,
    aes_key_hex: String,
    file_size_ciphertext: usize,
}

#[derive(Serialize)]
struct ImageMessage<'a> {
    from_user_id: &'a str,
    to_user_id: &'a str,
    client_id: String,
    message_type: i64,
    message_state: i64,
    context_token: &'a str,
    item_list: Vec<ImageMessageItem<'a>>,
}

#[derive(Serialize)]
struct ImageMessageItem<'a> {
    #[serde(rename = "type")]
    item_type: i64,
    image_item: ImagePayload<'a>,
}

#[derive(Serialize)]
struct ImagePayload<'a> {
    media: ImageMediaPayload<'a>,
    mid_size: usize,
}

#[derive(Serialize)]
struct ImageMediaPayload<'a> {
    encrypt_query_param: &'a str,
    aes_key: String,
    encrypt_type: i64,
}

fn build_cdn_upload_url(upload_param: &str, filekey: &str) -> String {
    format!(
        "{}/upload?encrypted_query_param={}&filekey={}",
        WECHAT_CDN_BASE_URL,
        url::form_urlencoded::byte_serialize(upload_param.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(filekey.as_bytes()).collect::<String>()
    )
}

fn aes_ecb_padded_size(plaintext_size: usize) -> usize {
    ((plaintext_size + 1).div_ceil(16)) * 16
}

fn encrypt_aes_128_ecb_pkcs7(plaintext: &[u8], key: &[u8; 16]) -> Result<Vec<u8>> {
    let mut buf = plaintext.to_vec();
    let msg_len = buf.len();
    let padded_len = aes_ecb_padded_size(msg_len);
    buf.resize(padded_len, 0);
    let encrypted = Aes128EcbEnc::<Aes128>::new(key.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buf, msg_len)
        .map_err(|err| anyhow!("wechat image encrypt failed: {err}"))?;
    Ok(encrypted.to_vec())
}

async fn upload_encrypted_media_to_cdn(
    upload_param: &str,
    filekey: &str,
    ciphertext: Vec<u8>,
) -> Result<String> {
    let url = build_cdn_upload_url(upload_param, filekey);
    let response = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .post(url)
        .header(CONTENT_TYPE, "application/octet-stream")
        .body(ciphertext)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let err_header = response
            .headers()
            .get("x-error-message")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "wechat cdn upload failed: status={} err_header={} body={}",
            status,
            err_header,
            truncate_log(&body, 300)
        ));
    }
    response
        .headers()
        .get("x-encrypted-param")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("wechat cdn upload missing x-encrypted-param"))
}

async fn decode_response<T>(response: reqwest::Response, label: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| format!("wechat api {label} response body read failed"))?;
    chain_log::write_line(format!(
        "[wechat_api] event=response label={} status={} body_len={}{}",
        label,
        status.as_u16(),
        text.len(),
        response_preview_suffix(label, &text)
    ));
    if !status.is_success() {
        return Err(anyhow!(
            "wechat api {label} failed: status={} body={}",
            status,
            truncate_log(&text, 300)
        ));
    }
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "wechat api {label} response decode failed: {}",
            truncate_log(&text, 300)
        )
    })?;
    ensure_business_success(label, &value)?;
    serde_json::from_value(value).with_context(|| {
        format!(
            "wechat api {label} response decode failed: {}",
            truncate_log(&text, 300)
        )
    })
}

fn ensure_business_success(label: &str, value: &Value) -> Result<()> {
    let Some(code) = response_business_code(value) else {
        return Ok(());
    };
    if code == 0 {
        return Ok(());
    }
    let errmsg = value
        .get("errmsg")
        .or_else(|| value.get("errMsg"))
        .or_else(|| value.get("message"))
        .and_then(|item| item.as_str())
        .unwrap_or_default();
    let body = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    Err(anyhow!(
        "wechat api {label} business error code={} ret={} errmsg={} body={}",
        classify_business_code(code),
        code,
        errmsg,
        truncate_log(&body, 300)
    ))
}

fn response_business_code(value: &Value) -> Option<i64> {
    value
        .get("ret")
        .or_else(|| value.get("errcode"))
        .or_else(|| value.get("errCode"))
        .or_else(|| value.get("code"))
        .and_then(|item| item.as_i64())
}

fn classify_business_code(code: i64) -> &'static str {
    match code {
        -2 => "ret_minus_2",
        -14 => "session_timeout",
        _ => "protocol_error",
    }
}

fn response_preview_suffix(label: &str, text: &str) -> String {
    if !matches!(
        label,
        "wechat_send_message" | "wechat_send_image" | "wechat_notify_start"
    ) {
        return String::new();
    }
    format!(
        " body_preview={}",
        truncate_log(&text.replace('\n', "\\n"), 300)
    )
}

fn build_headers(token: Option<&str>) -> Result<HeaderMap> {
    let mut headers = build_common_headers()?;
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("authorizationtype"),
        HeaderValue::from_static("ilink_bot_token"),
    );
    headers.insert(
        HeaderName::from_static("x-wechat-uin"),
        HeaderValue::from_str(&random_wechat_uin())?,
    );
    if let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) {
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
    }
    Ok(headers)
}

fn build_common_headers() -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("ilink-app-id"),
        HeaderValue::from_static(ILINK_APP_ID),
    );
    headers.insert(
        HeaderName::from_static("ilink-app-clientversion"),
        HeaderValue::from_str(&client_version().to_string())?,
    );
    Ok(headers)
}

fn base_info() -> Value {
    json!({
        "channel_version": env!("CARGO_PKG_VERSION"),
        "bot_agent": format!("Codex Remote Gateway/{}", env!("CARGO_PKG_VERSION")),
    })
}

fn endpoint_url(base_url: &str, endpoint: &str) -> Result<Url> {
    let base = if base_url.trim().is_empty() {
        DEFAULT_WECHAT_API_BASE
    } else {
        base_url.trim()
    };
    let base = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    Url::parse(&base)
        .with_context(|| format!("invalid wechat base url `{base}`"))?
        .join(endpoint)
        .with_context(|| format!("invalid wechat endpoint `{endpoint}`"))
}

fn client_version() -> u32 {
    let parts = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect::<Vec<_>>();
    let major = parts.first().copied().unwrap_or(0) & 0xff;
    let minor = parts.get(1).copied().unwrap_or(0) & 0xff;
    let patch = parts.get(2).copied().unwrap_or(0) & 0xff;
    (major << 16) | (minor << 8) | patch
}

fn random_wechat_uin() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0);
    let seq = WECHAT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let value = ((now ^ seq.rotate_left(13)) & 0xffff_ffff).to_string();
    general_purpose::STANDARD.encode(value.as_bytes())
}

fn next_client_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let seq = WECHAT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now}-{seq}")
}

fn is_timeout_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<reqwest::Error>()
        .is_some_and(|err| err.is_timeout())
        || err.to_string().contains("operation timed out")
}

fn url_escape(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn redact_url(value: &str) -> String {
    value
        .replace("qrcode=", "qrcode=***")
        .replace("verify_code=", "verify_code=***")
}

fn truncate_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    output.push_str("...");
    output
}

pub(crate) fn default_long_poll_timeout_ms() -> u64 {
    LONG_POLL_TIMEOUT_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_success_accepts_empty_or_zero_code() {
        ensure_business_success("wechat_send_message", &json!({})).unwrap();
        ensure_business_success("wechat_send_message", &json!({ "ret": 0 })).unwrap();
        ensure_business_success("wechat_send_message", &json!({ "errcode": 0 })).unwrap();
    }

    #[test]
    fn business_error_classifies_expired_context_token() {
        let err = ensure_business_success(
            "wechat_send_message",
            &json!({ "ret": -2, "errmsg": "context token expired" }),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("ret_minus_2"));
        assert!(err.contains("ret=-2"));
    }
}
