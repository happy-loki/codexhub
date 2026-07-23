use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};

use super::types::WecomSettings;

pub(crate) struct WecomSendCommand {
    pub cmd: String,
    pub req_id: Option<String>,
    pub body: Value,
    pub result: oneshot::Sender<Result<Value>>,
}

#[derive(Clone)]
pub struct WecomApi {
    pub(crate) settings: WecomSettings,
    sender: Arc<Mutex<Option<mpsc::UnboundedSender<WecomSendCommand>>>>,
}

impl WecomApi {
    pub fn new(settings: WecomSettings) -> Self {
        Self {
            settings,
            sender: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) async fn install_sender(
        &self,
        sender: Option<mpsc::UnboundedSender<WecomSendCommand>>,
    ) {
        *self.sender.lock().await = sender;
    }

    async fn request(&self, cmd: &str, req_id: Option<&str>, body: Value) -> Result<Value> {
        crate::chain_log::write_line(format!(
            "[wecom_api] event=request cmd={} req_id={} msgtype={} card_type={} card_action_type={} button_count={} action_menu_count={}",
            cmd,
            req_id.unwrap_or(""),
            body.get("msgtype").and_then(Value::as_str).unwrap_or(""),
            body.pointer("/template_card/card_type")
                .and_then(Value::as_str)
                .unwrap_or(""),
            body.pointer("/template_card/card_action/type")
                .map(Value::to_string)
                .unwrap_or_default(),
            body.pointer("/template_card/button_list")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default(),
            body.pointer("/template_card/action_menu/action_list")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default()
        ));
        let sender = self
            .sender
            .lock()
            .await
            .clone()
            .context("WeCom WebSocket is not connected")?;
        let (result_tx, result_rx) = oneshot::channel();
        sender
            .send(WecomSendCommand {
                cmd: cmd.to_string(),
                req_id: req_id.map(str::to_string),
                body,
                result: result_tx,
            })
            .map_err(|_| anyhow::anyhow!("WeCom WebSocket sender is closed"))?;
        tokio::time::timeout(Duration::from_secs(30), result_rx)
            .await
            .context("WeCom acknowledgement timed out")?
            .context("WeCom acknowledgement channel closed")?
    }

    pub async fn send_markdown(&self, chat_id: &str, content: &str) -> Result<String> {
        let response = self
            .request(
                "aibot_send_msg",
                None,
                json!({
                    "chatid": chat_id,
                    "msgtype": "markdown",
                    "markdown": { "content": content }
                }),
            )
            .await?;
        Ok(response_req_id(&response))
    }

    pub async fn send_template_card(&self, chat_id: &str, card: Value) -> Result<String> {
        let response = self
            .request(
                "aibot_send_msg",
                None,
                json!({
                    "chatid": chat_id,
                    "msgtype": "template_card",
                    "template_card": card
                }),
            )
            .await?;
        Ok(response_req_id(&response))
    }

    pub async fn reply_template_card(&self, callback_req_id: &str, card: Value) -> Result<String> {
        let response = self
            .request(
                "aibot_respond_msg",
                Some(callback_req_id),
                json!({
                    "msgtype": "template_card",
                    "template_card": card
                }),
            )
            .await?;
        Ok(response_req_id(&response))
    }

    pub async fn reply_welcome_template_card(
        &self,
        callback_req_id: &str,
        card: Value,
    ) -> Result<String> {
        let response = self
            .request(
                "aibot_respond_welcome_msg",
                Some(callback_req_id),
                json!({
                    "msgtype": "template_card",
                    "template_card": card
                }),
            )
            .await?;
        Ok(response_req_id(&response))
    }

    pub async fn reply_stream(
        &self,
        callback_req_id: &str,
        stream_id: &str,
        content: &str,
        finish: bool,
    ) -> Result<String> {
        let response = self
            .request(
                "aibot_respond_msg",
                Some(callback_req_id),
                json!({
                    "msgtype": "stream",
                    "stream": {
                        "id": stream_id,
                        "finish": finish,
                        "content": content
                    }
                }),
            )
            .await?;
        Ok(response_req_id(&response))
    }

    pub async fn update_template_card(
        &self,
        callback_req_id: &str,
        card: Value,
        user_id: Option<&str>,
    ) -> Result<String> {
        let mut body = json!({
            "response_type": "update_template_card",
            "template_card": card
        });
        if let Some(user_id) = user_id.filter(|value| !value.trim().is_empty()) {
            body["userids"] = json!([user_id]);
        }
        let response = self
            .request("aibot_respond_update_msg", Some(callback_req_id), body)
            .await?;
        Ok(response_req_id(&response))
    }

    pub async fn upload_and_send_media(
        &self,
        chat_id: &str,
        media_type: &str,
        path: &Path,
    ) -> Result<String> {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("failed to read WeCom media {}", path.display()))?;
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("attachment.bin");
        let media_id = self.upload_media(media_type, filename, &bytes).await?;
        let response = self
            .request(
                "aibot_send_msg",
                None,
                json!({
                    "chatid": chat_id,
                    "msgtype": media_type,
                    media_type: { "media_id": media_id }
                }),
            )
            .await?;
        Ok(response_req_id(&response))
    }

