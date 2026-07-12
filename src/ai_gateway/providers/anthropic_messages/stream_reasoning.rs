use std::collections::VecDeque;

use axum::body::Bytes;
use serde_json::json;

use super::stream_events::emit_sse;
use super::stream_state::AnthropicStreamState;
use crate::ai_gateway::encrypted_content::AnthropicEncryptedContentKind;
use crate::ai_gateway::model::generate_item_id;

pub(super) struct StreamReasoningItem {
    pub(super) item_id: String,
    pub(super) output_index: usize,
    pub(super) text: String,
    pub(super) summary_part_started: bool,
    pub(super) encrypted_content: Option<String>,
    pub(super) kind: AnthropicEncryptedContentKind,
}

impl AnthropicStreamState {
    pub(super) fn handle_thinking_delta(&mut self, text: &str, queue: &mut VecDeque<Bytes>) {
        if text.is_empty() {
            return;
        }
        self.ensure_reasoning_item(queue, AnthropicEncryptedContentKind::Thinking);

        let mut added_part = None;
        let mut delta_event = None;
        let seq_for_part = self.next_seq();
        let seq_for_delta = self.next_seq();
        if let Some(item) = self.reasoning_item.as_mut() {
            if !item.summary_part_started {
                item.summary_part_started = true;
                added_part = Some(json!({
                    "type": "response.reasoning_summary_part.added",
                    "sequence_number": seq_for_part,
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": ""},
                }));
            }
            item.text.push_str(text);
            delta_event = Some(json!({
                "type": "response.reasoning_summary_text.delta",
                "sequence_number": seq_for_delta,
                "item_id": item.item_id,
                "output_index": item.output_index,
                "summary_index": 0,
                "delta": text,
            }));
        }
        if let Some(data) = added_part {
            emit_sse(queue, "response.reasoning_summary_part.added", data);
        }
        if let Some(data) = delta_event {
            emit_sse(queue, "response.reasoning_summary_text.delta", data);
        }
    }

    pub(super) fn handle_thinking_signature(
        &mut self,
        signature: &str,
        queue: &mut VecDeque<Bytes>,
    ) {
        if signature.is_empty() {
            return;
        }
        self.ensure_reasoning_item(queue, AnthropicEncryptedContentKind::Thinking);
        if let Some(item) = self.reasoning_item.as_mut() {
            item.encrypted_content = Some(signature.to_string());
        }
    }

    pub(super) fn handle_redacted_thinking(
        &mut self,
        data: Option<&str>,
        queue: &mut VecDeque<Bytes>,
    ) {
        let Some(data) = data.filter(|value| !value.is_empty()) else {
            return;
        };
        self.ensure_reasoning_item(queue, AnthropicEncryptedContentKind::RedactedThinking);
        if let Some(item) = self.reasoning_item.as_mut() {
            item.encrypted_content = Some(data.to_string());
        }
    }

    pub(super) fn close_reasoning_item(&mut self, queue: &mut VecDeque<Bytes>) {
        let Some(item) = self.reasoning_item.take() else {
            return;
        };
        if item.kind == AnthropicEncryptedContentKind::Thinking && item.summary_part_started {
            emit_sse(
                queue,
                "response.reasoning_summary_text.done",
                json!({
                    "type": "response.reasoning_summary_text.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "summary_index": 0,
                    "text": item.text,
                }),
            );
            emit_sse(
                queue,
                "response.reasoning_summary_part.done",
                json!({
                    "type": "response.reasoning_summary_part.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": item.text},
                }),
            );
        }

        let mut output_item = json!({
            "type": "reasoning",
            "id": item.item_id,
            "status": "completed",
        });
        if item.kind == AnthropicEncryptedContentKind::Thinking {
            output_item["summary"] = if item.text.is_empty() {
                json!([])
            } else {
                json!([{"type": "summary_text", "text": item.text}])
            };
        }
        if let Some(encrypted_content) = item.encrypted_content {
            output_item["encrypted_content"] = json!(encrypted_content);
        }

        emit_sse(
            queue,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": self.next_seq(),
                "output_index": item.output_index,
                "item": output_item.clone(),
            }),
        );
        self.completed_output.push(output_item);
    }

    fn ensure_reasoning_item(
        &mut self,
        queue: &mut VecDeque<Bytes>,
        kind: AnthropicEncryptedContentKind,
    ) {
        if self.reasoning_item.is_some() {
            return;
        }
        let item_id = generate_item_id();
        let output_index = self.output_index;
        self.output_index += 1;
        let mut added_item = json!({
            "type": "reasoning",
            "id": item_id,
            "status": "in_progress",
        });
        if kind == AnthropicEncryptedContentKind::Thinking {
            added_item["summary"] = json!([]);
        }
        emit_sse(
            queue,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_seq(),
                "output_index": output_index,
                "item": added_item,
            }),
        );
        self.reasoning_item = Some(StreamReasoningItem {
            item_id,
            output_index,
            text: String::new(),
            summary_part_started: false,
            encrypted_content: None,
            kind,
        });
    }
}
