use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axum::body::Bytes;
use futures_util::Stream;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::config::AppConfig;

const DB_FILE_NAME: &str = "ai-gateway-request-logs.sqlite";

#[derive(Debug, Clone, Default)]
pub struct LogUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub read_cache_tokens: Option<i64>,
    pub read_cache_hit_rate: Option<f64>,
    pub write_cache_tokens: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RequestLogRecord {
    pub request_id: String,
    pub model_id: String,
    pub stream: bool,
    pub channel: String,
    pub provider_type: String,
    pub status: String,
    pub usage: LogUsage,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub created_at_ms: i64,
    pub error_message: Option<String>,
    pub request_json: Option<String>,
    pub upstream_request_json: Option<String>,
    pub response_json: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RequestLogUpdate {
    pub status: Option<String>,
    pub usage: Option<LogUsage>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub error_message: Option<String>,
    pub upstream_request_json: Option<String>,
    pub response_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogEntry {
    pub id: i64,
    pub request_id: String,
    pub model_id: String,
    pub stream: bool,
    pub channel: String,
    pub provider_type: String,
    pub status: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub read_cache_tokens: Option<i64>,
    pub read_cache_hit_rate: Option<f64>,
    pub write_cache_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub created_at_ms: i64,
    pub created_at: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogDetail {
    #[serde(flatten)]
    pub summary: RequestLogEntry,
    pub request_json: Option<String>,
    pub upstream_request_json: Option<String>,
    pub response_json: Option<String>,
}

#[derive(Clone)]
pub struct RequestLogContext {
    pub db_path: PathBuf,
    pub log_id: i64,
    pub started_at: Instant,
}

pub fn database_path(config: &AppConfig) -> PathBuf {
    config
        .state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DB_FILE_NAME)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn elapsed_ms(started_at: Instant) -> i64 {
    started_at.elapsed().as_millis() as i64
}

pub fn insert_record(db_path: &Path, record: &RequestLogRecord) -> rusqlite::Result<i64> {
    let conn = open(db_path)?;
    conn.execute(
        "INSERT INTO ai_gateway_request_logs (
            request_id, model_id, stream, channel, provider_type, status,
            input_tokens, output_tokens, total_tokens, read_cache_tokens,
            read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
            ttft_ms, created_at_ms, error_message, request_json, upstream_request_json,
            response_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            &record.request_id,
            &record.model_id,
            record.stream as i64,
            &record.channel,
            &record.provider_type,
            &record.status,
            record.usage.input_tokens,
            record.usage.output_tokens,
            record.usage.total_tokens,
            record.usage.read_cache_tokens,
            record.usage.read_cache_hit_rate,
            record.usage.write_cache_tokens,
            record.cost_usd,
            record.latency_ms,
            record.ttft_ms,
            record.created_at_ms,
            &record.error_message,
            &record.request_json,
            &record.upstream_request_json,
            &record.response_json,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_record(db_path: &Path, id: i64, update: &RequestLogUpdate) -> rusqlite::Result<()> {
    let conn = open(db_path)?;
    let existing = conn
        .query_row(
            "SELECT
                status, input_tokens, output_tokens, total_tokens, read_cache_tokens,
                read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
                ttft_ms, error_message, upstream_request_json, response_json
             FROM ai_gateway_request_logs WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<f64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            },
        )
        .optional()?;

    let Some(existing) = existing else {
        return Ok(());
    };
    let usage = update.usage.clone().unwrap_or(LogUsage {
        input_tokens: existing.1,
        output_tokens: existing.2,
        total_tokens: existing.3,
        read_cache_tokens: existing.4,
        read_cache_hit_rate: existing.5,
        write_cache_tokens: existing.6,
    });

    conn.execute(
        "UPDATE ai_gateway_request_logs SET
            status = ?1,
            input_tokens = ?2,
            output_tokens = ?3,
            total_tokens = ?4,
            read_cache_tokens = ?5,
            read_cache_hit_rate = ?6,
            write_cache_tokens = ?7,
            cost_usd = ?8,
            latency_ms = ?9,
            ttft_ms = ?10,
            error_message = ?11,
            upstream_request_json = ?12,
            response_json = ?13
         WHERE id = ?14",
        params![
            update.status.as_deref().unwrap_or(&existing.0),
            usage.input_tokens,
            usage.output_tokens,
            usage.total_tokens,
            usage.read_cache_tokens,
            usage.read_cache_hit_rate,
            usage.write_cache_tokens,
            update.cost_usd.or(existing.7),
            update.latency_ms.or(existing.8),
            update.ttft_ms.or(existing.9),
            update.error_message.clone().or(existing.10),
            update.upstream_request_json.clone().or(existing.11),
            update.response_json.clone().or(existing.12),
            id,
        ],
    )?;
    Ok(())
}

pub fn list_recent(db_path: &Path, limit: usize) -> rusqlite::Result<Vec<RequestLogEntry>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT
            id, request_id, model_id, stream, channel, provider_type, status,
            input_tokens, output_tokens, total_tokens, read_cache_tokens,
            read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
            ttft_ms, created_at_ms,
            datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at,
            error_message
         FROM ai_gateway_request_logs
         ORDER BY created_at_ms DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit.min(500) as i64], |row| {
        Ok(RequestLogEntry {
            id: row.get(0)?,
            request_id: row.get(1)?,
            model_id: row.get(2)?,
            stream: row.get::<_, i64>(3)? != 0,
            channel: row.get(4)?,
            provider_type: row.get(5)?,
            status: row.get(6)?,
            input_tokens: row.get(7)?,
            output_tokens: row.get(8)?,
            total_tokens: row.get(9)?,
            read_cache_tokens: row.get(10)?,
            read_cache_hit_rate: row.get(11)?,
            write_cache_tokens: row.get(12)?,
            cost_usd: row.get(13)?,
            latency_ms: row.get(14)?,
            ttft_ms: row.get(15)?,
            created_at_ms: row.get(16)?,
            created_at: row.get(17)?,
            error_message: row.get(18)?,
        })
    })?;

    let mut logs = Vec::new();
    for row in rows {
        logs.push(row?);
    }
    Ok(logs)
}

pub fn delete_older_than(db_path: &Path, cutoff_ms: i64) -> rusqlite::Result<usize> {
    let conn = open(db_path)?;
    conn.execute(
        "DELETE FROM ai_gateway_request_logs WHERE created_at_ms < ?1",
        params![cutoff_ms],
    )
}

pub fn delete_all(db_path: &Path) -> rusqlite::Result<usize> {
    let conn = open(db_path)?;
    conn.execute("DELETE FROM ai_gateway_request_logs", [])
}

pub fn get_detail(db_path: &Path, id: i64) -> rusqlite::Result<Option<RequestLogDetail>> {
    let conn = open(db_path)?;
    conn.query_row(
        "SELECT
            id, request_id, model_id, stream, channel, provider_type, status,
            input_tokens, output_tokens, total_tokens, read_cache_tokens,
            read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
            ttft_ms, created_at_ms,
            datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at,
            error_message, request_json, upstream_request_json, response_json
         FROM ai_gateway_request_logs
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(RequestLogDetail {
                summary: RequestLogEntry {
                    id: row.get(0)?,
                    request_id: row.get(1)?,
                    model_id: row.get(2)?,
                    stream: row.get::<_, i64>(3)? != 0,
                    channel: row.get(4)?,
                    provider_type: row.get(5)?,
                    status: row.get(6)?,
                    input_tokens: row.get(7)?,
                    output_tokens: row.get(8)?,
                    total_tokens: row.get(9)?,
                    read_cache_tokens: row.get(10)?,
                    read_cache_hit_rate: row.get(11)?,
                    write_cache_tokens: row.get(12)?,
                    cost_usd: row.get(13)?,
                    latency_ms: row.get(14)?,
                    ttft_ms: row.get(15)?,
                    created_at_ms: row.get(16)?,
                    created_at: row.get(17)?,
                    error_message: row.get(18)?,
                },
                request_json: row.get(19)?,
                upstream_request_json: row.get(20)?,
                response_json: row.get(21)?,
            })
        },
    )
    .optional()
}

