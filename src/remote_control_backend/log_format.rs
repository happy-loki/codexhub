use std::collections::HashMap;

use serde_json::Value;

use crate::{
    app_state::{PendingRemoteRequest, RemoteControlRecentEvent, RemoteControlStreamDiagnostics},
    chain_log,
};

pub(in crate::remote_control_backend) fn format_stream_diagnostics(
    diagnostics: &RemoteControlStreamDiagnostics,
) -> String {
    format!(
        "output_delta_count={} output_delta_last_seq_id={} output_delta_last_thread={} output_delta_last_item={} output_delta_last_seen_at_ms={} output_delta_last_worker_capacity={} window_started_at_ms={} window_server_in_count={} window_output_delta_count={} window_ack_count={} window_first_seq_id={} window_last_seq_id={} max_window_started_at_ms={} max_window_last_at_ms={} max_window_server_in_count={} max_window_output_delta_count={} max_window_ack_count={} ack_count={} last_ack_seq_id={} last_ack_elapsed_ms={} max_ack_elapsed_ms={}",
        diagnostics.output_delta_count,
        diagnostics
            .output_delta_last_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_thread_id
            .as_deref()
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_item_id
            .as_deref()
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_seen_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_worker_capacity
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .window_started_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics.window_server_in_count,
        diagnostics.window_output_delta_count,
        diagnostics.window_ack_count,
        diagnostics
            .window_first_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .window_last_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .max_window_started_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .max_window_last_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics.max_window_server_in_count,
        diagnostics.max_window_output_delta_count,
        diagnostics.max_window_ack_count,
        diagnostics.ack_count,
        diagnostics
            .last_ack_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .last_ack_elapsed_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics.max_ack_elapsed_ms
    )
}

pub(in crate::remote_control_backend) fn json_preview(text: &str) -> String {
    const LIMIT: usize = 220;
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(LIMIT) {
        out.push(ch);
    }
    if compact.chars().count() > LIMIT {
        out.push_str("...");
    }
    out
}

pub(in crate::remote_control_backend) fn log_text_preview(text: &str, limit: usize) -> String {
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(limit) {
        out.push(ch);
    }
    if compact.chars().count() > limit {
        out.push_str("...");
    }
    out
}

pub(in crate::remote_control_backend) fn log_codex_to_remote_message(
    connection_epoch: u64,
    message: &Value,
) {
    if !chain_log::diagnostic_enabled() {
        return;
    }
    let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("");
    if method.is_empty() {
        return;
    }
    let params = message.get("params");
    let thread_id = params
        .and_then(thread_id_from_payload)
        .or_else(|| thread_id_from_payload(message))
        .unwrap_or_default();
    let turn_id = params
        .and_then(turn_id_from_payload)
        .or_else(|| turn_id_from_payload(message))
        .unwrap_or_default();
    let item = params.and_then(|p| p.get("item"));
    let item_id = item
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .and_then(|p| p.get("itemId"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("");
    let item_type = item
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let text = codex_message_text_for_log(method, params, item);
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=codex_to_remote connection_epoch={} method={} thread={} turn={} item={} type={} text_len={} preview={}",
            connection_epoch,
            method,
            thread_id,
            turn_id,
            item_id,
            item_type,
            text.chars().count(),
            log_text_preview(&text, 500)
        )
    });
}

pub(in crate::remote_control_backend) fn thread_id_from_payload(value: &Value) -> Option<String> {
    value
        .get("threadId")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("thread_id").and_then(|v| v.as_str()))
        .or_else(|| {
            value
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            value
                .get("turn")
                .and_then(|turn| turn.get("threadId"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string)
}

pub(in crate::remote_control_backend) fn turn_id_from_payload(value: &Value) -> Option<String> {
    value
        .get("turnId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string)
}

fn codex_message_text_for_log(
    method: &str,
    params: Option<&Value>,
    item: Option<&Value>,
) -> String {
    if let Some(delta) = params.and_then(|p| p.get("delta")).and_then(|v| v.as_str()) {
        return delta.to_string();
    }
    if let Some(message) = params
        .and_then(|p| p.get("message"))
        .and_then(|v| v.as_str())
    {
        return message.to_string();
    }
    if let Some(item) = item {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if let Some(text) = item.get("aggregatedOutput").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if method.contains("commandExecution")
            && let Some(command) = item
                .get("commandActions")
                .and_then(|v| v.as_array())
                .and_then(|actions| actions.first())
                .and_then(|action| action.get("command"))
                .and_then(|v| v.as_str())
                .or_else(|| item.get("command").and_then(|v| v.as_str()))
        {
            return command.to_string();
        }
        return item.to_string();
    }
    params.map(Value::to_string).unwrap_or_default()
}

pub(in crate::remote_control_backend) fn is_command_execution_output_delta_message(
    message: &Value,
) -> bool {
    let message = message.get("message").unwrap_or(message);
    message.get("method").and_then(|value| value.as_str())
        == Some("item/commandExecution/outputDelta")
}

pub(in crate::remote_control_backend) fn message_summary(value: &Value) -> String {
    if let Some(message) = value.get("message") {
        return message_summary(message);
    }
    if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
        let id = value.get("id").map(|v| v.to_string()).unwrap_or_default();
        let thread_id = thread_id_from_payload(value)
            .or_else(|| value.get("params").and_then(thread_id_from_payload))
            .unwrap_or_default();
        return format!("method={method} id={id} thread={thread_id}");
    }
    if let Some(id) = value.get("id") {
        if value.get("result").is_some() {
            let thread_id = value
                .get("result")
                .and_then(thread_id_from_payload)
                .unwrap_or_default();
            return format!("response id={} thread={thread_id}", id);
        }
        if let Some(error) = value.get("error") {
            return format!("error id={} body={}", id, json_preview(&error.to_string()));
        }
    }
    json_preview(&value.to_string())
}

pub(in crate::remote_control_backend) fn client_envelope_recent_kind(envelope: &Value) -> String {
    envelope
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("client_message")
        .to_string()
}

pub(in crate::remote_control_backend) fn pending_requests_summary(
    pending: &HashMap<String, PendingRemoteRequest>,
) -> String {
    if pending.is_empty() {
        return String::new();
    }
    pending
        .iter()
        .map(|(request_key, pending)| {
            format!(
                "{}:{}:thread={}:envelopes={}",
                request_key,
                pending.method,
                pending.thread_id.as_deref().unwrap_or_default(),
                pending.envelopes.len()
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(in crate::remote_control_backend) fn format_recent_event(
    event: &RemoteControlRecentEvent,
) -> String {
    format!(
        "ts_ms={} direction={} connection_epoch={} client_id={} stream_id={} seq_id={} kind={} summary={}",
        event.ts_ms,
        event.direction,
        event.connection_epoch,
        event.client_id,
        event.stream_id,
        event
            .seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        event.kind,
        event.summary
    )
}
