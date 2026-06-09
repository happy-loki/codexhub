#![allow(dead_code)]

use serde_json::{Value, json};

use crate::im_runtime::ApprovalDecisionOption;

#[derive(Debug, Clone)]
pub struct CodexNotification {
    pub method: String,
    pub params: Option<Value>,
    pub request_id: Option<Value>,
    pub remote_client_key: Option<String>,
    pub remote_client_id: Option<String>,
    pub remote_stream_id: Option<String>,
}

pub fn extract_agent_delta(notification: &CodexNotification) -> Option<String> {
    match notification.method.as_str() {
        "item/agentMessage/delta" | "item/reasoning/summaryTextDelta" => notification
            .params
            .as_ref()
            .and_then(|p| p.get("delta").or_else(|| p.get("text")))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}

pub fn is_agent_message_item(item: &Value) -> bool {
    match item.get("type").and_then(|v| v.as_str()) {
        Some("agentMessage") | Some("agent_message") => true,
        Some("message") => item.get("role").and_then(|v| v.as_str()) == Some("assistant"),
        Some("event_msg") | Some("response_item") => item
            .get("payload")
            .is_some_and(|payload| is_agent_message_item(payload)),
        Some("task_complete") => true,
        _ => false,
    }
}

pub fn extract_agent_message_text(item: &Value) -> Option<String> {
    if !is_agent_message_item(item) {
        return None;
    }
    extract_message_text(item)
}

pub fn extract_turn_reply_text(params: &Value) -> Option<String> {
    extract_direct_turn_reply_text(params)
        .or_else(|| {
            params
                .get("payload")
                .and_then(extract_direct_turn_reply_text)
        })
        .or_else(|| params.get("turn").and_then(extract_direct_turn_reply_text))
        .or_else(|| extract_agent_message_text(params))
        .or_else(|| latest_agent_message_in_items(params))
        .or_else(|| params.get("turn").and_then(latest_agent_message_in_items))
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn latest_agent_message_in_items(value: &Value) -> Option<String> {
    value
        .get("turn")
        .and_then(|turn| turn.get("items"))
        .or_else(|| value.get("items"))
        .and_then(|items| items.as_array())
        .and_then(|items| items.iter().rev().find_map(extract_agent_message_text))
}

fn extract_direct_turn_reply_text(value: &Value) -> Option<String> {
    text_from_fields(
        value,
        &[
            "lastAgentMessage",
            "last_agent_message",
            "agentMessage",
            "agent_message",
        ],
    )
}

fn extract_message_text(value: &Value) -> Option<String> {
    text_from_fields(
        value,
        &[
            "text",
            "message",
            "lastAgentMessage",
            "last_agent_message",
            "outputText",
            "output_text",
        ],
    )
    .or_else(|| value.get("content").and_then(text_from_content_value))
    .or_else(|| value.get("payload").and_then(extract_message_text))
}

fn text_from_fields(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(text_from_content_value))
}

fn text_from_content_value(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return non_empty_text(text);
    }
    if let Some(items) = value.as_array() {
        let parts = items
            .iter()
            .filter_map(text_from_content_entry)
            .collect::<Vec<_>>();
        return (!parts.is_empty()).then(|| parts.join("\n\n"));
    }
    if value.is_object() {
        return text_from_content_entry(value);
    }
    None
}

fn text_from_content_entry(entry: &Value) -> Option<String> {
    if let Some(text) = entry.as_str() {
        return non_empty_text(text);
    }
    text_from_fields(entry, &["text", "content", "message", "value"])
        .or_else(|| entry.get("payload").and_then(extract_message_text))
}

