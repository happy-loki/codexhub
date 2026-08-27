//! Streaming envelope for the internal web-search path.
//!
//! The internal web-search flow may make several upstream Anthropic round-trips
//! for a single client request (answer, then search, then answer again, ...).
//! Each round is converted independently by `AnthropicSseToResponsesSse`, which
//! emits its own `response.created` / `response.in_progress` / `response.completed`
//! envelope and numbers its `output_index` / `sequence_number` from zero.
//!
//! To present the client with one coherent Responses stream that still streams
//! token-by-token, this rewriter:
//!   * emits a single `response.created` + `response.in_progress` up front,
//!   * forwards every real content event (text/reasoning deltas, tool and
//!     converter-generated web-search progress, item added/done) while renumbering `output_index`
//!     and `sequence_number` into one global sequence,
//!   * swallows each per-round envelope event, accumulating the completed
//!     output items and usage,
//!   * emits a single terminal `response.completed` (or `response.incomplete`).

use std::collections::HashMap;

use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::ai_gateway::error::GatewayError;

/// Events that carry the streaming response envelope. They are emitted once by
/// the rewriter rather than once per upstream round.
const ENVELOPE_EVENTS: &[&str] = &[
    "response.created",
    "response.in_progress",
    "response.completed",
    "response.incomplete",
    "response.failed",
];

pub(super) struct InternalSseEnvelope {
    response_id: String,
    model: String,
    created_at: i64,
    /// Next global sequence number handed to a forwarded event.
    sequence_number: usize,
    /// Next global output index. Each real output item (message, reasoning,
    /// function/web-search call) is assigned one stable index here.
    next_output_index: usize,
    /// Whether the leading created/in_progress envelope has been sent.
    started: bool,
    /// Completed output items accumulated across all rounds, in emission order.
    completed_output: Vec<Value>,
    /// Merged usage across rounds.
    usage: Option<Value>,
    /// Terminal status carried by the last round's envelope (defaults to
    /// "completed").
    status: String,
    incomplete_details: Option<Value>,
    /// Per-round mapping from the converter's local output_index to the global
    /// output_index. Cleared at the start of every round.
    index_map: HashMap<i64, i64>,
}

impl InternalSseEnvelope {
    pub(super) fn new(response_id: String, model: String, created_at: i64) -> Self {
        Self {
            response_id,
            model,
            created_at,
            sequence_number: 0,
            next_output_index: 0,
            started: false,
            completed_output: Vec::new(),
            usage: None,
            status: "completed".to_string(),
            incomplete_details: None,
            index_map: HashMap::new(),
        }
    }

