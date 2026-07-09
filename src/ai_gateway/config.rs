use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const DEFAULT_PROVIDER_TIMEOUT_SECS: u64 = 600;
const DEFAULT_PROVIDER_WEIGHT: u32 = 100;

/// AI Gateway 顶层配置，对应 config.toml 中 `[aiGateway]` 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiGatewayConfig {
    pub enabled: bool,
    /// 兼容旧配置的遗留字段；路由不使用默认/兜底 provider。
    #[serde(skip_serializing)]
    pub default_provider: String,
    /// 全局 prompt_cache_retention 值（如 "1h"），可选。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<String>,
    /// provider 列表。
    pub providers: Vec<ProviderConfig>,
    /// Codex App 前端模型选择器可见的模型列表。
    ///
    /// 该列表只控制 `/models` 暴露给 Codex App 的 catalog 模型，不参与上游 provider 路由。
    pub codex_visible_models: Vec<String>,
    /// 是否过滤 Codex 请求中的 image_generation tool。
    pub filter_image_generation_tool: bool,
    /// 是否启用请求日志记录。
    #[serde(default = "default_false")]
    pub request_logging_enabled: bool,
    /// 是否记录请求/响应/SSE 详情。关闭时仍保留摘要指标。
    #[serde(default = "default_false")]
    pub request_log_details_enabled: bool,
}

fn default_false() -> bool {
    false
}

impl Default for AiGatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_provider: String::new(),
            prompt_cache_retention: None,
            providers: Vec::new(),
            codex_visible_models: Vec::new(),
            filter_image_generation_tool: false,
            request_logging_enabled: false,
            request_log_details_enabled: false,
        }
    }
}

impl AiGatewayConfig {
    /// 按 model 名选择已启用的 provider：匹配显式 model 或 model alias。
    pub fn select_provider(&self, model: &str) -> Option<&ProviderConfig> {
        self.select_provider_for_session(model, None)
    }

    /// 按 model 名和 session_id 选择已启用的 provider（无路由状态版本）。
    ///
    /// 权重语义是**优先级**：权重最高的 provider 优先命中；权重相同的一组内用
    /// Rendezvous/HRW Hash 按 session 稳定分流（无 session 时用 route_id 稳定排序）。
    /// 该版本不感知熔断/粘性，仅用于不需要状态的调用点。
    pub fn select_provider_for_session(
        &self,
        model: &str,
        session_id: Option<&str>,
    ) -> Option<&ProviderConfig> {
        let candidates: Vec<&ProviderConfig> = self
            .providers
            .iter()
            .filter(|provider| provider.enabled && provider.matches_model(model))
            .collect();
        select_by_priority(&candidates, session_id)
    }
}

/// 在候选 provider 中按「优先级（权重降序）+ 同权重组内 HRW 分流」选择一个。
///
/// 这是路由的核心排序：先比 `effective_weight`（高者胜），权重相同再比 HRW 分数
/// （有 session 时按 session 稳定散列，无 session 时退化为 route_id 字典序）。
pub(crate) fn select_by_priority<'a>(
    candidates: &[&'a ProviderConfig],
    session_id: Option<&str>,
) -> Option<&'a ProviderConfig> {
    let session_id = session_id.map(str::trim).filter(|value| !value.is_empty());
    candidates.iter().copied().max_by(|left, right| {
        left.effective_weight()
            .cmp(&right.effective_weight())
            .then_with(|| match session_id {
                Some(sid) => hrw_score(sid, left).total_cmp(&hrw_score(sid, right)),
                None => Ordering::Equal,
            })
            .then_with(|| provider_route_id(left).cmp(&provider_route_id(right)))
    })
}

/// 同权重组内的稳定分流分数（越大越优先）。仅用于打散权重相同的候选，不再跨权重
/// 做按比例分流。
fn hrw_score(session_id: &str, provider: &ProviderConfig) -> f64 {
    let hash = hrw_hash_u64(session_id, provider);
    ((hash as f64) + 1.0) / ((u64::MAX as f64) + 1.0)
}

fn hrw_hash_u64(session_id: &str, provider: &ProviderConfig) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"codexhub-ai-gateway-hrw-v2\0");
    hasher.update(session_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(provider_route_id(provider).as_bytes());
    let digest = hasher.finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("sha256 digest has 32 bytes"))
}

