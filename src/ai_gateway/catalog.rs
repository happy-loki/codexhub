use std::collections::HashSet;

use once_cell::sync::Lazy;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::config::AiGatewayConfig;

static BASE_MODEL_CATALOG: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(include_str!("models.json")).expect("embedded AI Gateway model catalog")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModelOption {
    pub slug: String,
    pub display_name: String,
    pub description: String,
}

pub fn visible_catalog_model_options() -> Vec<CatalogModelOption> {
    catalog_models()
        .iter()
        .filter(|model| is_catalog_model_visible(model))
        .filter_map(|model| {
            let slug = model_slug(model)?.to_string();
            let display_name = model
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(&slug)
                .to_string();
            let description = model
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(CatalogModelOption {
                slug,
                display_name,
                description,
            })
        })
        .collect()
}

#[cfg(test)]
pub fn configured_models_response(config: &AiGatewayConfig) -> Value {
    build_configured_models_response(config)
}

pub fn configured_models_etag(config: &AiGatewayConfig) -> String {
    let response = build_configured_models_response(config);
    configured_models_etag_from_response(&response)
}

pub fn configured_models_response_with_etag(config: &AiGatewayConfig) -> (Value, String) {
    let response = build_configured_models_response(config);
    let etag = configured_models_etag_from_response(&response);
    (response, etag)
}

fn build_configured_models_response(config: &AiGatewayConfig) -> Value {
    let catalog_models = catalog_models();

    let mut emitted = HashSet::new();
    let mut models = Vec::new();
    let mut priority = 0;

    for model_id in selected_codex_model_ids(config) {
        if !emitted.insert(model_id.clone()) {
            continue;
        }

        let model = catalog_models
            .iter()
            .find(|model| {
                model_slug(model) == Some(model_id.as_str()) && is_catalog_model_visible(model)
            })
            .cloned();
        let Some(mut model) = model else {
            continue;
        };
        normalize_deepseek_model(&mut model);
        if let Some(object) = model.as_object_mut() {
            object.insert("priority".to_string(), json!(priority));
        }
        priority += 1;
        models.push(model);
    }

    json!({ "models": models })
}

fn catalog_models() -> &'static Vec<Value> {
    BASE_MODEL_CATALOG
        .get("models")
        .and_then(Value::as_array)
        .expect("embedded AI Gateway model catalog must contain models array")
}

fn configured_models_etag_from_response(response: &Value) -> String {
    let serialized = serde_json::to_vec(response)
        .expect("configured models response should always serialize for etag");
    let digest = Sha256::digest(serialized);
    format!("\"sha256:{}\"", hex::encode(digest))
}

