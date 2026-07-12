use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::config::{ProviderConfig, ProviderType, provider_route_id};

const MARKER_PREFIX: &str = "codexhub:enc:v1:";
const FOOTPRINT_BYTES: usize = 6;
const ANTHROPIC_THINKING_KIND: &str = "thinking";
const ANTHROPIC_REDACTED_THINKING_KIND: &str = "redacted_thinking";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnthropicEncryptedContentKind {
    Thinking,
    RedactedThinking,
}

impl AnthropicEncryptedContentKind {
    fn marker(self) -> &'static str {
        match self {
            Self::Thinking => ANTHROPIC_THINKING_KIND,
            Self::RedactedThinking => ANTHROPIC_REDACTED_THINKING_KIND,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EncryptedContentScope {
    protocol: &'static str,
    footprint: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EncryptedContentStats {
    pub(crate) decoded: usize,
    pub(crate) filtered: usize,
    pub(crate) legacy_preserved: usize,
    pub(crate) dropped_items: usize,
}

enum ScopedContent<'a> {
    Unmarked,
    Matching(&'a str),
    Foreign,
}

impl EncryptedContentScope {
    pub(crate) fn for_provider(provider: &ProviderConfig) -> Self {
        let protocol = match provider.provider_type {
            ProviderType::OpenAiResponses => "openai",
            ProviderType::GrokResponses => "grok",
            ProviderType::ChatCompletions => "chat_completions",
            ProviderType::AnthropicMessages => "anthropic",
        };
        let digest = Sha256::digest(provider_route_id(provider).as_bytes());
        let footprint = hex::encode(&digest[..FOOTPRINT_BYTES]);
        Self {
            protocol,
            footprint,
        }
    }

    pub(crate) fn encode(&self, content: &str) -> String {
        if content.starts_with(MARKER_PREFIX) {
            return content.to_string();
        }
        format!(
            "{MARKER_PREFIX}{}:{}:{content}",
            self.protocol, self.footprint
        )
    }

    pub(crate) fn decode_matching<'a>(&self, content: &'a str) -> Option<&'a str> {
        match self.decode(content) {
            ScopedContent::Matching(content) => Some(content),
            ScopedContent::Unmarked | ScopedContent::Foreign => None,
        }
    }

    pub(crate) fn encode_anthropic(
        &self,
        kind: AnthropicEncryptedContentKind,
        content: &str,
    ) -> String {
        if content.starts_with(MARKER_PREFIX) {
            return content.to_string();
        }
        self.encode(&format!("{}:{content}", kind.marker()))
    }

    pub(crate) fn decode_anthropic<'a>(
        &self,
        content: &'a str,
    ) -> Option<(AnthropicEncryptedContentKind, &'a str)> {
        if self.protocol != "anthropic" {
            return None;
        }
        let payload = self.decode_matching(content)?;
        if let Some(content) = payload
            .strip_prefix(ANTHROPIC_THINKING_KIND)
            .and_then(|payload| payload.strip_prefix(':'))
        {
            return Some((AnthropicEncryptedContentKind::Thinking, content));
        }
        payload
            .strip_prefix(ANTHROPIC_REDACTED_THINKING_KIND)
            .and_then(|payload| payload.strip_prefix(':'))
            .map(|content| (AnthropicEncryptedContentKind::RedactedThinking, content))
    }

    fn decode<'a>(&self, content: &'a str) -> ScopedContent<'a> {
        let Some(encoded) = content.strip_prefix(MARKER_PREFIX) else {
            return ScopedContent::Unmarked;
        };
        let mut parts = encoded.splitn(3, ':');
        let Some(protocol) = parts.next() else {
            return ScopedContent::Foreign;
        };
        let Some(footprint) = parts.next() else {
            return ScopedContent::Foreign;
        };
        let Some(content) = parts.next() else {
            return ScopedContent::Foreign;
        };
        if protocol == self.protocol && footprint == self.footprint {
            ScopedContent::Matching(content)
        } else {
            ScopedContent::Foreign
        }
    }
}