pub(crate) fn provider_route_id(provider: &ProviderConfig) -> String {
    let name = provider.name.trim();
    let base_url = provider.base_url.trim();
    format!(
        "{}\0{}\0{}",
        name,
        provider.provider_type.route_key(),
        base_url
    )
}

impl ProviderType {
    fn route_key(&self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai_responses",
            Self::GrokResponses => "grok_responses",
            Self::ChatCompletions => "chat_completions",
            Self::AnthropicMessages => "anthropic_messages",
        }
    }
}

/// 将 provider Base URL 规范成 API 根地址。
///
/// UI 允许用户填 `https://api.example.com` 或 `https://api.example.com/v1`；
/// 出站请求统一在这里去掉末尾 `/v1`，再拼具体路径。
pub fn provider_api_root(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.to_ascii_lowercase().ends_with("/v1") {
        base[..base.len() - 3].trim_end_matches('/').to_string()
    } else {
        base.to_string()
    }
}

/// 将 provider Base URL 规范成 UI/配置展示用地址。
///
/// 目前支持的上游协议都使用 `/v1`，所以保存时统一展示为带 `/v1`。
pub fn provider_display_base_url(base_url: &str) -> String {
    let root = provider_api_root(base_url);
    if root.is_empty() {
        String::new()
    } else {
        format!("{root}/v1")
    }
}

/// 单个 provider 配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderConfig {
    /// provider 名称标识（如 "openai"、"deepseek"）。
    pub name: String,
    /// 是否启用该 provider。
    pub enabled: bool,
    /// provider 类型：`"openai_responses"`、`"grok_responses"`、`"chat_completions"` 或 `"anthropic_messages"`。
    pub provider_type: ProviderType,
    /// provider 兼容 profile。Anthropic Messages 兼容厂商优先使用该字段表达差异。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// 上游 API base URL。
    pub base_url: String,
    /// 可选的模型列表 API URL。为空时 GUI 按 base_url 推导 `/models`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_url: Option<String>,
    /// API key。
    pub api_key: String,
    /// 该 provider 支持的 model 列表（精确匹配用）。
    pub models: Vec<String>,
    /// Codex 侧 model 到上游 provider model 的映射。
    ///
    /// 例如 `glm-5.2 = "GLM-5.2"`。路由使用 key 匹配 Codex 请求，出站使用 value。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub model_aliases: BTreeMap<String, String>,
    /// 可选的 prompt_cache_retention 覆盖。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<String>,
    /// 同一 model 命中多个 provider 时的路由权重。实际路由中最小按 1 处理。
    pub weight: u32,
    /// 上游请求超时（秒）。
    pub timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            provider_type: ProviderType::OpenAiResponses,
            compatibility: None,
            base_url: String::new(),
            models_url: None,
            api_key: String::new(),
            models: Vec::new(),
            model_aliases: BTreeMap::new(),
            prompt_cache_retention: None,
            weight: DEFAULT_PROVIDER_WEIGHT,
            timeout_secs: DEFAULT_PROVIDER_TIMEOUT_SECS,
        }
    }
}

impl ProviderConfig {
    pub fn effective_weight(&self) -> u32 {
        self.weight.max(1)
    }

    pub fn matches_model(&self, model: &str) -> bool {
        self.resolve_upstream_model(model).is_some()
    }

    pub fn resolve_upstream_model<'a>(&'a self, model: &'a str) -> Option<&'a str> {
        if let Some(mapped) = self.model_aliases.get(model).map(String::as_str) {
            return Some(mapped);
        }
        if let Some((_, mapped)) = self
            .model_aliases
            .iter()
            .find(|(codex_model, _)| codex_model.eq_ignore_ascii_case(model))
        {
            return Some(mapped.as_str());
        }
        if let Some(configured) = self
            .models
            .iter()
            .find(|configured| configured.as_str() == model)
        {
            return Some(configured.as_str());
        }
        self.models
            .iter()
            .find(|configured| configured.eq_ignore_ascii_case(model))
            .map(String::as_str)
    }
}

