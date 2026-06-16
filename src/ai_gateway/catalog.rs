use std::collections::HashSet;

use once_cell::sync::Lazy;
use serde_json::{Value, json};

use super::config::{AiGatewayConfig, ProviderConfig};

static BASE_MODEL_CATALOG: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(include_str!("models.json")).expect("embedded AI Gateway model catalog")
});

pub fn configured_models_response(config: &AiGatewayConfig) -> Value {
    let catalog_models = BASE_MODEL_CATALOG
        .get("models")
        .and_then(Value::as_array)
        .expect("embedded AI Gateway model catalog must contain models array");
    let gpt55_template = catalog_models
        .iter()
        .find(|model| model_slug(model) == Some("gpt-5.5"))
        .expect("embedded AI Gateway model catalog must contain gpt-5.5 template");

    let mut emitted = HashSet::new();
    let mut models = Vec::new();
    let mut priority = 0;

    for model_id in configured_model_ids(config) {
        if !emitted.insert(model_id.clone()) {
            continue;
        }

        let mut model = catalog_models
            .iter()
            .find(|model| model_slug(model) == Some(model_id.as_str()))
            .cloned()
            .unwrap_or_else(|| model_from_template(gpt55_template, &model_id));
        normalize_deepseek_model(&mut model);
        if let Some(object) = model.as_object_mut() {
            object.insert("priority".to_string(), json!(priority));
        }
        priority += 1;
        models.push(model);
    }

    json!({ "models": models })
}

fn configured_model_ids(config: &AiGatewayConfig) -> Vec<String> {
    config
        .providers
        .iter()
        .filter(|provider| provider.enabled)
        .flat_map(provider_models)
        .collect()
}

fn provider_models(provider: &ProviderConfig) -> impl Iterator<Item = String> + '_ {
    provider
        .models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

fn model_slug(model: &Value) -> Option<&str> {
    model.get("slug").and_then(Value::as_str)
}

fn model_from_template(template: &Value, model_id: &str) -> Value {
    let mut model = template.clone();
    if let Some(object) = model.as_object_mut() {
        object.insert("slug".to_string(), json!(model_id));
        object.insert("display_name".to_string(), json!(model_id));
        object.insert("description".to_string(), Value::Null);
        object.insert("availability_nux".to_string(), Value::Null);
    }
    normalize_deepseek_model(&mut model);
    model
}

fn normalize_deepseek_model(model: &mut Value) {
    let Some(slug) = model_slug(model) else {
        return;
    };
    if !slug.starts_with("deepseek-") {
        return;
    }

    if let Some(object) = model.as_object_mut() {
        object.insert("apply_patch_tool_type".to_string(), Value::Null);
        object.insert("web_search_tool_type".to_string(), json!("text"));
        object.insert("supports_search_tool".to_string(), Value::Bool(false));
        object.insert(
            "supports_image_detail_original".to_string(),
            Value::Bool(false),
        );
        object.insert("input_modalities".to_string(), json!(["text"]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::config::{ProviderConfig, ProviderType};

    fn provider(name: &str, enabled: bool, models: &[&str]) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            enabled,
            provider_type: ProviderType::OpenAiResponses,
            models: models.iter().map(|model| model.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn configured_models_response_filters_enabled_provider_models() {
        let config = AiGatewayConfig {
            providers: vec![
                provider("openai", true, &["gpt-5.5"]),
                provider("deepseek", true, &["deepseek-v4-pro", "deepseek-v4-flash"]),
                provider("disabled", false, &["disabled-model"]),
            ],
            ..Default::default()
        };

        let response = configured_models_response(&config);
        let slugs: Vec<&str> = response["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect();

        assert_eq!(
            slugs,
            vec!["gpt-5.5", "deepseek-v4-pro", "deepseek-v4-flash"]
        );
        assert_eq!(response["models"][1]["display_name"], "deepseek-v4-pro");
        assert_eq!(response["models"][1]["apply_patch_tool_type"], Value::Null);
        assert_eq!(response["models"][1]["supports_search_tool"], false);
        assert_eq!(
            response["models"][1]["supports_image_detail_original"],
            false
        );
        assert_eq!(response["models"][1]["input_modalities"], json!(["text"]));
    }

    #[test]
    fn configured_models_response_uses_template_for_unknown_configured_model() {
        let config = AiGatewayConfig {
            providers: vec![provider("custom", true, &["custom-model"])],
            ..Default::default()
        };

        let response = configured_models_response(&config);
        assert_eq!(response["models"][0]["slug"], "custom-model");
        assert_eq!(response["models"][0]["display_name"], "custom-model");
        assert_eq!(response["models"][0]["visibility"], "list");
        assert_eq!(response["models"][0]["supported_in_api"], true);
    }

    #[test]
    fn deepseek_models_disable_apply_patch_tool() {
        let model = model_from_template(gpt55_template(), "deepseek-v4-pro");
        assert_eq!(model["apply_patch_tool_type"], Value::Null);
        assert_eq!(model["supports_image_detail_original"], false);
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["web_search_tool_type"], "text");
        assert_eq!(model["supports_search_tool"], false);
    }

    #[test]
    fn configured_models_response_returns_empty_when_no_models_configured() {
        let config = AiGatewayConfig {
            providers: vec![provider("empty", true, &[])],
            ..Default::default()
        };

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    fn gpt55_template() -> &'static Value {
        BASE_MODEL_CATALOG
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| {
                models
                    .iter()
                    .find(|model| model_slug(model) == Some("gpt-5.5"))
            })
            .expect("gpt-5.5 template")
    }
}
