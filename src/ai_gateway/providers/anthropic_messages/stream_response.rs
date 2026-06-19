use std::collections::VecDeque;

use axum::body::Bytes;
use serde_json::{Value, json};

use super::stream_events::{convert_anthropic_stream_usage, emit_sse, merge_i64_field};
use super::stream_state::AnthropicStreamState;

impl AnthropicStreamState {
    pub(super) fn handle_message_start(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        if self.has_started {
            return;
        }
        self.has_started = true;
        let message = event.get("message").unwrap_or(event);
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            self.response_id = id.to_string();
        }
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        if let Some(usage) = message
            .get("usage")
            .and_then(convert_anthropic_stream_usage)
        {
            self.merge_usage(usage);
        }
        self.emit_response_created(queue);
        self.emit_response_in_progress(queue);
    }

    pub(super) fn handle_message_delta(&mut self, event: &Value) {
        if let Some(delta) = event.get("delta") {
            if let Some(stop_reason) = delta.get("stop_reason").and_then(Value::as_str) {
                self.stop_reason = Some(stop_reason.to_string());
            }
        }
        if let Some(usage) = event.get("usage").and_then(convert_anthropic_stream_usage) {
            self.merge_usage(usage);
        }
    }

    pub(super) fn ensure_started(&mut self, queue: &mut VecDeque<Bytes>) {
        if self.has_started {
            return;
        }
        self.has_started = true;
        self.emit_response_created(queue);
        self.emit_response_in_progress(queue);
    }

    pub(super) fn emit_response_created(&mut self, queue: &mut VecDeque<Bytes>) {
        emit_sse(
            queue,
            "response.created",
            json!({
                "type": "response.created",
                "sequence_number": self.next_seq(),
                "response": self.response_object("in_progress"),
            }),
        );
    }

    pub(super) fn emit_response_in_progress(&mut self, queue: &mut VecDeque<Bytes>) {
        emit_sse(
            queue,
            "response.in_progress",
            json!({
                "type": "response.in_progress",
                "sequence_number": self.next_seq(),
                "response": self.response_object("in_progress"),
            }),
        );
    }

    pub(super) fn emit_response_completed(&mut self, queue: &mut VecDeque<Bytes>) {
        let status = match self.stop_reason.as_deref() {
            Some("max_tokens") => "incomplete",
            _ => "completed",
        };
        let event_type = if status == "incomplete" {
            "response.incomplete"
        } else {
            "response.completed"
        };
        emit_sse(
            queue,
            event_type,
            json!({
                "type": event_type,
                "sequence_number": self.next_seq(),
                "response": self.response_object(status),
            }),
        );
        self.response_completed = true;
    }

    pub(super) fn next_seq(&mut self) -> usize {
        let seq = self.sequence_number;
        self.sequence_number += 1;
        seq
    }

    fn response_object(&self, status: &str) -> Value {
        let mut response = json!({
            "id": self.response_id,
            "object": "response",
            "model": self.model,
            "created_at": self.created_at,
            "status": status,
            "output": self.completed_output,
        });
        if let Some(usage) = &self.usage {
            response["usage"] = usage.clone();
        }
        response
    }

    fn merge_usage(&mut self, usage: Value) {
        let Some(existing) = self.usage.as_mut() else {
            self.usage = Some(usage);
            return;
        };
        merge_i64_field(existing, &usage, "input_tokens");
        merge_i64_field(existing, &usage, "output_tokens");
        let input = existing
            .get("input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let output = existing
            .get("output_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        existing["total_tokens"] = json!(input + output);

        if let Some(cached) = usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_i64)
        {
            existing["input_tokens_details"]["cached_tokens"] = json!(cached);
        }
    }
}