/// Provider 类型枚举。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// OpenAI Responses API 透传。
    OpenAiResponses,
    /// Grok/xAI Responses API 透传，带 Grok 专用兼容处理。
    GrokResponses,
    /// Chat Completions API（DeepSeek 等）。
    ChatCompletions,
    /// Anthropic Messages API（Claude）。
    AnthropicMessages,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(name: &str, ptype: ProviderType, models: Vec<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.into(),
            provider_type: ptype,
            models: models.into_iter().map(|s| s.into()).collect(),
            ..Default::default()
        }
    }

    fn make_config(providers: Vec<ProviderConfig>) -> AiGatewayConfig {
        AiGatewayConfig {
            enabled: true,
            providers,
            ..Default::default()
        }
    }

    #[test]
    fn test_exact_match() {
        let config = make_config(vec![
            make_provider(
                "openai",
                ProviderType::OpenAiResponses,
                vec!["gpt-4o", "gpt-4.1"],
            ),
            make_provider(
                "deepseek",
                ProviderType::ChatCompletions,
                vec!["deepseek-v4-flash", "deepseek-v4-pro"],
            ),
        ]);
        let p = config.select_provider("deepseek-v4-flash").unwrap();
        assert_eq!(p.name, "deepseek");

        let p = config.select_provider("gpt-4o").unwrap();
        assert_eq!(p.name, "openai");
    }

    #[test]
    fn test_empty_model_list_does_not_match_provider_name_prefix() {
        let config = make_config(vec![
            make_provider("openai", ProviderType::OpenAiResponses, vec![]),
            make_provider("deepseek", ProviderType::ChatCompletions, vec![]),
        ]);
        assert!(config.select_provider("deepseek-v3").is_none());
        assert!(config.select_provider("deepseek-v4-flash").is_none());
    }

    #[test]
    fn test_no_fallback_even_with_legacy_default_provider() {
        let config = make_config(vec![
            make_provider("openai", ProviderType::OpenAiResponses, vec!["gpt-4o"]),
            make_provider(
                "deepseek",
                ProviderType::ChatCompletions,
                vec!["deepseek-v4-flash"],
            ),
        ]);
        let config = AiGatewayConfig {
            default_provider: "openai".into(),
            ..config
        };
        assert!(config.select_provider("claude-sonnet-4").is_none());
    }

    #[test]
    fn test_no_match_no_default() {
        let config = make_config(vec![make_provider(
            "openai",
            ProviderType::OpenAiResponses,
            vec!["gpt-4o"],
        )]);
        assert!(config.select_provider("unknown-model").is_none());
    }

    #[test]
    fn test_exact_match_uses_configured_model_not_provider_name() {
        let config = make_config(vec![
            make_provider("deepseek", ProviderType::ChatCompletions, vec![]),
            make_provider(
                "other",
                ProviderType::OpenAiResponses,
                vec!["deepseek-v4-flash"],
            ),
        ]);
        let p = config.select_provider("deepseek-v4-flash").unwrap();
        assert_eq!(p.name, "other");
    }

    #[test]
    fn test_model_match_is_case_insensitive() {
        let config = make_config(vec![make_provider(
            "glm",
            ProviderType::AnthropicMessages,
            vec!["GLM-5.2"],
        )]);

        let provider = config.select_provider("glm-5.2").unwrap();
        assert_eq!(provider.name, "glm");
        assert_eq!(provider.resolve_upstream_model("glm-5.2"), Some("GLM-5.2"));
    }

    #[test]
    fn test_model_alias_routes_to_provider_model() {
        let mut provider =
            make_provider("glm", ProviderType::AnthropicMessages, vec!["xx-GLM-5-2"]);
        provider
            .model_aliases
            .insert("glm-5.2".to_string(), "xx-GLM-5-2".to_string());
        let config = make_config(vec![provider]);

        let provider = config.select_provider("glm-5.2").unwrap();
        assert_eq!(provider.name, "glm");
        assert_eq!(
            provider.resolve_upstream_model("glm-5.2"),
            Some("xx-GLM-5-2")
        );
    }

    #[test]
    fn test_disabled_provider_is_not_selected() {
        let mut provider = make_provider(
            "deepseek",
            ProviderType::ChatCompletions,
            vec!["deepseek-v4-flash"],
        );
        provider.enabled = false;
        let config = make_config(vec![provider]);

        assert!(config.select_provider("deepseek-v4-flash").is_none());
        assert!(config.select_provider("deepseek-v3").is_none());
    }

    #[test]
    fn test_session_provider_selection_is_stable() {
        let config = make_config(vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-c", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
        ]);

        let first = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();
        let second = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();

        assert_eq!(first, second);
    }

    #[test]
    fn test_session_provider_selection_honors_weight() {
        let mut low = make_provider("low", ProviderType::OpenAiResponses, vec!["gpt-5.5"]);
        low.weight = 1;
        let mut high = make_provider("high", ProviderType::OpenAiResponses, vec!["gpt-5.5"]);
        high.weight = 100;
        let config = make_config(vec![low, high]);

        let high_count = (0..200)
            .filter(|idx| {
                config
                    .select_provider_for_session("gpt-5.5", Some(&format!("session-{idx}")))
                    .is_some_and(|provider| provider.name == "high")
            })
            .count();

        assert!(high_count > 180, "high provider selected {high_count}/200");
    }

    #[test]
    fn weight_is_priority_not_proportional() {
        // 复现用户报的 bug：100 vs 99 应始终选权重高的（优先级语义），
        // 而不是按比例 ~50/50 分流。
        let mut openai = make_provider(
            "openai",
            ProviderType::OpenAiResponses,
            vec!["gpt-5.4-mini"],
        );
        openai.weight = 100;
        let mut deepseek = make_provider(
            "deepseek",
            ProviderType::ChatCompletions,
            vec!["deepseek-v4-flash"],
        );
        deepseek.weight = 99;
        deepseek
            .model_aliases
            .insert("gpt-5.4-mini".into(), "deepseek-v4-flash".into());
        let config = make_config(vec![openai, deepseek]);

        for idx in 0..200 {
            let picked = config
                .select_provider_for_session("gpt-5.4-mini", Some(&format!("s-{idx}")))
                .unwrap();
            assert_eq!(picked.name, "openai", "session s-{idx} should hit openai");
        }
    }

    #[test]
    fn equal_weight_distributes_by_session() {
        // 权重相同的一组内按 session 稳定分流，两个都应拿到一部分流量。
        let mut a = make_provider("a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]);
        a.weight = 50;
        let mut b = make_provider("b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]);
        b.weight = 50;
        let config = make_config(vec![a, b]);

        let a_count = (0..200)
            .filter(|idx| {
                config
                    .select_provider_for_session("gpt-5.5", Some(&format!("s-{idx}")))
                    .is_some_and(|p| p.name == "a")
            })
            .count();
        assert!(
            (40..=160).contains(&a_count),
            "equal weights should split, a got {a_count}/200"
        );

        // 同一 session 始终稳定映射到同一个 provider（确定性）。
        let first = config
            .select_provider_for_session("gpt-5.5", Some("stable"))
            .unwrap()
            .name
            .clone();
        for _ in 0..10 {
            let again = config
                .select_provider_for_session("gpt-5.5", Some("stable"))
                .unwrap();
            assert_eq!(again.name, first);
        }
    }

    #[test]
    fn test_zero_weight_is_treated_as_one() {
        let mut provider = make_provider(
            "deepseek",
            ProviderType::ChatCompletions,
            vec!["deepseek-v4-flash"],
        );
        provider.weight = 0;
        let config = make_config(vec![provider]);

        assert_eq!(
            config.select_provider("deepseek-v4-flash").unwrap().name,
            "deepseek"
        );
        assert_eq!(config.providers[0].effective_weight(), 1);
    }

    #[test]
    fn test_session_provider_selection_is_independent_of_config_order() {
        let providers = vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-c", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-d", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
        ];
        let config = make_config(providers.clone());
        let reversed = make_config(providers.into_iter().rev().collect());

        let selected = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();
        let selected_reversed = reversed
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();

        assert_eq!(selected, selected_reversed);
    }

    #[test]
    fn test_session_provider_selection_ignores_unmatched_models() {
        let config = make_config(vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("other", ProviderType::OpenAiResponses, vec!["gpt-4o"]),
        ]);

        let selected = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap();

        assert_ne!(selected.name, "other");
    }

    #[test]
    fn test_session_provider_selection_stays_when_non_selected_provider_removed() {
        let providers = vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-c", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-d", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
        ];
        let config = make_config(providers.clone());
        let selected = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();
        let removed_name = providers
            .iter()
            .find(|provider| provider.name != selected)
            .unwrap()
            .name
            .clone();
        let pruned = make_config(
            providers
                .into_iter()
                .filter(|provider| provider.name != removed_name)
                .collect(),
        );

        let selected_after_removal = pruned
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();

        assert_eq!(selected, selected_after_removal);
    }

    #[test]
    fn test_missing_session_id_uses_highest_weight() {
        let mut low = make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]);
        low.weight = 10;
        let mut high = make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]);
        high.weight = 100;
        let config = make_config(vec![low, high]);

        assert_eq!(
            config
                .select_provider_for_session("gpt-5.5", None)
                .unwrap()
                .name,
            "openai-b"
        );
        assert_eq!(
            config
                .select_provider_for_session("gpt-5.5", Some("   "))
                .unwrap()
                .name,
            "openai-b"
        );
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
            enabled = true
            defaultProvider = "openai"
            [[providers]]
            name = "openai"
            providerType = "open_ai_responses"
            baseUrl = "https://api.openai.com"
            apiKey = "sk-xxx"
            models = ["gpt-4o"]
            timeoutSecs = 120

            [[providers]]
            name = "deepseek"
            providerType = "chat_completions"
            baseUrl = "https://api.deepseek.com"
            apiKey = "sk-yyy"
            models = ["deepseek-v4-flash"]

            [[providers]]
            name = "anthropic"
            providerType = "anthropic_messages"
            compatibility = "anthropic"
            baseUrl = "https://api.anthropic.com"
            apiKey = "sk-ant"
            models = ["claude-sonnet-4-6"]

            [[providers]]
            name = "glm"
            providerType = "anthropic_messages"
            compatibility = "glm_anthropic"
            baseUrl = "https://open.bigmodel.cn/api/anthropic"
            modelsUrl = "https://open.bigmodel.cn/api/paas/v4/models"
            apiKey = "sk-glm"
            models = ["glm-4.6"]
            modelAliases = { "glm-5.2" = "GLM-5.2" }

            [[providers]]
            name = "grok"
            providerType = "grok_responses"
            baseUrl = "https://api.x.ai/v1"
            apiKey = "xai-xxx"
            models = ["grok-4.5"]
        "#;
        let config: AiGatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.providers.len(), 5);
        assert_eq!(
            config.providers[0].provider_type,
            ProviderType::OpenAiResponses
        );
        assert_eq!(
            config.providers[1].provider_type,
            ProviderType::ChatCompletions
        );
        assert_eq!(
            config.providers[2].provider_type,
            ProviderType::AnthropicMessages
        );
        assert_eq!(
            config.providers[3].provider_type,
            ProviderType::AnthropicMessages
        );
        assert_eq!(
            config.providers[4].provider_type,
            ProviderType::GrokResponses
        );
        assert_eq!(config.providers[0].timeout_secs, 120);
        assert_eq!(
            config.providers[1].timeout_secs,
            DEFAULT_PROVIDER_TIMEOUT_SECS
        );
        assert_eq!(config.providers[0].weight, DEFAULT_PROVIDER_WEIGHT);
        assert_eq!(config.providers[1].weight, DEFAULT_PROVIDER_WEIGHT);
        assert_eq!(config.providers[0].compatibility, None);
        assert!(!config.request_log_details_enabled);
        assert_eq!(
            config.providers[2].compatibility.as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            config.providers[3].compatibility.as_deref(),
            Some("glm_anthropic")
        );
        assert_eq!(
            config.providers[3].models_url.as_deref(),
            Some("https://open.bigmodel.cn/api/paas/v4/models")
        );
        assert_eq!(
            config.providers[3]
                .model_aliases
                .get("glm-5.2")
                .map(String::as_str),
            Some("GLM-5.2")
        );
    }

    #[test]
    fn request_log_details_default_to_disabled() {
        let config = AiGatewayConfig::default();
        assert!(!config.request_logging_enabled);
        assert!(!config.request_log_details_enabled);
    }

    #[test]
    fn test_provider_api_root_accepts_versioned_or_root_base_url() {
        assert_eq!(
            provider_api_root("https://api.deepseek.com"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            provider_api_root("https://api.deepseek.com/v1"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            provider_api_root("https://proxy.example.com/openai/v1/"),
            "https://proxy.example.com/openai"
        );
    }

    #[test]
    fn test_provider_display_base_url_uses_v1() {
        assert_eq!(
            provider_display_base_url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            provider_display_base_url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(provider_display_base_url(""), "");
    }
}