fn non_empty_text(text: &str) -> Option<String> {
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn is_turn_completed(notification: &CodexNotification, turn_id: &str) -> bool {
    notification.method == "turn/completed"
        && notification.params.as_ref().and_then(|p| {
            p.get("turnId").and_then(|v| v.as_str()).or_else(|| {
                p.get("turn")
                    .and_then(|t| t.get("id"))
                    .and_then(|v| v.as_str())
            })
        }) == Some(turn_id)
}

pub fn notification_thread_id(notification: &CodexNotification) -> Option<String> {
    notification
        .params
        .as_ref()
        .and_then(|p| {
            p.get("threadId")
                .and_then(|v| v.as_str())
                .or_else(|| p.get("thread_id").and_then(|v| v.as_str()))
                .or_else(|| {
                    p.get("thread")
                        .and_then(|t| t.get("id"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| {
                    p.get("turn")
                        .and_then(|t| t.get("threadId"))
                        .and_then(|v| v.as_str())
                })
        })
        .map(str::to_string)
}

fn concise_command(params: &Value) -> String {
    params
        .get("command")
        .and_then(|v| {
            if let Some(text) = v.as_str() {
                Some(text.to_string())
            } else {
                v.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            }
        })
        .unwrap_or_default()
}

pub struct ApprovalRequestView {
    pub request_kind: String,
    pub summary: String,
    pub decisions: Vec<ApprovalDecisionOption>,
}

pub fn approval_request_view(notification: &CodexNotification) -> Option<ApprovalRequestView> {
    let params = notification.params.as_ref()?;
    match notification.method.as_str() {
        "item/commandExecution/requestApproval" => {
            let summary = command_approval_summary(params);
            let decisions = command_approval_decisions(params);
            Some(ApprovalRequestView {
                request_kind: "command".to_string(),
                summary,
                decisions,
            })
        }
        "item/fileChange/requestApproval" => {
            let summary = file_change_approval_summary(params);
            let decisions = file_change_approval_decisions();
            Some(ApprovalRequestView {
                request_kind: "fileChange".to_string(),
                summary,
                decisions,
            })
        }
        "applyPatchApproval" => {
            let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            Some(ApprovalRequestView {
                request_kind: "review".to_string(),
                summary: format!("reason: {reason}"),
                decisions: vec![
                    decision_option("Yes, proceed", json!("approved")),
                    decision_option("No, and tell Codex what to do differently", json!("denied")),
                ],
            })
        }
        "execCommandApproval" => {
            let command = concise_command(params);
            let cwd = params.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            Some(ApprovalRequestView {
                request_kind: "review".to_string(),
                summary: format!("command: `{command}`\ncwd: `{cwd}`"),
                decisions: vec![
                    decision_option("Yes, proceed", json!("approved")),
                    decision_option("No, and tell Codex what to do differently", json!("denied")),
                ],
            })
        }
        _ => None,
    }
}

pub fn approval_response(decision: Value) -> Value {
    json!({ "decision": decision })
}

pub fn approval_decision_by_input(
    pending: &crate::im_runtime::PendingApproval,
    input: &str,
) -> Option<(usize, ApprovalDecisionOption)> {
    let normalized = input.trim().to_ascii_lowercase();
    let index = if let Some(number) = normalized.strip_prefix('/') {
        number.parse::<usize>().ok()
    } else {
        normalized.parse::<usize>().ok()
    };
    if let Some(index) = index {
        if index > 0 {
            return pending
                .decisions
                .get(index - 1)
                .cloned()
                .map(|decision| (index, decision));
        }
    }

    let decision = match normalized.as_str() {
        "/y" | "/yes" | "y" | "yes" => pending
            .decisions
            .iter()
            .position(|option| is_accept_decision(&option.decision))
            .map(|index| (index + 1, pending.decisions[index].clone())),
        "/n" | "/no" | "n" | "no" => pending
            .decisions
            .iter()
            .position(|option| is_negative_decision(&option.decision))
            .map(|index| (index + 1, pending.decisions[index].clone())),
        _ => None,
    };
    decision
}

fn command_approval_summary(params: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(reason) = params
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        lines.push(format!("reason: {reason}"));
    }
    if let Some(network) = params.get("networkApprovalContext") {
        let host = network
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let protocol = network
            .get("protocol")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        lines.push(format!("networkApprovalContext: `{protocol}://{host}`"));
    }
    let command = concise_command(params);
    if !command.trim().is_empty() {
        lines.push(format!("command: `{command}`"));
    }
    if let Some(cwd) = params.get("cwd").and_then(|v| v.as_str()) {
        lines.push(format!("cwd: `{cwd}`"));
    }
    if let Some(actions) = command_actions_summary(params) {
        lines.push(actions);
    }
    if let Some(permissions) = additional_permissions_summary(params) {
        lines.push(permissions);
    }
    if let Some(amendment) = execpolicy_amendment_summary(params) {
        lines.push(amendment);
    }
    if let Some(amendments) = network_policy_amendments_summary(params) {
        lines.push(amendments);
    }
    if lines.is_empty() {
        "approval requested".to_string()
    } else {
        lines.join("\n")
    }
}

fn file_change_approval_summary(params: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) {
        lines.push(format!("itemId: `{item_id}`"));
    }
    if let Some(reason) = params
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        lines.push(format!("reason: {reason}"));
    }
    if let Some(grant_root) = params
        .get("grantRoot")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        lines.push(format!("grantRoot: `{grant_root}`"));
    }
    if lines.is_empty() {
        "fileChange approval requested".to_string()
    } else {
        lines.join("\n")
    }
}

