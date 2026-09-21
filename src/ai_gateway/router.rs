use std::time::Instant;

use super::config::{
    AiGatewayConfig, ProviderConfig, ProviderType, provider_route_id, select_by_priority,
};
use super::error::GatewayError;
use super::routing_state::GatewayRoutingState;

/// 根据 model 名选择 provider（无状态版本），找不到则返回错误。
pub fn resolve_provider<'a>(
    model: &str,
    session_id: Option<&str>,
    config: &'a AiGatewayConfig,
) -> Result<&'a ProviderConfig, GatewayError> {
    config
        .select_provider_for_session(model, session_id)
        .ok_or_else(|| GatewayError::invalid_model(model))
}

/// 状态感知的路由选择：熔断健康过滤 + 会话粘性 + 权重优先级。
///
/// 顺序：
/// 1. 候选 = enabled && matches_model。
/// 2. healthy = 候选中未被拉黑的。
/// 3. 粘性：若该 session 已绑定某 route 且它在 healthy 中，直接复用（保 cache）。
/// 4. 否则在 healthy 中按优先级（权重降序 + 同权重 HRW）选，并写入绑定。
/// 5. 兜底：healthy 为空（全被拉黑）时，忽略健康在全部候选里按优先级选。
///
/// 返回选中的 provider 及其 `route_id`（供调用方在请求结束时反馈成功/失败）。
pub fn resolve_provider_with_state<'a>(
    model: &str,
    session_id: Option<&str>,
    config: &'a AiGatewayConfig,
    state: &mut GatewayRoutingState,
    now: Instant,
) -> Result<(&'a ProviderConfig, String), GatewayError> {
    resolve_provider_with_state_matching(model, session_id, config, state, now, |_| true)
}

/// 与普通状态路由相同，但只在指定 provider 类型中选择。
pub fn resolve_provider_with_state_for_type<'a>(
    model: &str,
    session_id: Option<&str>,
    config: &'a AiGatewayConfig,
    state: &mut GatewayRoutingState,
    now: Instant,
    provider_type: &ProviderType,
) -> Result<(&'a ProviderConfig, String), GatewayError> {
    resolve_provider_with_state_matching(model, session_id, config, state, now, |provider| {
        &provider.provider_type == provider_type
            || (provider_type == &ProviderType::OpenAiResponses
                && provider.provider_type.is_openai())
    })
}