pub fn usage_from_response_value(response: &Value) -> LogUsage {
    let Some(usage) = response.get("usage") else {
        return LogUsage::default();
    };

    let input_tokens = first_i64(usage, &["input_tokens", "prompt_tokens"]);
    let output_tokens = first_i64(usage, &["output_tokens", "completion_tokens"]);
    let total_tokens =
        first_i64(usage, &["total_tokens"]).or_else(|| match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        });
    let read_cache_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| first_i64(details, &["cached_tokens", "cache_read_input_tokens"]))
        .or_else(|| {
            usage.get("prompt_tokens_details").and_then(|details| {
                first_i64(details, &["cached_tokens", "cache_read_input_tokens"])
            })
        })
        .or_else(|| {
            first_i64(
                usage,
                &[
                    "cache_read_input_tokens",
                    "cached_tokens",
                    "prompt_cache_hit_tokens",
                ],
            )
        });
    let write_cache_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| {
            first_i64(
                details,
                &[
                    "cache_creation_tokens",
                    "cache_write_input_tokens",
                    "write_cached_tokens",
                ],
            )
        })
        .or_else(|| {
            usage.get("prompt_tokens_details").and_then(|details| {
                first_i64(
                    details,
                    &[
                        "cache_creation_tokens",
                        "cache_write_input_tokens",
                        "write_cached_tokens",
                    ],
                )
            })
        })
        .or_else(|| {
            first_i64(
                usage,
                &[
                    "cache_creation_input_tokens",
                    "cache_write_input_tokens",
                    "write_cached_tokens",
                ],
            )
        });
    let read_cache_hit_rate = match (read_cache_tokens, input_tokens) {
        (Some(cached), Some(input)) if input > 0 => Some(cached as f64 / input as f64),
        _ => None,
    };

    LogUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        read_cache_tokens,
        read_cache_hit_rate,
        write_cache_tokens,
    }
}

