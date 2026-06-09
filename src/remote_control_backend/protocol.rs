use std::collections::HashMap;

use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::warn;

use crate::chain_log;

use super::message_summary;

const REMOTE_CONTROL_SEGMENT_TARGET_BYTES: usize = 100 * 1024;
const REMOTE_CONTROL_SEGMENT_MAX_BYTES: usize = 150 * 1024;
const REMOTE_CONTROL_REASSEMBLED_MAX_BYTES: usize = 100 * 1024 * 1024;
const REMOTE_CONTROL_SEGMENT_COUNT_MAX: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(in crate::remote_control_backend) enum IncomingServerEvent {
    ServerMessage {
        message: Value,
    },
    ServerMessageChunk {
        segment_id: usize,
        segment_count: usize,
        message_size_bytes: usize,
        message_chunk_base64: String,
    },
    Ack,
    Pong {
        status: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::remote_control_backend) struct IncomingServerEnvelope {
    #[serde(flatten)]
    pub(in crate::remote_control_backend) event: IncomingServerEvent,
    pub(in crate::remote_control_backend) client_id: String,
    pub(in crate::remote_control_backend) stream_id: String,
    pub(in crate::remote_control_backend) seq_id: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutgoingClientEvent {
    ClientMessage {
        message: Value,
    },
    ClientMessageChunk {
        segment_id: usize,
        segment_count: usize,
        message_size_bytes: usize,
        message_chunk_base64: String,
    },
    Ack {
        segment_id: Option<usize>,
    },
    Ping,
    #[allow(dead_code)]
    ClientClosed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct OutgoingClientEnvelope {
    #[serde(flatten)]
    event: OutgoingClientEvent,
    client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

pub(in crate::remote_control_backend) struct ServerChunkAssembly {
    segment_count: usize,
    message_size_bytes: usize,
    raw: Vec<u8>,
    next_segment_id: usize,
}

pub(in crate::remote_control_backend) enum ServerChunkObservation {
    Pending,
    Complete(Value),
    Dropped,
}

pub(in crate::remote_control_backend) fn build_client_ping_envelope(
    client_id: &str,
    stream_id: &str,
    cursor: Option<&str>,
) -> Value {
    json!(OutgoingClientEnvelope {
        event: OutgoingClientEvent::Ping,
        client_id: client_id.to_string(),
        stream_id: Some(stream_id.to_string()),
        seq_id: None,
        cursor: cursor.map(str::to_string),
    })
}

pub(in crate::remote_control_backend) fn build_client_ack_envelope(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: Option<usize>,
) -> Value {
    json!(OutgoingClientEnvelope {
        event: OutgoingClientEvent::Ack { segment_id },
        client_id: client_id.to_string(),
        stream_id: Some(stream_id.to_string()),
        seq_id: Some(seq_id),
        cursor: None,
    })
}

pub(in crate::remote_control_backend) fn observe_server_chunk(
    chunks: &mut HashMap<(String, String, u64), ServerChunkAssembly>,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    message_chunk_base64: &str,
) -> ServerChunkObservation {
    let key = (client_id.to_string(), stream_id.to_string(), seq_id);
    if chunks
        .get(&key)
        .is_some_and(|assembly| segment_id < assembly.next_segment_id)
    {
        warn!(
            "dropping duplicate remote-control server chunk: next={} got={} seq={seq_id}",
            chunks
                .get(&key)
                .map(|assembly| assembly.next_segment_id)
                .unwrap_or_default(),
            segment_id
        );
        return ServerChunkObservation::Dropped;
    }
    if segment_count == 0
        || segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX
        || segment_id >= segment_count
        || message_size_bytes == 0
        || message_size_bytes > REMOTE_CONTROL_REASSEMBLED_MAX_BYTES
        || message_chunk_base64.is_empty()
    {
        warn!(
            "invalid remote-control server chunk metadata: segment={segment_id}/{segment_count} size={message_size_bytes}"
        );
        chunks.remove(&key);
        return ServerChunkObservation::Dropped;
    }
    let assembly = chunks
        .entry(key.clone())
        .or_insert_with(|| ServerChunkAssembly {
            segment_count,
            message_size_bytes,
            raw: Vec::new(),
            next_segment_id: 0,
        });
    let expected_segment_id = assembly.next_segment_id;
    if assembly.segment_count != segment_count
        || assembly.message_size_bytes != message_size_bytes
        || expected_segment_id != segment_id
    {
        let _ = assembly;
        chunks.remove(&key);
        warn!(
            "out-of-order remote-control server chunk: expected={} got={} seq={seq_id}",
            expected_segment_id, segment_id
        );
        return ServerChunkObservation::Dropped;
    }
    let chunk = match base64::engine::general_purpose::STANDARD.decode(message_chunk_base64) {
        Ok(chunk) => chunk,
        Err(err) => {
            let _ = assembly;
            chunks.remove(&key);
            warn!("invalid remote-control server chunk base64: {err}");
            return ServerChunkObservation::Dropped;
        }
    };
    if assembly.raw.len().saturating_add(chunk.len()) > assembly.message_size_bytes {
        let _ = assembly;
        chunks.remove(&key);
        warn!("remote-control server chunk size overflow: seq={seq_id}");
        return ServerChunkObservation::Dropped;
    }
    assembly.raw.extend_from_slice(&chunk);
    assembly.next_segment_id += 1;
    if assembly.next_segment_id < assembly.segment_count {
        return ServerChunkObservation::Pending;
    }
    let Some(assembly) = chunks.remove(&key) else {
        warn!("missing completed remote-control server chunk assembly");
        return ServerChunkObservation::Dropped;
    };
    if assembly.raw.len() != assembly.message_size_bytes {
        warn!(
            "remote-control server chunk size mismatch: expected={} got={}",
            assembly.message_size_bytes,
            assembly.raw.len()
        );
        return ServerChunkObservation::Dropped;
    }
    match serde_json::from_slice::<Value>(&assembly.raw) {
        Ok(message) => ServerChunkObservation::Complete(message),
        Err(err) => {
            warn!("invalid reassembled remote-control server message: {err}");
            ServerChunkObservation::Dropped
        }
    }
}

pub(in crate::remote_control_backend) fn build_client_message_envelopes(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    message: Value,
    cursor: Option<&str>,
) -> Result<Vec<Value>> {
    let envelope =
        build_client_envelope(client_id, Some(stream_id), seq_id, message.clone(), cursor);
    if serialized_json_len(&envelope)? <= REMOTE_CONTROL_SEGMENT_MAX_BYTES {
        return Ok(vec![envelope]);
    }

    let raw = serde_json::to_vec(&message).context("failed to serialize remote-control message")?;
    let message_size_bytes = raw.len();
    if message_size_bytes > REMOTE_CONTROL_REASSEMBLED_MAX_BYTES {
        anyhow::bail!(
            "remote-control message exceeds reassembled size limit: {} bytes",
            message_size_bytes
        );
    }

    let minimal_segment_count =
        usize::min(message_size_bytes.max(1), REMOTE_CONTROL_SEGMENT_COUNT_MAX);
    let minimal_chunk = &raw[..usize::min(raw.len(), 1)];
    if serialized_client_chunk_len(
        client_id,
        stream_id,
        seq_id,
        0,
        minimal_segment_count,
        message_size_bytes,
        minimal_chunk,
        cursor,
    )? > REMOTE_CONTROL_SEGMENT_MAX_BYTES
    {
        anyhow::bail!("remote-control message cannot fit within segment size limit");
    }

    let mut segment_count = usize::max(
        2,
        message_size_bytes.div_ceil(REMOTE_CONTROL_SEGMENT_TARGET_BYTES),
    );
    loop {
        if segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX {
            anyhow::bail!(
                "remote-control segment count exceeds maximum: {}",
                segment_count
            );
        }
        let chunk_size = usize::max(1, message_size_bytes.div_ceil(segment_count));
        segment_count = message_size_bytes.div_ceil(chunk_size);
        let segments_fit = raw
            .chunks(chunk_size)
            .enumerate()
            .all(|(segment_id, chunk)| {
                serialized_client_chunk_len(
                    client_id,
                    stream_id,
                    seq_id,
                    segment_id,
                    segment_count,
                    message_size_bytes,
                    chunk,
                    cursor,
                )
                .is_ok_and(|size| size <= REMOTE_CONTROL_SEGMENT_MAX_BYTES)
            });
        if segments_fit {
            chain_log::write_line(format!(
                "[remote_control] event=client_segmented client_id={} stream_id={} seq_id={} bytes={} segment_count={} summary={}",
                client_id,
                stream_id,
                seq_id,
                message_size_bytes,
                segment_count,
                message_summary(&message)
            ));
            warn!(
                target: "codex_remote::remote_control",
                event = "remote_control_client_segmented",
                client_id,
                stream_id,
                seq_id,
                bytes = message_size_bytes,
                segment_count,
                summary = %message_summary(&message),
                "remote-control client message segmented"
            );
            return raw
                .chunks(chunk_size)
                .enumerate()
                .map(|(segment_id, chunk)| {
                    build_client_chunk_envelope(
                        client_id,
                        stream_id,
                        seq_id,
                        segment_id,
                        segment_count,
                        message_size_bytes,
                        chunk,
                        cursor,
                    )
                })
                .collect();
        }
        if chunk_size == 1 {
            anyhow::bail!("remote-control message cannot fit within segment size limit");
        }
        let next_segment_count = segment_count + 1;
        let next_chunk_size = usize::max(1, message_size_bytes.div_ceil(next_segment_count));
        segment_count = if next_chunk_size == chunk_size {
            message_size_bytes
        } else {
            next_segment_count
        };
    }
}

fn serialized_client_chunk_len(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
    cursor: Option<&str>,
) -> Result<usize> {
    serialized_json_len(&build_client_chunk_envelope(
        client_id,
        stream_id,
        seq_id,
        segment_id,
        segment_count,
        message_size_bytes,
        chunk,
        cursor,
    )?)
}

fn build_client_chunk_envelope(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
    cursor: Option<&str>,
) -> Result<Value> {
    if segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX {
        anyhow::bail!(
            "remote-control segment count exceeds maximum: {}",
            segment_count
        );
    }
    Ok(json!(OutgoingClientEnvelope {
        event: OutgoingClientEvent::ClientMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            message_chunk_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
        },
        client_id: client_id.to_string(),
        stream_id: Some(stream_id.to_string()),
        seq_id: Some(seq_id),
        cursor: cursor.map(str::to_string),
    }))
}

fn serialized_json_len(value: &Value) -> Result<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .context("failed to serialize remote-control envelope")
}

fn build_client_envelope(
    client_id: &str,
    stream_id: Option<&str>,
    seq_id: u64,
    message: Value,
    cursor: Option<&str>,
) -> Value {
    json!(OutgoingClientEnvelope {
        event: OutgoingClientEvent::ClientMessage { message },
        client_id: client_id.to_string(),
        stream_id: stream_id.map(str::to_string),
        seq_id: Some(seq_id),
        cursor: cursor.map(str::to_string),
    })
}