fn command_actions_summary(params: &Value) -> Option<String> {
    let actions = params.get("commandActions")?.as_array()?;
    if actions.is_empty() {
        return None;
    }
    let lines = actions
        .iter()
        .take(4)
        .filter_map(|action| {
            let action_type = action
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let command = action.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let detail = action
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| action.get("query").and_then(|v| v.as_str()))
                .or_else(|| action.get("name").and_then(|v| v.as_str()))
                .unwrap_or("");
            if command.is_empty() && detail.is_empty() {
                None
            } else if detail.is_empty() {
                Some(format!("- `{action_type}` `{command}`"))
            } else {
                Some(format!("- `{action_type}` `{command}` → `{detail}`"))
            }
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| format!("commandActions:\n{}", lines.join("\n")))
}

fn additional_permissions_summary(params: &Value) -> Option<String> {
    let permissions = params.get("additionalPermissions")?;
    let mut parts = Vec::new();
    if permissions
        .get("network")
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        parts.push("network".to_string());
    }
    if let Some(file_system) = permissions.get("fileSystem") {
        if let Some(read) = string_array(file_system.get("read")) {
            if !read.is_empty() {
                parts.push(format!("read {}", read.join(", ")));
            }
        }
        if let Some(write) = string_array(file_system.get("write")) {
            if !write.is_empty() {
                parts.push(format!("write {}", write.join(", ")));
            }
        }
        if let Some(entries) = file_system.get("entries").and_then(|v| v.as_array()) {
            let entry_text = entries
                .iter()
                .take(6)
                .filter_map(|entry| {
                    let access = entry
                        .get("access")
                        .and_then(|v| v.as_str())
                        .unwrap_or("access");
                    let path = entry.get("path").map(compact_json_value)?;
                    Some(format!("{access} {path}"))
                })
                .collect::<Vec<_>>();
            if !entry_text.is_empty() {
                parts.push(entry_text.join(", "));
            }
        }
    }
    (!parts.is_empty()).then(|| format!("additionalPermissions: {}", parts.join("; ")))
}

fn execpolicy_amendment_summary(params: &Value) -> Option<String> {
    let amendment = params.get("proposedExecpolicyAmendment")?;
    let prefix = decision_prefix_from_execpolicy_amendment(amendment)?;
    Some(format!("proposedExecpolicyAmendment: `{prefix}`"))
}

fn network_policy_amendments_summary(params: &Value) -> Option<String> {
    let amendments = params
        .get("proposedNetworkPolicyAmendments")
        .and_then(|v| v.as_array())?;
    let lines = amendments
        .iter()
        .take(4)
        .filter_map(|amendment| {
            let host = amendment.get("host").and_then(|v| v.as_str())?;
            let action = amendment
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("allow");
            Some(format!("- `{action}` `{host}`"))
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| format!("proposedNetworkPolicyAmendments:\n{}", lines.join("\n")))
}

fn command_approval_decisions(params: &Value) -> Vec<ApprovalDecisionOption> {
    if let Some(decisions) = params.get("availableDecisions").and_then(|v| v.as_array()) {
        let mapped = decisions
            .iter()
            .filter_map(|decision| command_decision_option(decision, params))
            .collect::<Vec<_>>();
        if !mapped.is_empty() {
            return mapped;
        }
    }

    let mut decisions = Vec::new();
    decisions.push(command_decision_option(&json!("accept"), params).unwrap());
    if params.get("networkApprovalContext").is_some() {
        decisions.push(command_decision_option(&json!("acceptForSession"), params).unwrap());
        if let Some(amendment) = params
            .get("proposedNetworkPolicyAmendments")
            .and_then(|v| v.as_array())
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("action").and_then(|v| v.as_str()) == Some("allow"))
            })
        {
            decisions.push(command_decision_option(
                &json!({ "applyNetworkPolicyAmendment": { "network_policy_amendment": amendment } }),
                params,
            )
            .unwrap());
        }
    } else if params.get("additionalPermissions").is_none() {
        if let Some(amendment) = params.get("proposedExecpolicyAmendment") {
            decisions.push(command_decision_option(
                &json!({ "acceptWithExecpolicyAmendment": { "execpolicy_amendment": amendment } }),
                params,
            )
            .unwrap());
        }
    }
    decisions.push(command_decision_option(&json!("cancel"), params).unwrap());
    decisions
}