pub fn status_from_response_value(response: &Value) -> String {
    response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_string()
}

pub fn log_insert_error(err: rusqlite::Error) {
    warn!(error = %err, "failed to write AI Gateway request log");
}

pub fn log_update_error(err: rusqlite::Error) {
    warn!(error = %err, "failed to update AI Gateway request log");
}

pub struct ResponsesSseLogStream<S> {
    inner: S,
    context: RequestLogContext,
    line_buf: String,
    completed: bool,
    ttft_recorded: bool,
    output_queue: VecDeque<Result<Bytes, std::io::Error>>,
}

impl<S> ResponsesSseLogStream<S> {
    pub fn new(inner: S, context: RequestLogContext) -> Self {
        Self {
            inner,
            context,
            line_buf: String::new(),
            completed: false,
            ttft_recorded: false,
            output_queue: VecDeque::new(),
        }
    }
}

impl<S> Stream for ResponsesSseLogStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(item) = this.output_queue.pop_front() {
            return Poll::Ready(Some(item));
        }

        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                observe_sse_chunk(
                    &chunk,
                    &mut this.line_buf,
                    &this.context,
                    &mut this.completed,
                    &mut this.ttft_recorded,
                );
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                if !this.completed {
                    let update = RequestLogUpdate {
                        status: Some("failed".to_string()),
                        latency_ms: Some(elapsed_ms(this.context.started_at)),
                        error_message: Some(err.to_string()),
                        ..RequestLogUpdate::default()
                    };
                    if let Err(update_err) =
                        update_record(&this.context.db_path, this.context.log_id, &update)
                    {
                        log_update_error(update_err);
                    }
                }
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                if !this.completed {
                    let update = RequestLogUpdate {
                        status: Some("completed".to_string()),
                        latency_ms: Some(elapsed_ms(this.context.started_at)),
                        ..RequestLogUpdate::default()
                    };
                    if let Err(update_err) =
                        update_record(&this.context.db_path, this.context.log_id, &update)
                    {
                        log_update_error(update_err);
                    }
                    this.completed = true;
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

fn open(db_path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = db_path.parent().filter(|path| !path.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(1000))?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS ai_gateway_request_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            request_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            stream INTEGER NOT NULL,
            channel TEXT NOT NULL,
            provider_type TEXT NOT NULL,
            status TEXT NOT NULL,
            input_tokens INTEGER,
            output_tokens INTEGER,
            total_tokens INTEGER,
            read_cache_tokens INTEGER,
            read_cache_hit_rate REAL,
            write_cache_tokens INTEGER,
            cost_usd REAL,
            latency_ms INTEGER,
            ttft_ms INTEGER,
            created_at_ms INTEGER NOT NULL,
            error_message TEXT,
            request_json TEXT,
            upstream_request_json TEXT,
            response_json TEXT
        );
        "#,
    )?;

    match conn.execute(
        "ALTER TABLE ai_gateway_request_logs ADD COLUMN upstream_request_json TEXT",
        [],
    ) {
        Ok(_) => {}
        Err(err) if is_duplicate_column_error(&err) => {}
        Err(err) => return Err(err),
    }

    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_created
            ON ai_gateway_request_logs(created_at_ms DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_model
            ON ai_gateway_request_logs(model_id);
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_channel
            ON ai_gateway_request_logs(channel);
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_status
            ON ai_gateway_request_logs(status);
        "#,
    )
}

