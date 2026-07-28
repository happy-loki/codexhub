use serde_json::{Value, json};

use super::apply_patch_tool::{
    APPLY_PATCH_TOOL_NAME, GROK_APPLY_PATCH_INPUT_DESCRIPTION, grok_apply_patch_description,
};
use super::config::ProviderType;
use super::tool_names::ToolNameMap;

#[derive(Debug, Default)]
pub struct ResponsesToolPreparation {
    pub carriers_removed: usize,
    pub tools_added: usize,
    pub duplicates_removed: usize,
    pub grok_tools_converted: usize,
    pub grok_hosted_tools_normalized: usize,
    pub grok_tool_names: Option<ToolNameMap>,
}

impl ResponsesToolPreparation {
    pub fn changed(&self) -> bool {
        self.carriers_removed > 0
            || self.tools_added > 0
            || self.duplicates_removed > 0
            || self.grok_tools_converted > 0
            || self.grok_hosted_tools_normalized > 0
    }
}

/// Normalizes Responses tool declarations for the selected upstream.
/// Responses Lite may store client-executed tools in an `additional_tools`
/// input item; standard Responses already supplies them in top-level `tools`.
pub fn prepare_for_provider(
    raw_body: &mut Value,
    provider_type: &ProviderType,
) -> Result<ResponsesToolPreparation, String> {
    if provider_type == &ProviderType::OpenAiResponses {
        return Ok(ResponsesToolPreparation::default());
    }

    let mut preparation = ResponsesToolPreparation::default();
    let additional_tools = extract_additional_tools(raw_body, &mut preparation)?;
    merge_top_level_tools(raw_body, additional_tools, &mut preparation)?;

    if provider_type == &ProviderType::GrokResponses {
        let loaded_tools = extract_tool_search_output_tools(raw_body);
        merge_top_level_tools(raw_body, loaded_tools, &mut preparation)?;
        let mut tool_names = ToolNameMap::default();
        let stats = convert_grok_tools(raw_body, &mut tool_names)?;
        preparation.grok_tools_converted = stats.converted;
        preparation.grok_hosted_tools_normalized = stats.hosted_normalized;
        normalize_grok_tool_choice(raw_body, &mut tool_names);
        if !tool_names.is_empty() {
            preparation.grok_tool_names = Some(tool_names);
        }
    }

    Ok(preparation)
}

fn extract_additional_tools(
    raw_body: &mut Value,
    preparation: &mut ResponsesToolPreparation,
) -> Result<Vec<Value>, String> {
    let Some(input) = raw_body.get_mut("input") else {
        return Ok(Vec::new());
    };
    let Some(items) = input.as_array_mut() else {
        return Ok(Vec::new());
    };

    let mut retained = Vec::with_capacity(items.len());
    let mut tools = Vec::new();
    for item in std::mem::take(items) {
        if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
            retained.push(item);
            continue;
        }

        preparation.carriers_removed += 1;
        match item.get("tools") {
            None | Some(Value::Null) => {}
            Some(Value::Array(additional)) => tools.extend(additional.iter().cloned()),
            Some(_) => return Err("additional_tools.tools must be an array".to_string()),
        }
    }
    *items = retained;
    Ok(tools)
}

fn merge_top_level_tools(
    raw_body: &mut Value,
    additional_tools: Vec<Value>,
    preparation: &mut ResponsesToolPreparation,
) -> Result<(), String> {
    let object = raw_body
        .as_object_mut()
        .ok_or_else(|| "Responses request must be a JSON object".to_string())?;
    let existing_tools_value = object.remove("tools");
    let had_explicit_tools = existing_tools_value.is_some();
    let existing = match existing_tools_value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(tools)) => tools,
        Some(other) => {
            object.insert("tools".to_string(), other);
            return Err("Responses tools must be an array".to_string());
        }
    };

    if existing.is_empty() && additional_tools.is_empty() {
        if had_explicit_tools {
            object.insert("tools".to_string(), Value::Array(Vec::new()));
        }
        return Ok(());
    }

    let existing_count = existing.len();
    let mut merged = Vec::with_capacity(existing_count + additional_tools.len());
    for tool in existing {
        preparation.duplicates_removed += merge_tool(&mut merged, tool).duplicates;
    }
    for tool in additional_tools {
        let outcome = merge_tool(&mut merged, tool);
        preparation.duplicates_removed += outcome.duplicates;
        if outcome.added {
            preparation.tools_added += 1;
        }
    }
    object.insert("tools".to_string(), Value::Array(merged));
    Ok(())
}