pub(crate) fn encode_response_object(
    object: &mut Map<String, Value>,
    scope: &EncryptedContentScope,
) -> bool {
    if !is_provider_private_item(object) {
        return false;
    }
    let anthropic_kind = (scope.protocol == "anthropic").then(|| {
        if object
            .get("summary")
            .is_some_and(|summary| !summary.is_null())
        {
            AnthropicEncryptedContentKind::Thinking
        } else {
            AnthropicEncryptedContentKind::RedactedThinking
        }
    });
    let Some(Value::String(content)) = object.get_mut("encrypted_content") else {
        return false;
    };
    if content.is_empty() || content.starts_with(MARKER_PREFIX) {
        return false;
    }
    *content = match anthropic_kind {
        Some(kind) => scope.encode_anthropic(kind, content),
        None => scope.encode(content),
    };
    true
}

pub(crate) fn prepare_responses_request(
    value: &mut Value,
    scope: &EncryptedContentScope,
) -> EncryptedContentStats {
    let has_marker = value
        .get("input")
        .is_some_and(input_contains_codexhub_marker);
    prepare_input(value, scope, has_marker, false)
}

pub(crate) fn remove_all_responses_encrypted_content(value: &mut Value) -> EncryptedContentStats {
    let placeholder = EncryptedContentScope {
        protocol: "",
        footprint: String::new(),
    };
    prepare_input(value, &placeholder, true, true)
}

fn prepare_input(
    value: &mut Value,
    scope: &EncryptedContentScope,
    drop_unmarked: bool,
    drop_all: bool,
) -> EncryptedContentStats {
    let mut stats = EncryptedContentStats::default();
    let mut remove_input = false;
    let Some(input) = value.get_mut("input") else {
        return stats;
    };

    match input {
        Value::Array(items) => {
            items.retain_mut(|item| {
                let keep = prepare_input_item(item, scope, drop_unmarked, drop_all, &mut stats);
                if !keep {
                    stats.dropped_items += 1;
                }
                keep
            });
            remove_input = items.is_empty();
        }
        Value::Object(_) => {
            if !prepare_input_item(input, scope, drop_unmarked, drop_all, &mut stats) {
                stats.dropped_items += 1;
                remove_input = true;
            }
        }
        _ => {}
    }

    if remove_input {
        value.as_object_mut().map(|object| object.remove("input"));
    }
    stats
}

fn prepare_input_item(
    item: &mut Value,
    scope: &EncryptedContentScope,
    drop_unmarked: bool,
    drop_all: bool,
    stats: &mut EncryptedContentStats,
) -> bool {
    let Some(object) = item.as_object_mut() else {
        return true;
    };
    if !is_provider_private_item(object) {
        return true;
    }
    let Some(content) = object
        .get("encrypted_content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return true;
    };

    if drop_all {
        stats.filtered += 1;
        return remove_private_transport(object);
    }

    match scope.decode(&content) {
        ScopedContent::Matching(decoded) => {
            object.insert(
                "encrypted_content".to_string(),
                Value::String(decoded.to_string()),
            );
            stats.decoded += 1;
            true
        }
        ScopedContent::Foreign => {
            stats.filtered += 1;
            remove_private_transport(object)
        }
        ScopedContent::Unmarked if drop_unmarked => {
            stats.filtered += 1;
            remove_private_transport(object)
        }
        ScopedContent::Unmarked => {
            stats.legacy_preserved += 1;
            true
        }
    }
}

fn remove_private_transport(object: &mut Map<String, Value>) -> bool {
    object.remove("encrypted_content");
    object.remove("id");
    object.remove("status");
    if object.get("content").is_some_and(Value::is_null) {
        object.remove("content");
    }
    has_replayable_content(object)
}

fn has_replayable_content(object: &Map<String, Value>) -> bool {
    object.iter().any(|(key, value)| match key.as_str() {
        "type" | "id" | "status" | "internal_chat_message_metadata_passthrough" => false,
        "summary" | "content" => {
            !value.is_null()
                && !value.as_array().is_some_and(Vec::is_empty)
                && !value.as_str().is_some_and(str::is_empty)
        }
        _ => true,
    })
}

fn input_contains_codexhub_marker(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(input_contains_codexhub_marker),
        Value::Object(object) => object
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.starts_with(MARKER_PREFIX)),
        _ => false,
    }
}