    fn next_seq(&mut self) -> usize {
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
            "incomplete_details": Value::Null,
        });
        if let Some(usage) = &self.usage {
            response["usage"] = usage.clone();
        }
        if status == "incomplete" {
            if let Some(details) = &self.incomplete_details {
                response["incomplete_details"] = details.clone();
            }
        }
        response
    }

    async fn send(
        &self,
        tx: &mpsc::Sender<Result<Bytes, std::io::Error>>,
        event_type: &str,
        data: Value,
    ) -> Result<(), GatewayError> {
        tx.send(Ok(Bytes::from(format!(
            "event: {event_type}\ndata: {data}\n\n"
        ))))
        .await
        .map_err(|_| {
            GatewayError::upstream(
                axum::http::StatusCode::BAD_GATEWAY,
                "client disconnected during anthropic internal web search stream",
            )
        })
    }

    /// Emits the leading `response.created` + `response.in_progress` once.
    pub(super) async fn ensure_started(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, std::io::Error>>,
    ) -> Result<(), GatewayError> {
        if self.started {
            return Ok(());
        }
        self.started = true;
        let created_seq = self.next_seq();
        let created = self.response_object("in_progress");
        self.send(
            tx,
            "response.created",
            json!({
                "type": "response.created",
                "sequence_number": created_seq,
                "response": created,
            }),
        )
        .await?;
        let in_progress_seq = self.next_seq();
        let in_progress = self.response_object("in_progress");
        self.send(
            tx,
            "response.in_progress",
            json!({
                "type": "response.in_progress",
                "sequence_number": in_progress_seq,
                "response": in_progress,
            }),
        )
        .await
    }

    /// Resets the per-round output-index mapping. Call once before feeding a
    /// round's converted events.
    pub(super) fn begin_round(&mut self) {
        self.index_map.clear();
    }

    /// Translates a converter-local output index into the stable global index,
    /// allocating a new one on first sight within the round.
    fn global_index(&mut self, local: i64) -> i64 {
        if let Some(global) = self.index_map.get(&local) {
            return *global;
        }
        let global = self.next_output_index as i64;
        self.next_output_index += 1;
        self.index_map.insert(local, global);
        global
    }

    /// Handles one converted SSE event from a round. Envelope events are
    /// swallowed (their usage/output/status are accumulated); everything else is
    /// forwarded with renumbered `output_index` and `sequence_number`.
    pub(super) async fn forward_converted(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, std::io::Error>>,
        event_type: &str,
        mut data: Value,
    ) -> Result<(), GatewayError> {
        if ENVELOPE_EVENTS.contains(&event_type) {
            self.absorb_envelope(event_type, &data);
            return Ok(());
        }

        if let Some(local) = data.get("output_index").and_then(Value::as_i64) {
            let global = self.global_index(local);
            data["output_index"] = json!(global);
        }

        // Capture completed items so the terminal response object mirrors what
        // was streamed. `response.output_item.done` carries the finished item.
        if event_type == "response.output_item.done" {
            if let Some(item) = data.get("item") {
                self.completed_output.push(item.clone());
            }
        }

        let seq = self.next_seq();
        data["sequence_number"] = json!(seq);
        self.send(tx, event_type, data).await
    }

    fn absorb_envelope(&mut self, event_type: &str, data: &Value) {
        let Some(response) = data.get("response") else {
            return;
        };
        // Each inner round emits response.created, response.in_progress and a
        // terminal response.completed/incomplete/failed, all carrying the same
        // usage snapshot. Only absorb usage from the terminal event so a single
        // round is counted once; absorbing every envelope would multiply the
        // round's usage by the number of envelope events (e.g. 3x), inflating
        // the reported input token count and tripping premature compaction.
        if is_round_terminal_envelope(event_type) {
            if let Some(usage) = response.get("usage") {
                self.merge_usage(usage);
            }
        }
        match event_type {
            "response.incomplete" => {
                self.status = "incomplete".to_string();
                if let Some(details) = response.get("incomplete_details") {
                    if !details.is_null() {
                        self.incomplete_details = Some(details.clone());
                    }
                }
            }
            "response.failed" => {
                self.status = "failed".to_string();
            }
            _ => {}
        }
    }

    fn merge_usage(&mut self, usage: &Value) {
        let Some(existing) = self.usage.as_mut() else {
            self.usage = Some(usage.clone());
            return;
        };
        add_i64(existing, usage, "input_tokens");
        add_i64(existing, usage, "output_tokens");
        let input = existing
            .get("input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let output = existing
            .get("output_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        existing["total_tokens"] = json!(input + output);
        add_input_detail(existing, usage, "cached_tokens");
        add_input_detail(existing, usage, "cache_creation_tokens");
        add_input_detail(existing, usage, "cache_creation_5m_tokens");
        add_input_detail(existing, usage, "cache_creation_1h_tokens");
        add_output_detail(existing, usage, "reasoning_tokens");
    }

    /// Emits the single terminal `response.completed` / `response.incomplete`.
    pub(super) async fn finish(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, std::io::Error>>,
    ) -> Result<(), GatewayError> {
        self.ensure_started(tx).await?;
        let status = self.status.clone();
        let event_type = match status.as_str() {
            "incomplete" => "response.incomplete",
            "failed" => "response.failed",
            _ => "response.completed",
        };
        let response = self.response_object(&status);
        let seq = self.next_seq();
        self.send(
            tx,
            event_type,
            json!({
                "type": event_type,
                "sequence_number": seq,
                "response": response,
            }),
        )
        .await
    }
}

fn is_round_terminal_envelope(event_type: &str) -> bool {
    matches!(
        event_type,
        "response.completed" | "response.incomplete" | "response.failed"
    )
}

fn add_i64(existing: &mut Value, usage: &Value, field: &str) {
    let Some(value) = usage.get(field).and_then(Value::as_i64) else {
        return;
    };
    let current = existing.get(field).and_then(Value::as_i64).unwrap_or(0);
    existing[field] = json!(current + value);
}

fn add_input_detail(existing: &mut Value, usage: &Value, field: &str) {
    let Some(value) = usage
        .get("input_tokens_details")
        .and_then(|details| details.get(field))
        .and_then(Value::as_i64)
    else {
        return;
    };
    let current = existing
        .get("input_tokens_details")
        .and_then(|details| details.get(field))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    existing["input_tokens_details"][field] = json!(current + value);
}

fn add_output_detail(existing: &mut Value, usage: &Value, field: &str) {
    let Some(value) = usage
        .get("output_tokens_details")
        .and_then(|details| details.get(field))
        .and_then(Value::as_i64)
    else {
        return;
    };
    let current = existing
        .get("output_tokens_details")
        .and_then(|details| details.get(field))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    existing["output_tokens_details"][field] = json!(current + value);
}
