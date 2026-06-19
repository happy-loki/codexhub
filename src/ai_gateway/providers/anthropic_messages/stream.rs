use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};

use axum::body::Bytes;
use futures_util::Stream;
use serde_json::Value;

use super::stream_state::AnthropicStreamState;
use crate::ai_gateway::tool_names::ToolNameMap;

pub(super) struct AnthropicSseToResponsesSse<S> {
    inner: S,
    state: AnthropicStreamState,
    line_buf: String,
    event_name: Option<String>,
    data_lines: Vec<String>,
    output_queue: VecDeque<Bytes>,
}

impl<S> AnthropicSseToResponsesSse<S> {
    pub(super) fn new(inner: S, model: String, tool_name_map: ToolNameMap) -> Self {
        Self {
            inner,
            state: AnthropicStreamState::new(model, tool_name_map),
            line_buf: String::new(),
            event_name: None,
            data_lines: Vec::new(),
            output_queue: VecDeque::new(),
        }
    }

    fn process_sse_line(&mut self, line: &str) {
        if line.is_empty() {
            self.flush_sse_event();
            return;
        }
        if line.starts_with(':') {
            return;
        }
        if let Some(event) = line.strip_prefix("event:") {
            self.event_name = Some(event.strip_prefix(' ').unwrap_or(event).to_string());
            return;
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data_lines
                .push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }

    fn flush_sse_event(&mut self) {
        if self.data_lines.is_empty() {
            self.event_name = None;
            return;
        }
        let data = self.data_lines.join("\n");
        self.data_lines.clear();
        self.event_name = None;
        if data.trim() == "[DONE]" {
            self.state.handle_done(&mut self.output_queue);
            return;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            self.state.process_event(&value, &mut self.output_queue);
        }
    }
}

impl<S, E> Stream for AnthropicSseToResponsesSse<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(bytes) = this.output_queue.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }

        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let text = String::from_utf8_lossy(&chunk);
                    this.line_buf.push_str(&text);
                    while let Some(pos) = this.line_buf.find('\n') {
                        let line = this.line_buf[..pos].trim_end_matches('\r').to_string();
                        this.line_buf = this.line_buf[pos + 1..].to_string();
                        this.process_sse_line(&line);
                    }
                    if let Some(bytes) = this.output_queue.pop_front() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))));
                }
                Poll::Ready(None) => {
                    if !this.line_buf.is_empty() {
                        let line = std::mem::take(&mut this.line_buf);
                        this.process_sse_line(line.trim_end_matches('\r'));
                    }
                    this.flush_sse_event();
                    this.state.handle_done(&mut this.output_queue);
                    if let Some(bytes) = this.output_queue.pop_front() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