    async fn upload_media(&self, media_type: &str, filename: &str, bytes: &[u8]) -> Result<String> {
        const CHUNK_SIZE: usize = 512 * 1024;
        const MAX_CHUNKS: usize = 100;
        let total_chunks = bytes.len().div_ceil(CHUNK_SIZE).max(1);
        anyhow::ensure!(
            total_chunks <= MAX_CHUNKS,
            "WeCom media exceeds the 50 MB upload limit"
        );
        let digest = format!("{:x}", md5::compute(bytes));
        let init = self
            .request(
                "aibot_upload_media_init",
                None,
                json!({
                    "type": media_type,
                    "filename": filename,
                    "total_size": bytes.len(),
                    "total_chunks": total_chunks,
                    "md5": digest
                }),
            )
            .await?;
        let upload_id = init
            .pointer("/body/upload_id")
            .and_then(Value::as_str)
            .context("WeCom upload init returned no upload_id")?;
        for (chunk_index, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
            use base64::Engine as _;
            self.request(
                "aibot_upload_media_chunk",
                None,
                json!({
                    "upload_id": upload_id,
                    "chunk_index": chunk_index,
                    "base64_data": base64::engine::general_purpose::STANDARD.encode(chunk)
                }),
            )
            .await?;
        }
        let finish = self
            .request(
                "aibot_upload_media_finish",
                None,
                json!({ "upload_id": upload_id }),
            )
            .await?;
        finish
            .pointer("/body/media_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("WeCom upload finish returned no media_id")
    }
}

fn response_req_id(response: &Value) -> String {
    response
        .pointer("/headers/req_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api() -> WecomApi {
        WecomApi::new(WecomSettings {
            bot_id: "bot".into(),
            secret: "secret".into(),
            websocket_url: String::new(),
            allowed_user_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
        })
    }

    async fn capture_command<F, Fut>(invoke: F) -> (String, Option<String>, Value)
    where
        F: FnOnce(WecomApi) -> Fut,
        Fut: std::future::Future<Output = Result<String>> + Send + 'static,
    {
        let api = api();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        api.install_sender(Some(sender)).await;
        let task = tokio::spawn(invoke(api));
        let command = receiver.recv().await.expect("WeCom command");
        let WecomSendCommand {
            cmd,
            req_id,
            body,
            result,
        } = command;
        result
            .send(Ok(json!({ "headers": { "req_id": "response-1" } })))
            .expect("command result receiver");
        assert_eq!(task.await.expect("reply task").unwrap(), "response-1");
        (cmd, req_id, body)
    }

    #[tokio::test]
    async fn replies_template_card_through_message_callback() {
        let command = capture_command(|api| async move {
            api.reply_template_card("message-callback", json!({ "card_type": "text_notice" }))
                .await
        })
        .await;
        assert_eq!(command.0, "aibot_respond_msg");
        assert_eq!(command.1.as_deref(), Some("message-callback"));
        assert_eq!(command.2["msgtype"], "template_card");
    }

    #[tokio::test]
    async fn replies_template_card_through_welcome_callback() {
        let command = capture_command(|api| async move {
            api.reply_welcome_template_card(
                "welcome-callback",
                json!({ "card_type": "text_notice" }),
            )
            .await
        })
        .await;
        assert_eq!(command.0, "aibot_respond_welcome_msg");
        assert_eq!(command.1.as_deref(), Some("welcome-callback"));
        assert_eq!(command.2["msgtype"], "template_card");
    }
}
