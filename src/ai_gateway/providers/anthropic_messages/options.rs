use crate::ai_gateway::config::{ProviderConfig, provider_endpoint_url};
use crate::ai_gateway::error::GatewayError;

use super::types::ANTHROPIC_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicProviderProfile {
    Anthropic,
    GlmAnthropic,
}

impl AnthropicProviderProfile {
    fn from_compatibility(value: Option<&str>) -> Result<Self, GatewayError> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::Anthropic);
        };
        match value {
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "glm_anthropic" | "zhipu_anthropic" => Ok(Self::GlmAnthropic),
            other => Err(GatewayError::bad_request(format!(
                "unsupported Anthropic Messages compatibility profile '{}'",
                other
            ))),
        }
    }

    pub(super) fn is_web_search_server_tool(self, name: &str) -> bool {
        name == "web_search" || matches!(self, Self::GlmAnthropic) && name == "web_search_prime"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicAuthStyle {
    XApiKey,
    Bearer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicHeaderStyle {
    Plain,
    ClaudeCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicVersionHeader {
    Required(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicEndpointStyle {
    V1Messages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicToolChoiceSupport {
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicThinkingSupport {
    AnthropicNative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AnthropicCapabilities {
    pub text: bool,
    pub images: bool,
    pub tools: bool,
    pub parallel_tool_use: bool,
    pub tool_choice: AnthropicToolChoiceSupport,
    pub thinking: AnthropicThinkingSupport,
    pub prompt_cache_control: bool,
    pub system_cache_control: bool,
    pub web_search_tool: bool,
    pub computer_use_tool: bool,
    pub max_tool_result_blocks: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicUsageShape {
    Anthropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicStreamShape {
    Anthropic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AnthropicQuirks {
    pub require_user_message_first: bool,
    pub merge_consecutive_same_role_messages: bool,
    pub allow_empty_text_blocks: bool,
    pub require_tool_result_after_tool_use: bool,
    pub tool_name_regex: Option<&'static str>,
    pub usage_shape: AnthropicUsageShape,
    pub stream_event_shape: AnthropicStreamShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AnthropicProviderOptions {
    pub profile: AnthropicProviderProfile,
    pub auth: AnthropicAuthStyle,
    pub headers: AnthropicHeaderStyle,
    pub version_header: AnthropicVersionHeader,
    pub endpoint: AnthropicEndpointStyle,
    pub capabilities: AnthropicCapabilities,
    pub quirks: AnthropicQuirks,
}

impl AnthropicProviderOptions {
    pub(super) fn from_provider(provider: &ProviderConfig) -> Result<Self, GatewayError> {
        let profile =
            AnthropicProviderProfile::from_compatibility(provider.compatibility.as_deref())?;
        Ok(match profile {
            AnthropicProviderProfile::Anthropic => Self::anthropic(),
            AnthropicProviderProfile::GlmAnthropic => Self::glm_anthropic(),
        })
    }

    pub(super) fn messages_url(&self, provider: &ProviderConfig) -> String {
        match self.endpoint {
            AnthropicEndpointStyle::V1Messages => {
                provider_endpoint_url(&provider.base_url, "messages")
            }
        }
    }

    pub(super) fn anthropic() -> Self {
        let mut options = Self::base(AnthropicProviderProfile::Anthropic);
        options.auth = AnthropicAuthStyle::Bearer;
        options.headers = AnthropicHeaderStyle::ClaudeCode;
        options
    }

    pub(super) fn glm_anthropic() -> Self {
        let mut options = Self::base(AnthropicProviderProfile::GlmAnthropic);
        options.auth = AnthropicAuthStyle::Bearer;
        options.headers = AnthropicHeaderStyle::ClaudeCode;
        options
    }

    fn base(profile: AnthropicProviderProfile) -> Self {
        Self {
            profile,
            auth: AnthropicAuthStyle::XApiKey,
            headers: AnthropicHeaderStyle::Plain,
            version_header: AnthropicVersionHeader::Required(ANTHROPIC_VERSION),
            endpoint: AnthropicEndpointStyle::V1Messages,
            capabilities: AnthropicCapabilities {
                text: true,
                images: true,
                tools: true,
                parallel_tool_use: true,
                tool_choice: AnthropicToolChoiceSupport::Full,
                thinking: AnthropicThinkingSupport::AnthropicNative,
                prompt_cache_control: true,
                system_cache_control: true,
                web_search_tool: true,
                computer_use_tool: false,
                max_tool_result_blocks: None,
            },
            quirks: AnthropicQuirks {
                require_user_message_first: false,
                merge_consecutive_same_role_messages: false,
                allow_empty_text_blocks: false,
                require_tool_result_after_tool_use: true,
                tool_name_regex: Some(r"^[a-zA-Z0-9_-]{1,64}$"),
                usage_shape: AnthropicUsageShape::Anthropic,
                stream_event_shape: AnthropicStreamShape::Anthropic,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::config::ProviderType;

    fn provider(compatibility: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: "claude".to_string(),
            provider_type: ProviderType::AnthropicMessages,
            compatibility: compatibility.map(ToOwned::to_owned),
            base_url: "https://api.anthropic.com/v1".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn default_profile_is_anthropic() {
        let options = AnthropicProviderOptions::from_provider(&provider(None)).unwrap();
        assert_eq!(options.profile, AnthropicProviderProfile::Anthropic);
        assert_eq!(options.auth, AnthropicAuthStyle::Bearer);
        assert_eq!(options.headers, AnthropicHeaderStyle::ClaudeCode);
        assert_eq!(
            options.version_header,
            AnthropicVersionHeader::Required(ANTHROPIC_VERSION)
        );
        assert_eq!(options.endpoint, AnthropicEndpointStyle::V1Messages);
        assert_eq!(
            options.messages_url(&provider(None)),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn explicit_anthropic_profile_is_supported() {
        let options = AnthropicProviderOptions::from_provider(&provider(Some("anthropic")))
            .expect("profile should parse");
        assert_eq!(options.profile, AnthropicProviderProfile::Anthropic);
    }

    #[test]
    fn claude_alias_maps_to_anthropic_profile() {
        let options = AnthropicProviderOptions::from_provider(&provider(Some("claude")))
            .expect("profile should parse");
        assert_eq!(options.profile, AnthropicProviderProfile::Anthropic);
    }

    #[test]
    fn glm_profile_uses_anthropic_transport() {
        let options = AnthropicProviderOptions::from_provider(&provider(Some("glm_anthropic")))
            .expect("profile should parse");
        assert_eq!(options.profile, AnthropicProviderProfile::GlmAnthropic);
        assert_eq!(options.auth, AnthropicAuthStyle::Bearer);
        assert_eq!(options.headers, AnthropicHeaderStyle::ClaudeCode);
        assert_eq!(
            options.version_header,
            AnthropicVersionHeader::Required(ANTHROPIC_VERSION)
        );
        assert_eq!(
            options.messages_url(&ProviderConfig {
                base_url: "https://open.bigmodel.cn/api/anthropic".to_string(),
                ..provider(Some("glm_anthropic"))
            }),
            "https://open.bigmodel.cn/api/anthropic/v1/messages"
        );
    }

    #[test]
    fn zhipu_alias_maps_to_glm_profile() {
        let options = AnthropicProviderOptions::from_provider(&provider(Some("zhipu_anthropic")))
            .expect("profile should parse");
        assert_eq!(options.profile, AnthropicProviderProfile::GlmAnthropic);
    }

    #[test]
    fn unknown_profile_returns_clear_error() {
        let error = AnthropicProviderOptions::from_provider(&provider(Some("kimi_anthropic")))
            .expect_err("unknown profile should fail until implemented");
        assert!(
            error.message.contains("kimi_anthropic"),
            "unexpected error: {}",
            error.message
        );
    }
}