fn extract_tool_search_output_tools(raw_body: &Value) -> Vec<Value> {
    raw_body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_search_output"))
        .flat_map(|item| {
            item.get("tools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

#[derive(Default)]
struct MergeOutcome {
    added: bool,
    duplicates: usize,
}

fn merge_tool(target: &mut Vec<Value>, candidate: Value) -> MergeOutcome {
    let identity = tool_identity(&candidate);
    let Some(existing_index) = target
        .iter()
        .position(|existing| tool_identity(existing) == identity)
    else {
        target.push(candidate);
        return MergeOutcome {
            added: true,
            duplicates: 0,
        };
    };

    let mut duplicates = 1;
    if candidate.get("type").and_then(Value::as_str) == Some("namespace") {
        duplicates += merge_namespace_children(&mut target[existing_index], candidate);
    }
    MergeOutcome {
        added: false,
        duplicates,
    }
}

fn merge_namespace_children(existing: &mut Value, mut candidate: Value) -> usize {
    let Some(candidate_tools) = candidate
        .as_object_mut()
        .and_then(|object| object.remove("tools"))
        .and_then(|tools| tools.as_array().cloned())
    else {
        return 0;
    };
    let Some(existing_object) = existing.as_object_mut() else {
        return 0;
    };
    let existing_tools = existing_object
        .entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(existing_tools) = existing_tools.as_array_mut() else {
        return 0;
    };

    candidate_tools
        .into_iter()
        .map(|tool| merge_tool(existing_tools, tool).duplicates)
        .sum()
}

fn tool_identity(tool: &Value) -> String {
    let Some(object) = tool.as_object() else {
        return serde_json::to_string(tool).unwrap_or_default();
    };
    let tool_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    let function = object.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let namespace = object
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or("");

    if !tool_type.is_empty() && (!name.is_empty() || tool_type == "tool_search") {
        format!("{tool_type}\0{namespace}\0{name}")
    } else if !tool_type.is_empty() {
        format!(
            "{tool_type}\0{}",
            serde_json::to_string(tool).unwrap_or_default()
        )
    } else {
        serde_json::to_string(tool).unwrap_or_default()
    }
}

#[derive(Debug, Default)]
struct GrokToolPreparation {
    converted: usize,
    hosted_normalized: usize,
}

fn convert_grok_tools(
    raw_body: &mut Value,
    tool_names: &mut ToolNameMap,
) -> Result<GrokToolPreparation, String> {
    let Some(tools) = raw_body.get_mut("tools").and_then(Value::as_array_mut) else {
        return Ok(GrokToolPreparation::default());
    };
    let original = std::mem::take(tools);
    let mut converted = Vec::with_capacity(original.len());
    let mut stats = GrokToolPreparation::default();

    for tool in original {
        match tool.get("type").and_then(Value::as_str) {
            Some("custom") => {
                if let Some(tool) = grok_custom_tool(&tool, tool_names)? {
                    converted.push(tool);
                    stats.converted += 1;
                }
            }
            Some("namespace") => {
                let namespace = tool.get("name").and_then(Value::as_str).unwrap_or("");
                if let Some(children) = tool.get("tools").and_then(Value::as_array) {
                    for child in children {
                        if child.get("type").and_then(Value::as_str) != Some("function") {
                            continue;
                        }
                        if let Some(tool) = grok_function_tool(child, Some(namespace), tool_names) {
                            converted.push(tool);
                            stats.converted += 1;
                        }
                    }
                }
            }
            Some("function") => {
                if let Some(tool) = grok_function_tool(&tool, None, tool_names) {
                    converted.push(tool);
                }
            }
            Some("web_search") | Some("web_search_preview") => {
                let (tool, changed) = normalize_grok_web_search_tool(tool);
                converted.push(tool);
                stats.hosted_normalized += usize::from(changed);
            }
            Some("tool_search") => {
                if let Some(tool) = grok_tool_search_tool(&tool, tool_names) {
                    converted.push(tool);
                    stats.converted += 1;
                }
            }
            _ => converted.push(tool),
        }
    }

    let mut deduplicated = Vec::with_capacity(converted.len());
    for tool in converted {
        merge_tool(&mut deduplicated, tool);
    }
    *tools = deduplicated;
    Ok(stats)
}

/// Codex uses OpenAI-specific knobs for its hosted search declaration. Grok
/// exposes the same hosted tool through standard Responses, but image search
/// and domain exclusions use xAI's field names.
fn normalize_grok_web_search_tool(tool: Value) -> (Value, bool) {
    let Value::Object(mut object) = tool else {
        return (tool, false);
    };
    let original = object.clone();

    object.insert("type".to_string(), json!("web_search"));

    let image_search_requested = object
        .remove("search_content_types")
        .and_then(|value| value.as_array().cloned())
        .is_some_and(|types| types.iter().any(|value| value.as_str() == Some("image")));
    if image_search_requested && !object.contains_key("enable_image_search") {
        object.insert("enable_image_search".to_string(), json!(true));
    }

    for key in [
        "external_web_access",
        "indexed_web_access",
        "search_context_size",
        "user_location",
    ] {
        object.remove(key);
    }

    if let Some(filters) = object.get_mut("filters").and_then(Value::as_object_mut)
        && !filters.contains_key("excluded_domains")
        && let Some(blocked) = filters.remove("blocked_domains")
    {
        filters.insert("excluded_domains".to_string(), blocked);
    }

    let changed = object != original;
    (Value::Object(object), changed)
}

fn grok_function_tool(
    tool: &Value,
    namespace: Option<&str>,
    tool_names: &mut ToolNameMap,
) -> Option<Value> {
    let object = tool.as_object()?;
    let function = object.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)?;
    let encoded_name = tool_names.encode_function(namespace, name);

    let mut result = object.clone();
    result.remove("function");
    result.remove("namespace");
    result.insert("type".to_string(), json!("function"));
    result.insert("name".to_string(), json!(encoded_name));
    if let Some(function) = function {
        for (key, value) in function {
            if key != "name" {
                result.insert(key.clone(), value.clone());
            }
        }
    }
    if !result.contains_key("parameters")
        && let Some(schema) = result
            .remove("input_schema")
            .or_else(|| result.remove("inputSchema"))
    {
        result.insert("parameters".to_string(), schema);
    }
    Some(Value::Object(result))
}

fn grok_tool_search_tool(tool: &Value, tool_names: &mut ToolNameMap) -> Option<Value> {
    let object = tool.as_object()?;
    let mut result = serde_json::Map::new();
    result.insert("type".to_string(), json!("function"));
    result.insert("name".to_string(), json!(tool_names.encode_tool_search()));
    if let Some(description) = object.get("description") {
        result.insert("description".to_string(), description.clone());
    }
    if let Some(parameters) = object.get("parameters") {
        let mut parameters = parameters.clone();
        normalize_grok_tool_search_parameters(&mut parameters);
        result.insert("parameters".to_string(), parameters);
    }
    Some(Value::Object(result))
}

fn normalize_grok_tool_search_parameters(parameters: &mut Value) {
    let Some(limit) = parameters
        .as_object_mut()
        .and_then(|object| object.get_mut("properties"))
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut("limit"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if limit.get("type").and_then(Value::as_str) == Some("number") {
        limit.insert("type".to_string(), json!("integer"));
    }
}

fn grok_custom_tool(tool: &Value, tool_names: &mut ToolNameMap) -> Result<Option<Value>, String> {
    let Some(object) = tool.as_object() else {
        return Ok(None);
    };
    let Some(name) = object.get("name").and_then(Value::as_str) else {
        return Err("custom tool name is required".to_string());
    };
    let description = if name == APPLY_PATCH_TOOL_NAME {
        Value::String(grok_apply_patch_description())
    } else {
        object
            .get("description")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()))
    };
    let input_description = if name == APPLY_PATCH_TOOL_NAME {
        GROK_APPLY_PATCH_INPUT_DESCRIPTION
    } else {
        "Freeform input for the custom tool."
    };
    let argument_name = if name == APPLY_PATCH_TOOL_NAME {
        "patch"
    } else {
        "input"
    };

    Ok(Some(json!({
        "type": "function",
        "name": tool_names.encode_custom(name),
        "description": description,
        "parameters": {
            "type": "object",
            "properties": {
                (argument_name): {
                    "type": "string",
                    "description": input_description
                }
            },
            "required": [argument_name],
            "additionalProperties": false
        },
        "strict": false
    })))
}

fn normalize_grok_tool_choice(raw_body: &mut Value, tool_names: &mut ToolNameMap) {
    let Some(choice) = raw_body
        .get_mut("tool_choice")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let choice_type = choice
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let function = choice.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| choice.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);

    match (choice_type.as_str(), name) {
        ("tool_search", _) => {
            choice.insert("type".to_string(), json!("function"));
            choice.insert("name".to_string(), json!(tool_names.encode_tool_search()));
            choice.remove("function");
            choice.remove("namespace");
        }
        ("custom", Some(name)) => {
            choice.insert("type".to_string(), json!("function"));
            choice.insert("name".to_string(), json!(tool_names.encode_custom(&name)));
            choice.remove("function");
            choice.remove("namespace");
        }
        ("function", Some(name)) => {
            let namespace = choice
                .get("namespace")
                .and_then(Value::as_str)
                .map(str::to_string);
            choice.insert(
                "name".to_string(),
                json!(tool_names.encode_function(namespace.as_deref(), &name)),
            );
            choice.remove("function");
            choice.remove("namespace");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::tool_names::{ToolCallKind, ToolCallTarget};

    #[test]
    fn openai_keeps_additional_tools_native() {
        let mut body = json!({
            "input": [{"type":"additional_tools","tools":[{"type":"custom","name":"exec"}]}]
        });
        let original = body.clone();

        let preparation = prepare_for_provider(&mut body, &ProviderType::OpenAiResponses).unwrap();

        assert!(!preparation.changed());
        assert_eq!(body, original);
    }

    #[test]
    fn chat_provider_merges_deduplicates_and_removes_carriers() {
        let mut body = json!({
            "tools": [{"type":"function","name":"wait","parameters":{"type":"object"}}],
            "input": [
                {"type":"additional_tools","role":"developer","tools":[
                    {"type":"function","name":"wait","parameters":{"type":"object"}},
                    {"type":"custom","name":"exec"}
                ]},
                {"type":"message","role":"user","content":"run it"}
            ]
        });

        let preparation = prepare_for_provider(&mut body, &ProviderType::ChatCompletions).unwrap();

        assert_eq!(preparation.carriers_removed, 1);
        assert_eq!(preparation.tools_added, 1);
        assert_eq!(preparation.duplicates_removed, 1);
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
        assert_eq!(body["tools"][0]["name"], "wait");
        assert_eq!(body["tools"][1]["name"], "exec");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert_eq!(body["input"][0]["type"], "message");
    }

    #[test]
    fn duplicate_namespaces_merge_children_in_order() {
        let mut body = json!({
            "tools": [{"type":"namespace","name":"browser","tools":[
                {"type":"function","name":"open"}
            ]}],
            "input": [{"type":"additional_tools","tools":[
                {"type":"namespace","name":"browser","tools":[
                    {"type":"function","name":"open"},
                    {"type":"function","name":"click"}
                ]}
            ]}]
        });

        prepare_for_provider(&mut body, &ProviderType::AnthropicMessages).unwrap();

        let children = body["tools"][0]["tools"].as_array().unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["name"], "open");
        assert_eq!(children[1]["name"], "click");
    }

    #[test]
    fn grok_converts_custom_and_namespace_tools_to_functions() {
        let mut body = json!({
            "input": [{"type":"additional_tools","tools":[
                {"type":"custom","name":"exec","description":"Run code"},
                {"type":"namespace","name":"browser","tools":[
                    {"type":"function","name":"open","parameters":{"type":"object"}}
                ]}
            ]}]
        });

        let preparation = prepare_for_provider(&mut body, &ProviderType::GrokResponses).unwrap();

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|tool| tool["type"] == "function"));
        assert_eq!(tools[0]["name"], "exec");
        let browser_name = tools[1]["name"].as_str().unwrap();
        let map = preparation.grok_tool_names.unwrap();
        assert_eq!(
            map.decode("exec"),
            ToolCallTarget {
                kind: ToolCallKind::Custom,
                namespace: None,
                name: "exec".to_string(),
            }
        );
        assert_eq!(
            map.decode(browser_name),
            ToolCallTarget::function(Some("browser"), "open")
        );
        assert!(body["input"].as_array().unwrap().is_empty());
    }

    #[test]
    fn grok_converts_tool_search_and_loaded_tools_to_functions() {
        let mut body = json!({
            "input": [
                {"type":"tool_search_output","call_id":"call_search","status":"completed","execution":"client","tools":[
                    {"type":"namespace","name":"fs","tools":[
                        {"type":"function","name":"read_file","parameters":{"type":"object"}}
                    ]},
                    {"type":"tool_search","description":"Load more tools"}
                ]},
                {"type":"additional_tools","tools":[
                    {"type":"tool_search","description":"Discover client tools","parameters":{
                        "type":"object",
                        "properties":{
                            "query":{"type":"string"},
                            "limit":{"type":"number"}
                        },
                        "required":["query"]
                    }}
                ]}
            ],
            "tool_choice": {"type":"tool_search"}
        });

        let preparation = prepare_for_provider(&mut body, &ProviderType::GrokResponses).unwrap();

        assert_eq!(preparation.carriers_removed, 1);
        assert_eq!(preparation.grok_tools_converted, 2);
        assert_eq!(preparation.duplicates_removed, 1);
        assert_eq!(body["input"].as_array().unwrap().len(), 1);

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|tool| tool["type"] == "function"));

        let map = preparation.grok_tool_names.unwrap();
        let tool_search_name = body["tool_choice"]["name"].as_str().unwrap();
        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(map.decode(tool_search_name), ToolCallTarget::tool_search());
        let search_tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(tool_search_name))
            .expect("tool_search function");
        assert_eq!(
            search_tool["parameters"]["properties"]["limit"]["type"],
            "integer"
        );

        let fs_tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() != Some(tool_search_name))
            .expect("loaded fs tool");
        assert_eq!(
            map.decode(fs_tool["name"].as_str().unwrap()),
            ToolCallTarget::function(Some("fs"), "read_file")
        );
    }

    #[test]
    fn grok_standard_responses_normalizes_apply_patch_and_hosted_search() {
        let mut body = json!({
            "model": "grok-4.5",
            "input": [{"role":"user","content":"update the file and verify online"}],
            "tools": [
                {
                    "type": "custom",
                    "name": "apply_patch",
                    "description": "client freeform patch",
                    "format": {"type":"grammar","syntax":"lark","definition":"start: /.+/"}
                },
                {
                    "type": "web_search",
                    "external_web_access": true,
                    "indexed_web_access": true,
                    "search_context_size": "high",
                    "search_content_types": ["text", "image"],
                    "filters": {
                        "allowed_domains": ["docs.x.ai"],
                        "blocked_domains": ["example.com"]
                    }
                }
            ]
        });

        let preparation = prepare_for_provider(&mut body, &ProviderType::GrokResponses).unwrap();

        assert_eq!(preparation.carriers_removed, 0);
        assert_eq!(preparation.grok_tools_converted, 1);
        assert_eq!(preparation.grok_hosted_tools_normalized, 1);

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "apply_patch");
        assert_eq!(tools[0]["parameters"]["required"], json!(["patch"]));
        assert_eq!(
            tools[0]["parameters"]["properties"]["patch"]["type"],
            "string"
        );
        let description = tools[0]["description"].as_str().unwrap();
        assert!(description.contains("{\"patch\":\"<the entire patch body>\"}"));
        assert!(description.contains("The `patch` value must contain only the patch body"));
        assert!(description.contains("CRITICAL LITERAL-SYNTAX RULE FOR GROK"));
        assert!(description.contains("*** Begin Patch ***"));
        assert!(description.contains("do not blame a non-ASCII or absolute path"));
        assert!(
            tools[0]["parameters"]["properties"]["patch"]["description"]
                .as_str()
                .unwrap()
                .contains("never append a trailing ` ***`")
        );
        assert!(tools[0].get("format").is_none());

        let search = &tools[1];
        assert_eq!(search["type"], "web_search");
        assert_eq!(search["enable_image_search"], true);
        assert_eq!(search["filters"]["allowed_domains"], json!(["docs.x.ai"]));
        assert_eq!(
            search["filters"]["excluded_domains"],
            json!(["example.com"])
        );
        for removed in [
            "external_web_access",
            "indexed_web_access",
            "search_context_size",
            "search_content_types",
        ] {
            assert!(search.get(removed).is_none(), "unexpected field: {removed}");
        }

        let map = preparation.grok_tool_names.unwrap();
        assert_eq!(
            map.decode("apply_patch"),
            ToolCallTarget::custom("apply_patch")
        );
    }

    #[test]
    fn grok_normalizes_web_search_preview_without_forcing_image_search() {
        let mut body = json!({
            "input": [{"role":"user","content":"search"}],
            "tools": [{
                "type": "web_search_preview",
                "external_web_access": true,
                "search_content_types": ["text"]
            }]
        });

        let preparation = prepare_for_provider(&mut body, &ProviderType::GrokResponses).unwrap();

        assert_eq!(preparation.grok_hosted_tools_normalized, 1);
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert!(body["tools"][0].get("enable_image_search").is_none());
    }
}
