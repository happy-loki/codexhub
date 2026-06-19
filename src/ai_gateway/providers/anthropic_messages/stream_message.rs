use std::collections::VecDeque;

use axum::body::Bytes;
use serde_json::json;

use super::stream_events::emit_sse;
use super::stream_state::AnthropicStreamState;
use crate::ai_gateway::model::generate_item_id;

pub(super) struct StreamMessageItem {
    pub(super) item_id: String,
    pub(super) output_index: usize,
    pub(super) text: String,
    pub(super) content_part_started: bool,
}

impl AnthropicStreamState {
    pub(super) fn ensure_message_item(&mut self, queue: &mut VecDeque<Bytes>) {
        if self.message_item.is_some() {
            return;
        }
        let item_id = generate_item_id();
        let output_index = self.output_index;
        self.output_index += 1;
        emit_sse(
            queue,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_seq(),
                "output_index": output_index,
                "item": {
                    "type": "message",
                    "id": item_id,
                    "role": "assistant",
                    "status": "in_progress",
                    "content": [],
                }
            }),
        );
        self.message_item = Some(StreamMessageItem {
            item_id,
            output_index,
            text: String::new(),
            content_part_started: false,
        });
    }

    pub(super) fn handle_text_delta(&mut self, text: &str, queue: &mut VecDeque<Bytes>) {
        if text.is_empty() {
            return;
        }
        self.ensure_message_item(queue);

        let mut added_part = None;
        let mut delta_event = None;
        let seq_for_part = self.next_seq();
        let seq_for_delta = self.next_seq();
        if let Some(item) = self.message_item.as_mut() {
            if !item.content_part_started {
                item.content_part_started = true;
                added_part = Some(json!({
                    "type": "response.content_part.added",
                    "sequence_number": seq_for_part,
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": "", "annotations": []},
                }));
            }
            item.text.push_str(text);
            delta_event = Some(json!({
                "type": "response.output_text.delta",
                "sequence_number": seq_for_delta,
                "item_id": item.item_id,
                "output_index": item.output_index,
                "content_index": 0,
                "delta": text,
                "logprobs": [],
            }));
        }
        if let Some(data) = added_part {
            emit_sse(queue, "response.content_part.added", data);
        }
        if let Some(data) = delta_event {
            emit_sse(queue, "response.output_text.delta", data);
        }
    }

    pub(super) fn close_message_item(&mut self, queue: &mut VecDeque<Bytes>) {
        let Some(item) = self.message_item.take() else {
            return;
        };
        if item.content_part_started {
            emit_sse(
                queue,
                "response.output_text.done",
                json!({
                    "type": "response.output_text.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "content_index": 0,
                    "text": item.text,
                    "logprobs": [],
                }),
            );
            emit_sse(
                queue,
                "response.content_part.done",
                json!({
                    "type": "response.content_part.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": item.text, "annotations": []},
                }),
            );
        }
        let output_item = json!({
            "type": "message",
            "id": item.item_id,
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": item.text, "annotations": []}],
        });
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
}