fn file_change_approval_decisions() -> Vec<ApprovalDecisionOption> {
    vec![
        decision_option("Yes, proceed", json!("accept")),
        decision_option(
            "Yes, and don't ask again for these files",
            json!("acceptForSession"),
        ),
        decision_option("No, and tell Codex what to do differently", json!("cancel")),
    ]
}

fn command_decision_option(decision: &Value, params: &Value) -> Option<ApprovalDecisionOption> {
    let label = if let Some(decision_name) = decision.as_str() {
        match decision_name {
            "accept" => {
                if params.get("networkApprovalContext").is_some() {
                    "Yes, just this once".to_string()
                } else {
                    "Yes, proceed".to_string()
                }
            }
            "acceptForSession" => {
                if params.get("networkApprovalContext").is_some() {
                    "Yes, and allow this host for this conversation".to_string()
                } else if params.get("additionalPermissions").is_some() {
                    "Yes, and allow these permissions for this session".to_string()
                } else {
                    "Yes, and don't ask again for this command in this session".to_string()
                }
            }
            "decline" => "No, continue without running it".to_string(),
            "cancel" => "No, and tell Codex what to do differently".to_string(),
            _ => return None,
        }
    } else if let Some(amendment) = decision.get("acceptWithExecpolicyAmendment") {
        let prefix = amendment
            .get("execpolicy_amendment")
            .or_else(|| amendment.get("execpolicyAmendment"))
            .and_then(decision_prefix_from_execpolicy_amendment)
            .unwrap_or_else(|| "this command".to_string());
        if prefix.contains('\n') || prefix.contains('\r') {
            return None;
        }
        format!("Yes, and don't ask again for commands that start with `{prefix}`")
    } else if let Some(amendment) = decision.get("applyNetworkPolicyAmendment") {
        let action = amendment
            .get("network_policy_amendment")
            .or_else(|| amendment.get("networkPolicyAmendment"))
            .and_then(|v| v.get("action"))
            .and_then(|v| v.as_str())
            .unwrap_or("allow");
        if action == "deny" {
            "No, and block this host in the future".to_string()
        } else {
            "Yes, and allow this host in the future".to_string()
        }
    } else {
        return None;
    };
    Some(decision_option(&label, decision.clone()))
}

fn decision_option(label: &str, decision: Value) -> ApprovalDecisionOption {
    ApprovalDecisionOption {
        label: label.to_string(),
        decision,
    }
}

fn decision_prefix_from_execpolicy_amendment(value: &Value) -> Option<String> {
    let parts = value
        .as_array()?
        .iter()
        .filter_map(|part| part.as_str())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    if parts.len() >= 3
        && matches!(
            parts[0].rsplit(['/', '\\']).next(),
            Some("bash" | "zsh" | "sh")
        )
        && parts[1] == "-lc"
    {
        return Some(parts[2].to_string());
    }
    Some(parts.join(" "))
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    Some(
        value?
            .as_array()?
            .iter()
            .filter_map(|value| value.as_str().map(|text| format!("`{text}`")))
            .collect(),
    )
}

fn compact_json_value(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return format!("`{text}`");
    }
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn is_accept_decision(decision: &Value) -> bool {
    decision
        .as_str()
        .is_some_and(|value| value == "accept" || value == "approved")
}