fn resolve_provider_with_state_matching<'a>(
    model: &str,
    session_id: Option<&str>,
    config: &'a AiGatewayConfig,
    state: &mut GatewayRoutingState,
    now: Instant,
    mut matches: impl FnMut(&ProviderConfig) -> bool,
) -> Result<(&'a ProviderConfig, String), GatewayError> {
    let candidates: Vec<&ProviderConfig> = config
        .providers
        .iter()
        .filter(|provider| provider.enabled && provider.matches_model(model) && matches(provider))
        .collect();
    if candidates.is_empty() {
        return Err(GatewayError::invalid_model(model));
    }

    let session_id = session_id.map(str::trim).filter(|value| !value.is_empty());
    let healthy: Vec<&ProviderConfig> = candidates
        .iter()
        .copied()
        .filter(|provider| !state.is_blacklisted(&provider_route_id(provider), now))
        .collect();

    // 粘性：只要绑定的 route 还健康且仍服务该 model，就继续用它。
    if let Some(sid) = session_id
        && let Some(bound) = state.binding_for(sid, now)
        && let Some(provider) = healthy
            .iter()
            .copied()
            .find(|provider| provider_route_id(provider) == bound)
    {
        let route_id = bound.to_string();
        state.bind(sid, &route_id, now);
        return Ok((provider, route_id));
    }

    // 否则按优先级在健康集里选；全被拉黑时退回到全部候选（宁可试也不 500）。
    let pool = if healthy.is_empty() {
        &candidates
    } else {
        &healthy
    };
    let selected = select_by_priority(pool, session_id).ok_or_else(|| {
        // pool 非空时 select_by_priority 必返回 Some，此分支理论不可达。
        GatewayError::invalid_model(model)
    })?;
    let route_id = provider_route_id(selected);
    if let Some(sid) = session_id {
        state.bind(sid, &route_id, now);
    }
    Ok((selected, route_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::routing_state::{COOLDOWN, FAILURE_THRESHOLD};
    use std::time::Duration;

    fn provider(name: &str, weight: u32, model: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.into(),
            provider_type: ProviderType::OpenAiResponses,
            base_url: format!("https://{name}.example.com"),
            models: vec![model.into()],
            weight,
            ..Default::default()
        }
    }

    fn config(providers: Vec<ProviderConfig>) -> AiGatewayConfig {
        AiGatewayConfig {
            enabled: true,
            providers,
            ..Default::default()
        }
    }

    #[test]
    fn priority_selects_highest_weight() {
        let cfg = config(vec![
            provider("openai", 100, "m"),
            provider("deepseek", 99, "m"),
        ]);
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        let (p, _) = resolve_provider_with_state("m", Some("s1"), &cfg, &mut state, now).unwrap();
        assert_eq!(p.name, "openai");
    }

    #[test]
    fn sticky_keeps_channel_even_if_higher_weight_healthy() {
        let cfg = config(vec![
            provider("openai", 100, "m"),
            provider("deepseek", 99, "m"),
        ]);
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        // 人为把 session 绑定到低权重的 deepseek。
        let deepseek_route = provider_route_id(&cfg.providers[1]);
        state.bind("s1", &deepseek_route, now);
        let (p, _) = resolve_provider_with_state("m", Some("s1"), &cfg, &mut state, now).unwrap();
        // 粘性优先：即使 openai 权重更高且健康，也留在 deepseek。
        assert_eq!(p.name, "deepseek");
    }

    #[test]
    fn blacklisted_channel_fails_over_to_next() {
        let cfg = config(vec![
            provider("openai", 100, "m"),
            provider("deepseek", 99, "m"),
        ]);
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        let openai_route = provider_route_id(&cfg.providers[0]);
        for _ in 0..FAILURE_THRESHOLD {
            state.record_failure(&openai_route, now);
        }
        let (p, _) = resolve_provider_with_state("m", Some("s1"), &cfg, &mut state, now).unwrap();
        assert_eq!(p.name, "deepseek");

        // 冷却后 openai 恢复；但 session 已粘到 deepseek，仍留 deepseek。
        let later = now + COOLDOWN + Duration::from_secs(1);
        let (p2, _) =
            resolve_provider_with_state("m", Some("s1"), &cfg, &mut state, later).unwrap();
        assert_eq!(p2.name, "deepseek");
    }

    #[test]
    fn all_blacklisted_falls_back_to_priority() {
        let cfg = config(vec![
            provider("openai", 100, "m"),
            provider("deepseek", 99, "m"),
        ]);
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        for p in &cfg.providers {
            let route = provider_route_id(p);
            for _ in 0..FAILURE_THRESHOLD {
                state.record_failure(&route, now);
            }
        }
        // 全被拉黑：忽略健康，按优先级兜底选最高权重。
        let (p, _) = resolve_provider_with_state("m", Some("s1"), &cfg, &mut state, now).unwrap();
        assert_eq!(p.name, "openai");
    }

    #[test]
    fn unknown_model_errors() {
        let cfg = config(vec![provider("openai", 100, "m")]);
        let mut state = GatewayRoutingState::default();
        assert!(
            resolve_provider_with_state("nope", Some("s1"), &cfg, &mut state, Instant::now())
                .is_err()
        );
    }

    #[test]
    fn provider_type_filter_ignores_sticky_incompatible_route() {
        let openai = provider("openai", 50, "m");
        let mut grok = provider("grok", 100, "m");
        grok.provider_type = ProviderType::GrokResponses;
        let cfg = config(vec![openai, grok]);
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        let grok_route = provider_route_id(&cfg.providers[1]);
        state.bind("s1", &grok_route, now);

        let (selected, _) = resolve_provider_with_state_for_type(
            "m",
            Some("s1"),
            &cfg,
            &mut state,
            now,
            &ProviderType::OpenAiResponses,
        )
        .unwrap();

        assert_eq!(selected.name, "openai");
    }
}
