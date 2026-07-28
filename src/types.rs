use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatType {
    Direct,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImPlatformKind {
    Feishu,
    Telegram,
    Wechat,
    Wecom,
}

impl Default for ImPlatformKind {
    fn default() -> Self {
        Self::Feishu
    }
}

impl ImPlatformKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Feishu => "feishu",
            Self::Telegram => "telegram",
            Self::Wechat => "wechat",
            Self::Wecom => "wecom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundAttachment {
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub text_hint: Option<String>,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadRouteDirection {
    Prev,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboundCallbackKind {
    Message,
    Welcome,
    CardEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InboundAction {
    ThreadRouteOpen,
    ApprovalDecision {
        request_fingerprint: String,
        option_index: usize,
    },
    ThreadRouteChoice {
        request_id: String,
        action: String,
    },
    ThreadRouteCreateSubmit {
        request_id: String,
        cwd_choice: Option<String>,
        cwd_custom: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        permission: Option<String>,
    },
    ThreadRouteCreateDefault {
        request_id: String,
    },
    ThreadRouteCreateConfigured {
        request_id: String,
    },
    ThreadRouteCreateEdit {
        request_id: String,
        field: String,
    },
    ThreadRouteCreateSetIndex {
        request_id: String,
        field: String,
        page: usize,
        index: usize,
    },
    ThreadRouteCreateSetValue {
        request_id: String,
        field: String,
        value: String,
    },
    ThreadRouteCreateOptionsPage {
        request_id: String,
        field: String,
        direction: ThreadRouteDirection,
    },
    ThreadRouteResumeSelected {
        request_id: String,
        thread_id: String,
    },
    ThreadRouteResumeIndex {
        request_id: String,
        page: usize,
        index: usize,
    },
    ThreadRouteListPage {
        request_id: String,
        direction: ThreadRouteDirection,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundMessage {
    #[serde(default)]
    pub platform: ImPlatformKind,
    pub account_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub chat_type: ChatType,
    pub message_id: String,
    #[serde(default)]
    pub received_at_ms: u128,
    pub text: String,
    pub mentioned: bool,
    pub approval_request_key: Option<String>,
    #[serde(default)]
    pub action: Option<InboundAction>,
    #[serde(default)]
    pub card_message_id: Option<String>,
    #[serde(default)]
    pub callback_req_id: Option<String>,
    #[serde(default)]
    pub callback_kind: Option<InboundCallbackKind>,
    #[serde(default)]
    pub attachments: Vec<InboundAttachment>,
}

impl InboundMessage {
    pub fn conversation_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.platform.key(),
            self.account_id,
            self.chat_id
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub at_ms: u128,
    pub level: String,
    pub kind: String,
    pub message: String,
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis())
        .unwrap_or_default()
}
