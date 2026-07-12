use std::collections::{HashMap, VecDeque};

use axum::body::Bytes;
use serde_json::Value;

use super::options::AnthropicProviderProfile;
use super::stream_events::unix_timestamp;
use super::stream_message::StreamMessageItem;
use super::stream_reasoning::StreamReasoningItem;
use super::stream_tools::{AnthropicContentBlockState, AnthropicWebSearchBlockState};
use crate::ai_gateway::model::generate_response_id;
use crate::ai_gateway::tool_names::ToolNameMap;

pub(super) struct AnthropicStreamState {
    pub(super) has_started: bool,
    pub(super) response_completed: bool,
    pub(super) response_id: String,
    pub(super) model: String,
    pub(super) created_at: i64,
    pub(super) sequence_number: usize,
    pub(super) output_index: usize,
    pub(super) message_item: Option<StreamMessageItem>,
    pub(super) reasoning_item: Option<StreamReasoningItem>,
    pub(super) content_blocks: HashMap<usize, AnthropicContentBlockState>,
    pub(super) web_search_blocks: HashMap<usize, AnthropicWebSearchBlockState>,
    pub(super) completed_output: Vec<Value>,
    pub(super) usage: Option<Value>,
    pub(super) stop_reason: Option<String>,
    pub(super) tool_name_map: ToolNameMap,
    pub(super) profile: AnthropicProviderProfile,
    pub(super) glm_pending_text: Option<String>,
}

impl AnthropicStreamState {
    pub(super) fn new(
        model: String,
        tool_name_map: ToolNameMap,
        profile: AnthropicProviderProfile,
    ) -> Self {
        Self {
            has_started: false,
            response_completed: false,
            response_id: generate_response_id(),
            model,
            created_at: unix_timestamp(),
            sequence_number: 0,
            output_index: 0,
            message_item: None,
            reasoning_item: None,
            content_blocks: HashMap::new(),
            web_search_blocks: HashMap::new(),
            completed_output: Vec::new(),
            usage: None,
            stop_reason: None,
            tool_name_map,
            profile,
            glm_pending_text: None,
        }
    }

    pub(super) fn process_event(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => self.handle_message_start(event, queue),
            Some("content_block_start") => self.handle_content_block_start(event, queue),
            Some("content_block_delta") => self.handle_content_block_delta(event, queue),
            Some("content_block_stop") => self.handle_content_block_stop(event, queue),
            Some("message_delta") => self.handle_message_delta(event),
            Some("message_stop") => self.handle_done(queue),
            _ => {}
        }
    }

    pub(super) fn handle_done(&mut self, queue: &mut VecDeque<Bytes>) {
        if !self.has_started {
            return;
        }
        self.close_reasoning_item(queue);
        self.flush_glm_pending_text(queue);
        self.close_message_item(queue);
        let mut indices: Vec<usize> = self.content_blocks.keys().cloned().collect();
        indices.sort_unstable();
        for index in indices {
            self.close_tool_block(index, queue);
        }
        let mut web_indices: Vec<usize> = self.web_search_blocks.keys().cloned().collect();
        web_indices.sort_unstable();
        for index in web_indices {
            self.close_web_search_block(index, queue);
        }
        if !self.response_completed {
            self.emit_response_completed(queue);
        }
    }

    fn handle_content_block_start(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        self.ensure_started(queue);
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let block = event.get("content_block").unwrap_or(&Value::Null);
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                self.close_reasoning_item(queue);
                if matches!(self.profile, AnthropicProviderProfile::GlmAnthropic) {
                    self.glm_pending_text.get_or_insert_with(String::new);
                } else {
                    self.ensure_message_item(queue);
                }
                if let Some(citations) = block.get("citations").and_then(Value::as_array) {
                    for citation in citations {
                        self.handle_citation_delta(citation, queue);
                    }
                }
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        self.handle_text_delta(text, queue);
                    }
                }
            }
            Some("tool_use") => {
                if self.is_unmapped_web_search_tool_use(block) {
                    self.start_server_tool_block(index, block, queue);
                } else {
                    self.start_tool_block(index, block, queue);
                }
            }
            Some("server_tool_use") => self.start_server_tool_block(index, block, queue),
            Some("thinking") => {
                self.close_reasoning_item(queue);
                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                    self.handle_thinking_delta(text, queue);
                }
            }
            Some("redacted_thinking") => {
                self.close_reasoning_item(queue);
                self.handle_redacted_thinking(block.get("data").and_then(Value::as_str), queue);
            }
            Some("web_search_tool_result") => self.attach_web_search_result(index, block, queue),
            Some("tool_result")
                if matches!(self.profile, AnthropicProviderProfile::GlmAnthropic) =>
            {
                self.attach_web_search_result(index, block, queue)
            }
            _ => {}
        }
    }

    fn handle_content_block_delta(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        self.ensure_started(queue);
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let delta = event.get("delta").unwrap_or(&Value::Null);
        match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                self.close_reasoning_item(queue);
                if let Some(text) = delta.get("text").and_then(Value::as_str) {
                    self.handle_text_delta(text, queue);
                }
            }
            Some("citations_delta") => {
                self.close_reasoning_item(queue);
                if let Some(citation) = delta.get("citation") {
                    self.handle_citation_delta(citation, queue);
                }
            }
            Some("thinking_delta") => {
                if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                    self.handle_thinking_delta(text, queue);
                }
            }
            Some("signature_delta") => {
                if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                    self.handle_thinking_signature(signature, queue);
                }
            }
            Some("input_json_delta") => {
                self.close_reasoning_item(queue);
                if let Some(partial_json) = delta.get("partial_json").and_then(Value::as_str) {
                    if self.web_search_blocks.contains_key(&index) {
                        self.handle_web_search_delta(index, partial_json, queue);
                    } else {
                        self.handle_tool_delta(index, partial_json, queue);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_content_block_stop(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        if self.glm_pending_text.is_some() {
            self.flush_glm_pending_text(queue);
        }
        if self.content_blocks.contains_key(&index) {
            self.close_tool_block(index, queue);
        }
        if self.web_search_blocks.contains_key(&index) {
            self.close_web_search_tool_use_block(index, queue);
        }
    }
}
