use serde_json::Value as JsonValue;

use super::super::common::build_markdown_card;
use super::super::markdown::normalize_card_markdown;
fn extract_plan_title_and_steps(text: &str) -> (Option<String>, Vec<String>) {
    let normalized = normalize_card_markdown(text);
    let mut title = None;
    let mut steps = Vec::new();
    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if title.is_none() && trimmed.starts_with('#') {
            title = Some(trimmed.trim_start_matches('#').trim().to_string());
            continue;
        }
        if let Some(step) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            steps.push(step.to_string());
        }
    }
    (title, steps)
}

pub(in crate::im::feishu::renderer) fn build_plan_item_card(
    item: &JsonValue,
) -> Option<serde_json::Value> {
    let text = item
        .get("text")
        .and_then(|v| v.as_str())
        .map(normalize_card_markdown)?;
    if text.is_empty() {
        return None;
    }
    let (title, steps) = extract_plan_title_and_steps(&text);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": "当前阶段规划如下："
    })];
    if let Some(title) = title.filter(|value| !value.is_empty()) {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": format!("**计划概览**\n{}", normalize_card_markdown(&title))
        }));
    }
    if !steps.is_empty() {
        let step_lines = steps
            .iter()
            .enumerate()
            .map(|(idx, step)| format!("{}. {}", idx + 1, normalize_card_markdown(step)))
            .collect::<Vec<_>>()
            .join("\n");
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": format!("**步骤**\n{}", step_lines)
        }));
    } else {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": text
        }));
    }
    elements.push(serde_json::json!({
        "tag": "hr"
    }));
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": "_状态：计划已更新_"
    }));
    let mut card = build_markdown_card(&text, Some("计划"), Some("indigo"));
    card["body"]["elements"] = serde_json::Value::Array(elements);
    Some(card)
}