fn is_negative_decision(decision: &Value) -> bool {
    decision
        .as_str()
        .is_some_and(|value| matches!(value, "decline" | "cancel" | "denied"))
        || decision
            .get("applyNetworkPolicyAmendment")
            .and_then(|value| value.get("network_policy_amendment"))
            .or_else(|| {
                decision
                    .get("applyNetworkPolicyAmendment")
                    .and_then(|value| value.get("networkPolicyAmendment"))
            })
            .and_then(|value| value.get("action"))
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "deny")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::im_runtime::PendingApproval;

    use super::{
        CodexNotification, approval_decision_by_input, approval_request_view, approval_response,
        extract_agent_message_text, extract_turn_reply_text,
    };

    fn notification(method: &str, params: serde_json::Value) -> CodexNotification {
        CodexNotification {
            method: method.to_string(),
            params: Some(params),
            request_id: Some(json!(1)),
            remote_client_key: None,
            remote_client_id: None,
            remote_stream_id: None,
        }
    }

    #[test]
    fn command_approval_uses_available_decisions_verbatim() {
        let view = approval_request_view(&notification(
            "item/commandExecution/requestApproval",
            json!({
                "threadId": "thread",
                "turnId": "turn",
                "itemId": "item",
                "command": ["npm", "test"],
                "cwd": "D:\\repo",
                "availableDecisions": [
                    "accept",
                    "acceptForSession",
                    {
                        "acceptWithExecpolicyAmendment": {
                            "execpolicy_amendment": ["npm", "test"]
                        }
                    },
                    "cancel"
                ]
            }),
        ))
        .expect("approval view");

        assert_eq!(
            view.decisions
                .iter()
                .map(|option| option.decision.clone())
                .collect::<Vec<_>>(),
            vec![
                json!("accept"),
                json!("acceptForSession"),
                json!({
                    "acceptWithExecpolicyAmendment": {
                        "execpolicy_amendment": ["npm", "test"]
                    }
                }),
                json!("cancel")
            ]
        );
    }

    #[test]
    fn v1_approval_keeps_review_decision_values() {
        let view = approval_request_view(&notification(
            "execCommandApproval",
            json!({
                "command": "cargo test",
                "cwd": "D:\\repo"
            }),
        ))
        .expect("approval view");

        assert_eq!(view.decisions[0].decision, json!("approved"));
        assert_eq!(view.decisions[1].decision, json!("denied"));
        assert_eq!(
            approval_response(view.decisions[0].decision.clone()),
            json!({ "decision": "approved" })
        );
    }

    #[test]
    fn yes_no_reply_maps_to_current_protocol_decisions() {
        let pending = PendingApproval {
            request_id: json!(7),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "summary".to_string(),
            decisions: vec![
                super::decision_option("Yes", json!("accept")),
                super::decision_option("No", json!("cancel")),
            ],
            message_id: None,
            remote_client_key: None,
        };

        let (yes_index, yes) = approval_decision_by_input(&pending, "/y").expect("yes");
        let (no_index, no) = approval_decision_by_input(&pending, "/n").expect("no");

        assert_eq!(yes_index, 1);
        assert_eq!(yes.decision, json!("accept"));
        assert_eq!(no_index, 2);
        assert_eq!(no.decision, json!("cancel"));
    }

    #[test]
    fn agent_message_text_supports_response_message_content() {
        let item = json!({
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "output_text", "text": "hello"},
                {"type": "output_text", "text": "world"}
            ]
        });

        assert_eq!(
            extract_agent_message_text(&item).as_deref(),
            Some("hello\n\nworld")
        );
    }

    #[test]
    fn turn_reply_text_supports_task_complete_last_agent_message() {
        let params = json!({
            "threadId": "thread",
            "payload": {
                "type": "task_complete",
                "last_agent_message": "done"
            }
        });

        assert_eq!(extract_turn_reply_text(&params).as_deref(), Some("done"));
    }

    #[test]
    fn turn_reply_text_supports_not_loaded_turn_with_direct_message() {
        let params = json!({
            "threadId": "thread",
            "turn": {
                "items": [],
                "itemsView": "notLoaded",
                "lastAgentMessage": "final"
            }
        });

        assert_eq!(extract_turn_reply_text(&params).as_deref(), Some("final"));
    }
}
