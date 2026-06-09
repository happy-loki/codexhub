use crate::im::core::{i18n::ImText, thread::ThreadCreateDefaults};

use super::super::common::build_markdown_card;
use super::super::markdown::normalize_card_markdown;
use super::common::build_interactive_choice_block;
pub fn build_thread_create_settings_card(
    request_id: &str,
    defaults: &ThreadCreateDefaults,
    text: ImText,
) -> serde_json::Value {
    let cwd_options = thread_cwd_options(defaults, text);
    let model_options = thread_model_options(defaults, text);
    let effort_options = thread_effort_options(defaults, text);
    let permission_options = thread_permission_options(text);
    let default_line = |label: &str, value: Option<String>| {
        text.field_line(
            label,
            &value
                .as_deref()
                .map(normalize_card_markdown)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| text.codex_app_default_value().to_string()),
        )
    };
    let remote_line = text.field_line(
        text.remote_label(),
        &defaults
            .remote_name
            .as_ref()
            .map(|value| normalize_card_markdown(value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| text.not_connected().to_string()),
    );
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": text.create_settings_card_intro()
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!(
                "<font color='grey'>{}\n{}\n{}\n{}\n{}\n{}</font>",
                remote_line,
                default_line(text.cwd_label(), defaults.cwd.clone()),
                default_line(text.provider_label(), defaults.model_provider.clone()),
                default_line(text.model_label(), defaults.model.clone()),
                default_line(text.effort_label(), defaults.effort.clone()),
                default_line(
                    text.permission_label_title(),
                    defaults
                        .permission
                        .as_deref()
                        .map(|permission| text.permission_label(permission))
                )
            )
        }),
        serde_json::json!({
            "tag": "form",
            "name": "thread_create_form",
            "element_id": "thread_create_form",
            "direction": "vertical",
            "vertical_spacing": "8px",
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!("**{}**", text.cwd_section())
                },
                select_static_element(
                    "cwd_choice",
                    text.cwd_select_placeholder(),
                    cwd_options
                ),
                {
                    "tag": "input",
                    "name": "cwd_custom",
                    "required": false,
                    "placeholder": {
                        "tag": "plain_text",
                        "content": text.cwd_custom_placeholder()
                    }
                },
                {
                    "tag": "markdown",
                    "content": format!("**{}**", text.model_section())
                },
                select_static_element(
                    "model",
                    text.model_select_placeholder(),
                    model_options
                ),
                {
                    "tag": "markdown",
                    "content": format!("**{}**", text.effort_section())
                },
                select_static_element(
                    "effort",
                    text.effort_select_placeholder(),
                    effort_options
                ),
                {
                    "tag": "markdown",
                    "content": format!("**{}**", text.permission_section())
                },
                select_static_element(
                    "permission",
                    text.permission_select_placeholder(),
                    permission_options
                ),
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "auto",
                            "elements": [
                                {
                                    "tag": "button",
                                    "type": "primary",
                                    "text": {
                                        "tag": "plain_text",
                                        "content": text.confirm_create_button()
                                    },
                                    "name": "submit_thread_create",
                                    "form_action_type": "submit",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "thread_route_create_submit",
                                                "requestId": request_id
                                            }
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        }),
    ];
    elements.push(build_interactive_choice_block(
        text.create_default_button(),
        text.create_default_description(),
        serde_json::json!({
            "kind": "thread_route_create_default",
            "requestId": request_id
        }),
        false,
        false,
        false,
        text,
    ));
    elements.push(build_interactive_choice_block(
        text.back_button(),
        text.back_description(),
        serde_json::json!({
            "kind": "thread_route_choice",
            "requestId": request_id,
            "action": "back"
        }),
        false,
        false,
        false,
        text,
    ));

    let mut card = build_markdown_card("", Some(text.create_settings_card_title()), Some("indigo"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

fn select_static_element(
    name: &str,
    placeholder: &str,
    options: Vec<(String, String)>,
) -> serde_json::Value {
    serde_json::json!({
        "tag": "select_static",
        "name": name,
        "required": false,
        "placeholder": {
            "tag": "plain_text",
            "content": placeholder
        },
        "options": options
            .into_iter()
            .map(|(label, value)| {
                serde_json::json!({
                    "text": {
                        "tag": "plain_text",
                        "content": label
                    },
                    "value": value
                })
            })
            .collect::<Vec<_>>()
    })
}

fn thread_cwd_options(defaults: &ThreadCreateDefaults, text: ImText) -> Vec<(String, String)> {
    let mut options = vec![(
        text.use_default_cwd().to_string(),
        "__default__".to_string(),
    )];
    for project in defaults
        .projects
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .take(20)
    {
        options.push((project_option_label_with_path(project), project.to_string()));
    }
    options.push((
        text.custom_cwd_label().to_string(),
        "__custom__".to_string(),
    ));
    dedupe_options(options)
}

fn thread_model_options(defaults: &ThreadCreateDefaults, text: ImText) -> Vec<(String, String)> {
    let mut options = vec![(
        text.use_current_model().to_string(),
        "__default__".to_string(),
    )];
    for model in defaults
        .models
        .iter()
        .filter(|value| !value.value.trim().is_empty())
    {
        options.push((model.label.clone(), model.value.clone()));
    }
    dedupe_options(options)
}

fn thread_effort_options(defaults: &ThreadCreateDefaults, text: ImText) -> Vec<(String, String)> {
    let mut options = vec![(
        text.use_model_default_effort().to_string(),
        "__default__".to_string(),
    )];
    if let Some(effort) = defaults.effort.as_deref().map(str::trim)
        && !effort.is_empty()
    {
        options.push((text.reasoning_effort_label(effort), effort.to_string()));
    }
    for effort in defaults
        .efforts
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        options.push((text.reasoning_effort_label(effort), effort.to_string()));
    }
    dedupe_options(options)
}

fn thread_permission_options(text: ImText) -> Vec<(String, String)> {
    vec![
        (
            text.default_permission_label().to_string(),
            "workspace_user".to_string(),
        ),
        (
            text.auto_review_label().to_string(),
            "auto_review".to_string(),
        ),
        (
            text.full_access_label().to_string(),
            "full_access".to_string(),
        ),
    ]
}

fn dedupe_options(options: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut output = Vec::new();
    for (label, value) in options {
        if !output
            .iter()
            .any(|(_, existing_value): &(String, String)| existing_value == &value)
        {
            output.push((label, value));
        }
    }
    output
}

fn project_option_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn project_option_label_with_path(path: &str) -> String {
    let name = project_option_label(path);
    if name == path {
        path.to_string()
    } else {
        format!("{name} - {path}")
    }
}
