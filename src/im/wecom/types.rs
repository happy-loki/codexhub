use crate::config::WecomConfig;

#[derive(Debug, Clone)]
pub struct WecomSettings {
    pub bot_id: String,
    pub secret: String,
    pub websocket_url: String,
    pub allowed_user_ids: Vec<String>,
    pub allowed_chat_ids: Vec<String>,
}

impl WecomSettings {
    pub fn from_app_config(config: &WecomConfig) -> Self {
        Self {
            bot_id: config.bot_id.trim().to_string(),
            secret: config.secret.trim().to_string(),
            websocket_url: config.websocket_url.trim().to_string(),
            allowed_user_ids: config.allowed_user_ids.clone(),
            allowed_chat_ids: config.allowed_chat_ids.clone(),
        }
    }
}