fn is_duplicate_column_error(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("duplicate column name"))
}

fn observe_sse_chunk(
    chunk: &Bytes,
    line_buf: &mut String,
    context: &RequestLogContext,
    completed: &mut bool,
    ttft_recorded: &mut bool,
) {
    let text = String::from_utf8_lossy(chunk);
    line_buf.push_str(&text);
    while let Some(pos) = line_buf.find('\n') {
        let line = line_buf[..pos].trim_end_matches('\r').to_string();
        *line_buf = line_buf[pos + 1..].to_string();
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data.trim() == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        if is_first_token_event(event_type) && !*ttft_recorded {
            let update = RequestLogUpdate {
                ttft_ms: Some(elapsed_ms(context.started_at)),
                ..RequestLogUpdate::default()
            };
            if let Err(err) = update_record(&context.db_path, context.log_id, &update) {
                log_update_error(err);
            }
            *ttft_recorded = true;
        }
        if !matches!(
            event_type,
            "response.completed" | "response.incomplete" | "response.failed"
        ) {
            continue;
        }
        let response = event.get("response").unwrap_or(&event);
        let status = status_from_response_value(response);
        let usage = usage_from_response_value(response);
        let error_message = response
            .get("error")
            .and_then(|error| error.get("message").or(Some(error)))
            .and_then(Value::as_str)
            .map(str::to_string);
        let update = RequestLogUpdate {
            status: Some(status),
            usage: Some(usage),
            latency_ms: Some(elapsed_ms(context.started_at)),
            error_message,
            response_json: Some(compact_json(response)),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = update_record(&context.db_path, context.log_id, &update) {
            log_update_error(err);
        }
        *completed = true;
    }
}

fn is_first_token_event(event_type: &str) -> bool {
    event_type.starts_with("response.") && event_type.ends_with(".delta")
}

fn first_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_from_responses_value_extracts_cache() {
        let value = json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "total_tokens": 120,
                "input_tokens_details": {
                    "cached_tokens": 80,
                    "cache_creation_tokens": 5
                }
            }
        });

        let usage = usage_from_response_value(&value);
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(120));
        assert_eq!(usage.read_cache_tokens, Some(80));
        assert_eq!(usage.write_cache_tokens, Some(5));
        assert_eq!(usage.read_cache_hit_rate, Some(0.8));
    }

    #[test]
    fn usage_from_chat_value_extracts_deepseek_cache() {
        let value = json!({
            "usage": {
                "prompt_tokens": 16,
                "completion_tokens": 645,
                "total_tokens": 661,
                "prompt_cache_hit_tokens": 4
            }
        });

        let usage = usage_from_response_value(&value);
        assert_eq!(usage.input_tokens, Some(16));
        assert_eq!(usage.output_tokens, Some(645));
        assert_eq!(usage.total_tokens, Some(661));
        assert_eq!(usage.read_cache_tokens, Some(4));
        assert_eq!(usage.read_cache_hit_rate, Some(0.25));
    }

    #[test]
    fn sqlite_insert_update_and_list_roundtrip() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let record = RequestLogRecord {
            request_id: "req-1".to_string(),
            model_id: "deepseek-v4-flash".to_string(),
            stream: true,
            channel: "deepseek".to_string(),
            provider_type: "chat_completions".to_string(),
            status: "running".to_string(),
            usage: LogUsage::default(),
            cost_usd: None,
            latency_ms: None,
            ttft_ms: None,
            created_at_ms: now_ms(),
            error_message: None,
            request_json: Some(r#"{"model":"deepseek-v4-flash"}"#.to_string()),
            upstream_request_json: Some(
                r#"{"model":"deepseek-v4-flash","messages":[]}"#.to_string(),
            ),
            response_json: None,
        };

        let id = insert_record(&db_path, &record).unwrap();
        update_record(
            &db_path,
            id,
            &RequestLogUpdate {
                status: Some("completed".to_string()),
                usage: Some(LogUsage {
                    input_tokens: Some(10),
                    output_tokens: Some(3),
                    total_tokens: Some(13),
                    read_cache_tokens: Some(8),
                    read_cache_hit_rate: Some(0.8),
                    write_cache_tokens: None,
                }),
                latency_ms: Some(1234),
                response_json: Some(r#"{"status":"completed"}"#.to_string()),
                ..RequestLogUpdate::default()
            },
        )
        .unwrap();

        let logs = list_recent(&db_path, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, id);
        assert_eq!(logs[0].status, "completed");
        assert_eq!(logs[0].total_tokens, Some(13));
        assert_eq!(logs[0].read_cache_tokens, Some(8));
        assert_eq!(logs[0].latency_ms, Some(1234));
        let detail = get_detail(&db_path, id).unwrap().unwrap();
        assert_eq!(detail.summary.id, id);
        assert_eq!(
            detail.upstream_request_json.as_deref(),
            Some(r#"{"model":"deepseek-v4-flash","messages":[]}"#)
        );
        assert_eq!(
            detail.response_json.as_deref(),
            Some(r#"{"status":"completed"}"#)
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_delete_older_than_keeps_recent_logs() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let now = now_ms();
        for (request_id, created_at_ms) in [("old", now - 5_000), ("recent", now)] {
            let record = RequestLogRecord {
                request_id: request_id.to_string(),
                model_id: "deepseek-v4-flash".to_string(),
                stream: true,
                channel: "deepseek".to_string(),
                provider_type: "chat_completions".to_string(),
                status: "completed".to_string(),
                usage: LogUsage::default(),
                cost_usd: None,
                latency_ms: None,
                ttft_ms: None,
                created_at_ms,
                error_message: None,
                request_json: None,
                upstream_request_json: None,
                response_json: None,
            };
            insert_record(&db_path, &record).unwrap();
        }

        let deleted = delete_older_than(&db_path, now - 1_000).unwrap();
        assert_eq!(deleted, 1);
        let logs = list_recent(&db_path, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].request_id, "recent");
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_delete_all_removes_every_log() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        for request_id in ["req-1", "req-2"] {
            let record = RequestLogRecord {
                request_id: request_id.to_string(),
                model_id: "deepseek-v4-flash".to_string(),
                stream: true,
                channel: "deepseek".to_string(),
                provider_type: "chat_completions".to_string(),
                status: "completed".to_string(),
                usage: LogUsage::default(),
                cost_usd: None,
                latency_ms: None,
                ttft_ms: None,
                created_at_ms: now_ms(),
                error_message: None,
                request_json: None,
                upstream_request_json: None,
                response_json: None,
            };
            insert_record(&db_path, &record).unwrap();
        }

        let deleted = delete_all(&db_path).unwrap();
        assert_eq!(deleted, 2);
        assert!(list_recent(&db_path, 10).unwrap().is_empty());
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn observe_sse_chunk_records_ttft_and_total_latency_separately() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let record = RequestLogRecord {
            request_id: "req-ttft".to_string(),
            model_id: "deepseek-v4-flash".to_string(),
            stream: true,
            channel: "deepseek".to_string(),
            provider_type: "chat_completions".to_string(),
            status: "running".to_string(),
            usage: LogUsage::default(),
            cost_usd: None,
            latency_ms: None,
            ttft_ms: None,
            created_at_ms: now_ms(),
            error_message: None,
            request_json: None,
            upstream_request_json: None,
            response_json: None,
        };

        let log_id = insert_record(&db_path, &record).unwrap();
        let context = RequestLogContext {
            db_path: db_path.clone(),
            log_id,
            started_at: Instant::now(),
        };
        let mut line_buf = String::new();
        let mut completed = false;
        let mut ttft_recorded = false;

        observe_sse_chunk(
            &Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            ),
            &mut line_buf,
            &context,
            &mut completed,
            &mut ttft_recorded,
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
        observe_sse_chunk(
            &Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
            ),
            &mut line_buf,
            &context,
            &mut completed,
            &mut ttft_recorded,
        );

        let logs = list_recent(&db_path, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].ttft_ms.is_some());
        assert!(logs[0].latency_ms.is_some());
        assert!(logs[0].latency_ms.unwrap() >= logs[0].ttft_ms.unwrap());
        assert!(completed);
        assert!(ttft_recorded);
        let _ = std::fs::remove_file(db_path);
    }
}