fn selected_codex_model_ids(config: &AiGatewayConfig) -> Vec<String> {
    config
        .codex_visible_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn model_slug(model: &Value) -> Option<&str> {
    model.get("slug").and_then(Value::as_str)
}

fn is_catalog_model_visible(model: &Value) -> bool {
    model
        .get("supported_in_api")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && model.get("visibility").and_then(Value::as_str) == Some("list")
}

fn normalize_deepseek_model(model: &mut Value) {
    let Some(slug) = model_slug(model) else {
        return;
    };
    if !slug.starts_with("deepseek-") {
        return;
    }

    if let Some(object) = model.as_object_mut() {
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
    fn config(models: &[&str]) -> AiGatewayConfig {
        AiGatewayConfig {
            codex_visible_models: models.iter().map(|model| model.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn configured_models_response_uses_codex_visible_models() {
        let config = config(&[
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "grok-4.5",
            "gpt-5.5",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "custom-model",
            "codex-auto-review",
        ]);

        let response = configured_models_response(&config);
        let slugs: Vec<&str> = response["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect();

        assert_eq!(
            slugs,
            vec![
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "grok-4.5",
                "gpt-5.5",
                "deepseek-v4-pro",
                "deepseek-v4-flash"
            ]
        );
        assert_eq!(response["models"][3]["display_name"], "Grok-4.5");
        assert_eq!(
            response["models"][3]["comp_hash"],
            "codexhub-grok-summary-v1"
        );
        assert_eq!(response["models"][5]["display_name"], "deepseek-v4-pro");
        assert_eq!(
            response["models"][5]["comp_hash"],
            "codexhub-deepseek-summary-v1"
        );
        assert_eq!(response["models"][5]["apply_patch_tool_type"], "freeform");
        assert_eq!(response["models"][5]["supports_search_tool"], false);
        assert_eq!(
            response["models"][5]["supports_image_detail_original"],
            false
        );
        assert_eq!(response["models"][5]["input_modalities"], json!(["text"]));
    }

    #[test]
    fn configured_models_etag_is_stable_for_same_response() {
        let config = config(&["deepseek-v4-pro", "deepseek-v4-flash"]);

        let (response, etag) = configured_models_response_with_etag(&config);

        assert_eq!(response, configured_models_response(&config));
        assert_eq!(etag, configured_models_etag(&config));
        assert!(etag.starts_with("\"sha256:"));
        assert!(etag.ends_with('"'));
    }

    #[test]
    fn configured_models_etag_changes_when_visible_models_change() {
        let base_config = config(&["deepseek-v4-pro"]);
        let changed_config = config(&["deepseek-v4-pro", "deepseek-v4-flash"]);

        assert_ne!(
            configured_models_etag(&base_config),
            configured_models_etag(&changed_config)
        );
    }

    #[test]
    fn configured_models_response_skips_unknown_configured_model() {
        let config = config(&["custom-model"]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn configured_models_response_skips_hidden_catalog_model() {
        let config = config(&["codex-auto-review"]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn deepseek_models_preserve_apply_patch_tool_from_catalog() {
        let response = configured_models_response(&config(&["deepseek-v4-pro"]));
        let model = &response["models"][0];
        assert_eq!(model["apply_patch_tool_type"], "freeform");
        assert_eq!(model["supports_image_detail_original"], false);
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["web_search_tool_type"], "text");
        assert_eq!(model["supports_search_tool"], false);
    }

    #[test]
    fn configured_models_response_returns_empty_when_no_models_configured() {
        let config = config(&[]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn catalog_model_visibility_requires_api_support_and_list_visibility() {
        assert!(is_catalog_model_visible(&json!({
            "supported_in_api": true,
            "visibility": "list"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "supported_in_api": false,
            "visibility": "list"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "supported_in_api": true,
            "visibility": "hide"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "visibility": "list"
        })));
    }

    #[test]
    fn all_visible_catalog_models_declare_comp_hash() {
        let missing = catalog_models()
            .iter()
            .filter(|model| is_catalog_model_visible(model))
            .filter_map(|model| {
                let comp_hash = model
                    .get("comp_hash")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                comp_hash
                    .is_empty()
                    .then(|| model_slug(model).unwrap_or("<missing-slug>").to_string())
            })
            .collect::<Vec<_>>();

        assert!(missing.is_empty(), "models missing comp_hash: {missing:?}");
    }

    #[test]
    fn non_openai_comp_hashes_follow_protocol_families() {
        let comp_hash = |slug: &str| {
            catalog_models()
                .iter()
                .find(|model| model_slug(model) == Some(slug))
                .and_then(|model| model.get("comp_hash"))
                .and_then(Value::as_str)
                .expect("catalog model should declare comp_hash")
        };

        assert_eq!(comp_hash("grok-4.5"), "codexhub-grok-summary-v1");
        assert_eq!(comp_hash("deepseek-v4-pro"), "codexhub-deepseek-summary-v1");
        assert_eq!(
            comp_hash("deepseek-v4-flash"),
            "codexhub-deepseek-summary-v1"
        );
        assert_eq!(comp_hash("GLM-5.2"), "codexhub-anthropic-summary-v1");
        assert_eq!(comp_hash("Opus-4.8"), "codexhub-anthropic-summary-v1");
        assert_eq!(comp_hash("Sonnet-4.6"), "codexhub-anthropic-summary-v1");

        assert_ne!(comp_hash("grok-4.5"), comp_hash("gpt-5.6-sol"));
        assert_ne!(comp_hash("deepseek-v4-pro"), comp_hash("gpt-5.5"));
        assert_ne!(comp_hash("Opus-4.8"), comp_hash("deepseek-v4-pro"));
    }

    #[test]
    fn non_openai_models_use_372k_context_window() {
        for slug in [
            "grok-4.5",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "GLM-5.2",
            "Opus-4.8",
            "Sonnet-4.6",
        ] {
            let model = catalog_models()
                .iter()
                .find(|model| model_slug(model) == Some(slug))
                .expect("catalog model should exist");

            assert_eq!(model["context_window"], 372_000, "model {slug}");
            assert_eq!(model["max_context_window"], 372_000, "model {slug}");
        }
    }

    #[test]
    fn gpt_5_6_models_use_current_official_capabilities() {
        for (slug, priority) in [
            ("gpt-5.6-sol", 1),
            ("gpt-5.6-terra", 2),
            ("gpt-5.6-luna", 3),
        ] {
            let model = catalog_models()
                .iter()
                .find(|model| model_slug(model) == Some(slug))
                .expect("catalog model should exist");

            assert_eq!(model["context_window"], 272_000, "model {slug}");
            assert_eq!(model["max_context_window"], 272_000, "model {slug}");
            assert_eq!(model["use_responses_lite"], true, "model {slug}");
            assert_eq!(
                model["supports_reasoning_summary_parameter"], true,
                "model {slug}"
            );
            assert_eq!(model["visibility"], "list", "model {slug}");
            assert_eq!(model["priority"], priority, "model {slug}");
        }
    }

    #[test]
    fn gpt_5_4_models_remain_visible_for_codexhub() {
        let gpt_5_5 = catalog_models()
            .iter()
            .find(|model| model_slug(model) == Some("gpt-5.5"))
            .expect("gpt-5.5 should exist");
        assert_eq!(gpt_5_5["visibility"], "list");
        assert_eq!(gpt_5_5["supports_reasoning_summary_parameter"], true);
        assert_eq!(gpt_5_5.get("availability_nux"), Some(&Value::Null));

        for slug in ["gpt-5.4", "gpt-5.4-mini"] {
            let model = catalog_models()
                .iter()
                .find(|model| model_slug(model) == Some(slug))
                .expect("catalog model should exist");

            assert_eq!(model["visibility"], "list", "model {slug}");
            assert_eq!(model["include_skills_usage_instructions"], true);
            assert_eq!(model["supports_reasoning_summary_parameter"], true);
            assert_eq!(model.get("upgrade"), Some(&Value::Null), "model {slug}");
        }
    }

    #[test]
    fn visible_catalog_model_options_returns_listable_api_models() {
        let options = visible_catalog_model_options();
        let slugs = options
            .iter()
            .map(|model| model.slug.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "grok-4.5",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
        ] {
            assert!(
                slugs.contains(&expected),
                "missing visible model {expected}"
            );
        }
        assert!(!slugs.contains(&"gpt-5.2"));
        assert!(
            options
                .iter()
                .all(|model| !model.slug.trim().is_empty() && !model.display_name.trim().is_empty())
        );
    }
}
