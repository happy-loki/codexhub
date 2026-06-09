use super::common::{build_markdown_card, image_element};
use super::markdown::normalize_card_markdown;

#[allow(dead_code)]
pub fn build_image_generation_summary_card(
    status: &str,
    revised_prompt: Option<&str>,
    saved_path: Option<&str>,
) -> serde_json::Value {
    // User-facing IM summary: only keep the final revised prompt.
    // Everything else (status/path/result) is noisy for chat.
    let _ = status;
    let _ = saved_path;

    // IMPORTANT: do NOT wrap in ``` fences; Feishu renders code blocks with a horizontal scrollbar.
    let content = revised_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_card_markdown)
        .unwrap_or_else(|| "_未提供修订提示词_".to_string());

    build_markdown_card(&content, None, None)
}

#[allow(dead_code)]
pub fn build_status_card(text: &str) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    build_markdown_card(&content, None, None)
}

pub fn build_image_generation_result_card(
    status: &str,
    revised_prompt: Option<&str>,
    saved_path: Option<&str>,
    image_key: &str,
) -> serde_json::Value {
    let mut elements = vec![image_element(image_key, "生成图片")];
    let mut details = vec![format!(
        "**状态**：`{}`",
        normalize_card_markdown(status).trim()
    )];
    if let Some(revised_prompt) = revised_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(format!(
            "**修订提示词**\n{}",
            normalize_card_markdown(revised_prompt)
        ));
    }
    if let Some(saved_path) = saved_path.map(str::trim).filter(|value| !value.is_empty()) {
        details.push(format!(
            "**保存路径**：`{}`",
            normalize_card_markdown(saved_path)
        ));
    }
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": details.join("\n\n")
    }));

    let mut card = build_markdown_card("", Some("图片生成"), Some("orange"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_image_view_result_card(path: &str, image_key: &str) -> serde_json::Value {
    let elements = vec![
        image_element(image_key, "图片预览"),
        serde_json::json!({
            "tag": "markdown",
            "content": format!("**路径**：`{}`", normalize_card_markdown(path))
        }),
    ];
    let mut card = build_markdown_card("", Some("图片"), Some("carmine"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

#[allow(dead_code)]
pub fn build_history_summary_card(text: &str) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let mut card = build_markdown_card(&content, Some("历史摘要"), Some("grey"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card
}
