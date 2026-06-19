use std::collections::VecDeque;

use axum::body::Bytes;
use serde_json::{Value, json};
pub(super) fn emit_sse(queue: &mut VecDeque<Bytes>, event_type: &str, data: Value) {
    queue.push_back(Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        event_type, data
    )));
}

pub(super) fn convert_anthropic_stream_usage(usage: &Value) -> Option<Value> {
    usage.as_object()?;
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some(json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": input + output,
        "input_tokens_details": {"cached_tokens": cached},
        "output_tokens_details": {"reasoning_tokens": 0},
    }))
}

pub(super) fn merge_i64_field(target: &mut Value, source: &Value, field: &str) {
    if let Some(value) = source.get(field).and_then(Value::as_i64) {
        if value != 0 || target.get(field).and_then(Value::as_i64).is_none() {
            target[field] = json!(value);
        }
    }
}

pub(super) fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
