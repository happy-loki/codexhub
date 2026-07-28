use anyhow::{Context, Result};
use serde::Deserialize;

const QR_BASE: &str = "https://work.weixin.qq.com/ai/qc";

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct GenerateData {
    scode: String,
    auth_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueryData {
    status: String,
    bot_info: Option<BotInfo>,
}

#[derive(Debug, Deserialize)]
struct BotInfo {
    botid: String,
    secret: String,
}

#[derive(Debug, Clone)]
pub struct QrSession {
    pub scode: String,
    pub auth_url: String,
}

#[derive(Debug)]
pub enum QrPoll {
    Pending(String),
    Success { bot_id: String, secret: String },
}

pub async fn start(client: &reqwest::Client) -> Result<QrSession> {
    let platform = if cfg!(target_os = "windows") {
        2
    } else if cfg!(target_os = "macos") {
        1
    } else if cfg!(target_os = "linux") {
        3
    } else {
        0
    };
    let response = client
        .get(format!("{QR_BASE}/generate"))
        .query(&[("source", "wecom-cli"), ("plat", &platform.to_string())])
        .send()
        .await?
        .error_for_status()?
        .json::<Envelope<GenerateData>>()
        .await
        .context("invalid WeCom QR response")?;
    let auth_url = response.data.auth_url.unwrap_or_else(|| {
        format!(
            "{QR_BASE}/gen?source=wecom-cli&scode={}",
            response.data.scode
        )
    });
    Ok(QrSession {
        scode: response.data.scode,
        auth_url,
    })
}

pub async fn poll(client: &reqwest::Client, scode: &str) -> Result<QrPoll> {
    let response = client
        .get(format!("{QR_BASE}/query_result"))
        .query(&[("scode", scode)])
        .send()
        .await?
        .error_for_status()?
        .json::<Envelope<QueryData>>()
        .await
        .context("invalid WeCom QR poll response")?;
    if response.data.status == "success" {
        let info = response
            .data
            .bot_info
            .context("WeCom QR succeeded without bot credentials")?;
        return Ok(QrPoll::Success {
            bot_id: info.botid,
            secret: info.secret,
        });
    }
    Ok(QrPoll::Pending(response.data.status))
}
