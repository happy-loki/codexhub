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
    let uncached_input = usage
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
    let cache_creation = anthropic_cache_creation_input_tokens(usage);
    let input = uncached_input + cached + cache_creation;
    Some(json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": input + output,
        "input_tokens_details": {
            "cached_tokens": cached,
            "cache_creation_tokens": cache_creation,
        },
        "output_tokens_details": {"reasoning_tokens": 0},
    }))
}

fn anthropic_cache_creation_input_tokens(usage: &Value) -> i64 {
    usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .or_else(|| {
            usage.get("cache_creation").and_then(|cache_creation| {
                cache_creation
                    .as_object()
                    .map(|fields| fields.values().filter_map(Value::as_i64).sum::<i64>())
            })
        })
        .unwrap_or(0)
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
