use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── GatewayRequest ────────────────────────────────────────────

/// Responses API 请求的统一中间表示。
/// 参考 AxonHub `responses/model.go` 的 Request。
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayRequest {
    pub model: String,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default, deserialize_with = "deserialize_input")]
    pub input: Vec<ResponseItem>,
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub reasoning: Option<Reasoning>,
    #[serde(default)]
    pub text: Option<TextOptions>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub max_output_tokens: Option<i64>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,

    // cache
    #[serde(default)]
    pub prompt_cache_key: Option<String>,
    #[serde(default)]
    pub prompt_cache_retention: Option<String>,
    #[serde(default)]
    pub previous_response_id: Option<String>,
}

/// input 可以是纯字符串或 item 数组。
fn deserialize_input<'de, D>(deserializer: D) -> Result<Vec<ResponseItem>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(s) => Ok(vec![ResponseItem {
            item_type: ItemType::InputText,
            id: None,
            role: None,
            content: Some(ItemContent::Text(s)),
            text: None,
            name: None,
            namespace: None,
            call_id: None,
            arguments: None,
            input: None,
            output: None,
            status: None,
            image_url: None,
            detail: None,
            action: None,
            summary: None,
            encrypted_content: None,
        }]),
        Value::Array(_) => serde_json::from_value(value).map_err(serde::de::Error::custom),
        Value::Null => Ok(Vec::new()),
        _ => Err(serde::de::Error::custom("input must be string or array")),
    }
}

// ─── ResponseItem ──────────────────────────────────────────────

/// Responses API 的 Item，覆盖所有 type。
/// 参考 AxonHub `responses/model.go` 的 Item。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseItem {
    #[serde(rename = "type")]
    pub item_type: ItemType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ItemContent>,
    /// input_text / output_text 的顶层 text 字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Responses namespace tools use namespace + name and are flattened for Chat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// function_call / function_call_output 的 call_id。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// function_call 的参数 JSON 字符串。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// custom_tool_call 的 freeform input。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// function_call_output / custom_tool_call_output 的结果。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<FunctionCallOutput>,
    /// item 状态。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// input_image / image_generation_call 的图片 URL 或 data URL。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// input_image 的 detail。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// web_search_call / image_generation_call 的 action 元数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Value>,
    /// reasoning item 的 summary。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Vec<SummaryPart>>,
    /// reasoning item 的加密内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
}

/// Item type 枚举。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    Message,
    InputText,
    InputImage,
    FunctionCall,
    FunctionCallOutput,
    CustomToolCall,
    CustomToolCallOutput,
    WebSearchCall,
    ImageGenerationCall,
    Reasoning,
    OutputText,
    #[serde(other)]
    Unknown,
}

/// Item 的 content 可以是纯文本或 content part 数组。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ItemContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

// ─── ContentPart ───────────────────────────────────────────────

/// content part（嵌在 message.content 内）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// output_text 的 annotations。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<Value>>,
}

impl ContentPart {
    pub fn output_text(text: impl Into<String>) -> Self {
        Self {
            part_type: "output_text".into(),
            text: Some(text.into()),
            image_url: None,
            detail: None,
            annotations: Some(Vec::new()),
        }
    }

    pub fn summary_text(text: impl Into<String>) -> Self {
        Self {
            part_type: "summary_text".into(),
            text: Some(text.into()),
            image_url: None,
            detail: None,
            annotations: None,
        }
    }
}

// ─── FunctionCallOutput ────────────────────────────────────────

/// Responses API tool output is either a plain string or structured content items.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FunctionCallOutput {
    Text(String),
    ContentItems(Vec<FunctionCallOutputContentItem>),
}

impl FunctionCallOutput {
    pub fn to_chat_tool_content(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::ContentItems(items) => items
                .iter()
                .filter_map(|item| {
                    let item_type = item.item_type.as_str();
                    if matches!(item_type, "input_text" | "output_text" | "text") {
                        item.text.as_deref().filter(|text| !text.trim().is_empty())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl From<String> for FunctionCallOutput {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for FunctionCallOutput {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallOutputContentItem {
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

// ─── SummaryPart ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: String,
}

// ─── Reasoning / TextOptions ───────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    /// json_schema 时的 schema 定义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

// ─── ResponseObject（完整 response 对象）────────────────────────

/// Responses API 的完整 response 对象（非流式 / response.completed 时使用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseObject {
    pub id: String,
    #[serde(rename = "object")]
    pub object_type: String,
    pub model: String,
    pub created_at: i64,
    pub status: String,
    #[serde(default)]
    pub output: Vec<ResponseItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

impl ResponseObject {
    /// 创建初始的 in_progress response 对象。
    pub fn new_in_progress(id: String, model: String) -> Self {
        Self {
            id,
            object_type: "response".into(),
            model,
            created_at: chrono_timestamp(),
            status: "in_progress".into(),
            output: Vec::new(),
            usage: None,
            error: None,
        }
    }
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ─── Usage ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens_details: Option<InputTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<OutputTokensDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputTokensDetails {
    #[serde(default)]
    pub cached_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: i64,
}

// ─── 生成 gateway response id ──────────────────────────────────

pub fn generate_response_id() -> String {
    format!("gwresp_{}", uuid::Uuid::new_v4().as_simple())
}

pub fn generate_item_id() -> String {
    format!("gwitem_{}", uuid::Uuid::new_v4().as_simple())
}