fn is_provider_private_item(object: &Map<String, Value>) -> bool {
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|item_type| {
            matches!(
                item_type,
                "reasoning" | "compaction" | "compaction_summary" | "context_compaction"
            )
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn provider(name: &str, provider_type: ProviderType, base_url: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            provider_type,
            base_url: base_url.to_string(),
            ..ProviderConfig::default()
        }
    }

    #[test]
    fn matching_scope_round_trips_raw_encrypted_content() {
        let scope = EncryptedContentScope::for_provider(&provider(
            "grok",
            ProviderType::GrokResponses,
            "https://api.x.ai/v1",
        ));
        let encoded = scope.encode("opaque-grok-content");
        let mut request = json!({
            "input": [{
                "type": "reasoning",
                "id": "rs_1",
                "summary": [{"type": "summary_text", "text": "thinking"}],
                "encrypted_content": encoded
            }]
        });

        let stats = prepare_responses_request(&mut request, &scope);

        assert_eq!(stats.decoded, 1);
        assert_eq!(stats.filtered, 0);
        assert_eq!(
            request["input"][0]["encrypted_content"],
            "opaque-grok-content"
        );
        assert_eq!(request["input"][0]["id"], "rs_1");
    }

    #[test]
    fn decode_matching_rejects_unmarked_and_foreign_content() {
        let anthropic_scope = EncryptedContentScope::for_provider(&provider(
            "anthropic",
            ProviderType::AnthropicMessages,
            "https://api.anthropic.com/v1",
        ));
        let grok_scope = EncryptedContentScope::for_provider(&provider(
            "grok",
            ProviderType::GrokResponses,
            "https://api.x.ai/v1",
        ));

        let matching = anthropic_scope.encode("sig_123");
        let foreign = grok_scope.encode("opaque-grok-content");

        assert_eq!(anthropic_scope.decode_matching(&matching), Some("sig_123"));
        assert_eq!(anthropic_scope.decode_matching(&foreign), None);
        assert_eq!(anthropic_scope.decode_matching("legacy-unmarked"), None);
    }

    #[test]
    fn anthropic_typed_content_round_trips_kind_and_raw_content() {
        let scope = EncryptedContentScope::for_provider(&provider(
            "anthropic",
            ProviderType::AnthropicMessages,
            "https://api.anthropic.com/v1",
        ));

        let thinking =
            scope.encode_anthropic(AnthropicEncryptedContentKind::Thinking, "sig:with:colons");
        let redacted = scope.encode_anthropic(
            AnthropicEncryptedContentKind::RedactedThinking,
            "opaque-redacted",
        );

        assert_eq!(
            scope.decode_anthropic(&thinking),
            Some((AnthropicEncryptedContentKind::Thinking, "sig:with:colons"))
        );
        assert_eq!(
            scope.decode_anthropic(&redacted),
            Some((
                AnthropicEncryptedContentKind::RedactedThinking,
                "opaque-redacted"
            ))
        );
    }

    #[test]
    fn anthropic_response_object_uses_summary_presence_as_wire_kind() {
        let scope = EncryptedContentScope::for_provider(&provider(
            "anthropic",
            ProviderType::AnthropicMessages,
            "https://api.anthropic.com/v1",
        ));
        let mut thinking = json!({
            "type": "reasoning",
            "summary": [],
            "encrypted_content": "sig_123"
        });
        let mut redacted = json!({
            "type": "reasoning",
            "encrypted_content": "encrypted_456"
        });

        assert!(encode_response_object(
            thinking.as_object_mut().unwrap(),
            &scope
        ));
        assert!(encode_response_object(
            redacted.as_object_mut().unwrap(),
            &scope
        ));
        assert_eq!(
            scope.decode_anthropic(thinking["encrypted_content"].as_str().unwrap()),
            Some((AnthropicEncryptedContentKind::Thinking, "sig_123"))
        );
        assert_eq!(
            scope.decode_anthropic(redacted["encrypted_content"].as_str().unwrap()),
            Some((
                AnthropicEncryptedContentKind::RedactedThinking,
                "encrypted_456"
            ))
        );
    }

    #[test]
    fn foreign_scope_drops_private_blob_and_provider_item_id() {
        let grok_scope = EncryptedContentScope::for_provider(&provider(
            "grok",
            ProviderType::GrokResponses,
            "https://api.x.ai/v1",
        ));
        let openai_scope = EncryptedContentScope::for_provider(&provider(
            "openai",
            ProviderType::OpenAiResponses,
            "https://api.openai.com/v1",
        ));
        let mut request = json!({
            "input": [{
                "type": "reasoning",
                "id": "rs_grok",
                "status": "completed",
                "summary": [{"type": "summary_text", "text": "keep summary"}],
                "encrypted_content": grok_scope.encode("opaque-grok-content")
            }]
        });

        let stats = prepare_responses_request(&mut request, &openai_scope);

        assert_eq!(stats.filtered, 1);
        assert!(request["input"][0].get("encrypted_content").is_none());
        assert!(request["input"][0].get("id").is_none());
        assert!(request["input"][0].get("status").is_none());
        assert_eq!(request["input"][0]["summary"][0]["text"], "keep summary");
    }

    #[test]
    fn foreign_empty_reasoning_item_is_removed() {
        let grok_scope = EncryptedContentScope::for_provider(&provider(
            "grok",
            ProviderType::GrokResponses,
            "https://api.x.ai/v1",
        ));
        let openai_scope = EncryptedContentScope::for_provider(&provider(
            "openai",
            ProviderType::OpenAiResponses,
            "https://api.openai.com/v1",
        ));
        let mut request = json!({
            "input": [
                {
                    "type": "reasoning",
                    "encrypted_content": grok_scope.encode("opaque-grok-content")
                },
                {"type": "message", "role": "user", "content": []}
            ]
        });

        let stats = prepare_responses_request(&mut request, &openai_scope);

        assert_eq!(stats.filtered, 1);
        assert_eq!(stats.dropped_items, 1);
        assert_eq!(request["input"].as_array().unwrap().len(), 1);
        assert_eq!(request["input"][0]["type"], "message");
    }

    #[test]
    fn marked_history_filters_unmarked_legacy_private_content() {
        let scope = EncryptedContentScope::for_provider(&provider(
            "openai",
            ProviderType::OpenAiResponses,
            "https://api.openai.com/v1",
        ));
        let mut request = json!({
            "input": [
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "legacy"}],
                    "encrypted_content": "legacy-unmarked"
                },
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "current"}],
                    "encrypted_content": scope.encode("current-marked")
                }
            ]
        });

        let stats = prepare_responses_request(&mut request, &scope);

        assert_eq!(stats.filtered, 1);
        assert_eq!(stats.decoded, 1);
        assert!(request["input"][0].get("encrypted_content").is_none());
        assert_eq!(request["input"][1]["encrypted_content"], "current-marked");
    }

    #[test]
    fn entirely_unmarked_legacy_history_is_preserved_for_first_migration_turn() {
        let scope = EncryptedContentScope::for_provider(&provider(
            "openai",
            ProviderType::OpenAiResponses,
            "https://api.openai.com/v1",
        ));
        let mut request = json!({
            "input": [{
                "type": "reasoning",
                "encrypted_content": "legacy-unmarked"
            }]
        });

        let stats = prepare_responses_request(&mut request, &scope);

        assert_eq!(stats.legacy_preserved, 1);
        assert_eq!(request["input"][0]["encrypted_content"], "legacy-unmarked");
    }

    #[test]
    fn response_items_are_marked_idempotently() {
        let scope = EncryptedContentScope::for_provider(&provider(
            "grok",
            ProviderType::GrokResponses,
            "https://api.x.ai/v1",
        ));
        let mut item = json!({
            "type": "reasoning",
            "encrypted_content": "opaque-grok-content"
        });

        assert!(encode_response_object(
            item.as_object_mut().unwrap(),
            &scope
        ));
        let encoded = item["encrypted_content"].as_str().unwrap().to_string();
        assert!(encoded.starts_with("codexhub:enc:v1:grok:"));
        assert!(!encode_response_object(
            item.as_object_mut().unwrap(),
            &scope
        ));
        assert_eq!(item["encrypted_content"], encoded);
    }
}
