use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Write as _,
    sync::Arc,
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::{
    chain_log,
    im::feishu::FeishuStreamingCardState,
    types::{ImPlatformKind, InboundAttachment},
};

const PENDING_ATTACHMENTS_MAX_AGE_MS: u128 = 10 * 60 * 1000;
const PENDING_ATTACHMENTS_MAX_COUNT: usize = 8;
const TELEGRAM_COMMAND_PROGRESS_MAX_ENTRIES: usize = 128;
const TELEGRAM_COLLAB_PROGRESS_MAX_AGENTS: usize = 64;
const TERMINAL_NOTICE_TURN_HISTORY_MAX: usize = 256;
const TELEGRAM_REASONING_MAX_CHARS: usize = 12_000;
const TELEGRAM_PLAN_MAX_STEPS: usize = 64;
const TELEGRAM_DIFF_MAX_PATHS: usize = 128;

#[derive(Debug, Clone)]
struct PendingAttachments {
    attachments: Vec<InboundAttachment>,
    received_at_ms: u128,
}

#[derive(Debug, Clone)]
pub struct RouteTarget {
    pub platform: ImPlatformKind,
    pub conversation_key: String,
    pub account_id: String,
    pub chat_id: String,
    pub remote_client_key: String,
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub request_id: Value,
    pub request_kind: String,
    #[allow(dead_code)]
    pub method: String,
    #[allow(dead_code)]
    pub params: Value,
    pub summary: String,
    pub decisions: Vec<ApprovalDecisionOption>,
    pub message_id: Option<String>,
    pub remote_client_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalDecisionOption {
    pub label: String,
    pub decision: Value,
}

#[derive(Debug, Clone)]
pub struct ResolvedApproval {
    pub conversation_key: String,
    #[allow(dead_code)]
    pub approval: PendingApproval,
    pub was_current: bool,
    pub next_current: Option<PendingApproval>,
}

#[derive(Debug, Clone)]
pub struct ThreadRoutingRequestState {
    pub request_id: String,
    pub conversation_key: String,
    #[allow(dead_code)]
    pub account_id: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub stage: ThreadRoutingStage,
    pub page: usize,
    pub page_cursors: Vec<Option<String>>,
    pub thread_ids_by_page: Vec<Vec<String>>,
    pub create_draft: ThreadCreateDraftState,
    pub create_option_values_by_field_page: HashMap<String, Vec<Vec<String>>>,
    #[allow(dead_code)]
    pub history_cursor: Option<String>,
    pub history_has_next: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRoutingStage {
    Choice,
    ResumeList,
    CreateSettings,
    CreateOptions,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadCreateDraftState {
    pub cwd_choice: Option<String>,
    pub cwd_custom: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOrigin {
    Feishu,
    Telegram,
    Wechat,
    Wecom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadTurnState {
    Starting,
    Running(String),
}

#[derive(Debug, Default)]
pub struct RuntimeState {
    pub bridge_generation: u64,
    pub current_turn_by_thread: HashMap<String, String>,
    pub starting_turn_by_thread: HashSet<String>,
    pub turn_started_at_by_thread: HashMap<String, u128>,
    pub turn_finished_at_by_thread: HashMap<String, u128>,
    pub turn_origin_by_id: HashMap<String, TurnOrigin>,
    terminal_notice_failed_by_turn: HashMap<String, bool>,
    terminal_notice_turn_order: VecDeque<String>,
    terminal_status_fallback_by_thread: HashMap<String, TerminalStatusFallbackState>,
    next_terminal_status_fallback_token: u64,
    pub last_sent_text_by_route: HashMap<String, String>,
    pub route_by_thread: HashMap<String, RouteTarget>,
    pub last_route: Option<RouteTarget>,
    pub pending_approvals_by_conversation: HashMap<String, Vec<PendingApproval>>,
    pub pending_approval_request_keys: HashSet<String>,
    pub feishu_streaming_cards_by_item: HashMap<String, FeishuStreamingCardState>,
    telegram_drafts_by_item: HashMap<String, TelegramDraftState>,
    telegram_command_progress_by_thread: HashMap<String, TelegramCommandProgressState>,
    telegram_collab_progress_by_thread: HashMap<String, TelegramCollabProgressState>,
    next_telegram_draft_id: i64,
    pub wecom_streams_by_thread: HashMap<String, WecomStreamState>,
    pub thread_routing_requests: HashMap<String, ThreadRoutingRequestState>,
    pending_attachments_by_conversation: HashMap<String, PendingAttachments>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalStatusFallbackState {
    turn_id: String,
    token: u64,
    event_driver_started: bool,
    server_driver_started: bool,
}

#[derive(Debug, Clone)]
struct TelegramDraftState {
    draft_id: i64,
    content: String,
    sending: bool,
    dirty: bool,
    finished: bool,
    revision: u64,
    last_attempt_at_ms: u128,
    wake_driver: Arc<Notify>,
    completed: Arc<Notify>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramCommandProgressStatus {
    Running,
    Interrupted,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCommandProgressEntry {
    pub item_id: String,
    pub command: String,
    pub status: TelegramCommandProgressStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<u64>,
    pub failure_output: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TelegramCommandProgressSnapshot {
    pub turn_id: String,
    pub revision: u64,
    pub message_id: Option<String>,
    pub entries: Vec<TelegramCommandProgressEntry>,
    pub dropped_entries: usize,
    pub retry_count: usize,
    pub retry_error: Option<String>,
    pub reasoning_summary: Option<String>,
    pub plan_explanation: Option<String>,
    pub plan: Vec<TelegramPlanStep>,
    pub diff_summary: Option<TelegramDiffSummary>,
    pub completed: bool,
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramPlanStep {
    pub step: String,
    pub status: TelegramPlanStepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramPlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramDiffSummary {
    pub file_count: usize,
    pub additions: usize,
    pub deletions: usize,
    pub paths: Vec<String>,
    pub omitted_paths: usize,
}

#[derive(Debug, Clone)]
struct TelegramCommandProgressState {
    turn_id: String,
    revision: u64,
    message_id: Option<String>,
    entries: Vec<TelegramCommandProgressEntry>,
    dropped_entries: usize,
    retry_count: usize,
    retry_error: Option<String>,
    reasoning_item_id: Option<String>,
    reasoning_summary_index: Option<i64>,
    reasoning_summary: String,
    reasoning_completed: bool,
    plan_explanation: Option<String>,
    plan: Vec<TelegramPlanStep>,
    diff_summary: Option<TelegramDiffSummary>,
    completed: bool,
    failed: bool,
    dirty: bool,
    sending: bool,
    in_flight_revision: Option<u64>,
    cleanup_after_delivery: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramCollabProgressStatus {
    Running,
    Responded,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCollabProgressUpdate {
    pub agent_id: String,
    pub name: Option<String>,
    pub status: Option<TelegramCollabProgressStatus>,
    pub detail: Option<String>,
    pub occurred_at_ms: u128,
    pub create_if_missing: bool,
    pub restart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCollabProgressEntry {
    pub agent_id: String,
    pub name: String,
    pub status: TelegramCollabProgressStatus,
    pub detail: Option<String>,
    pub started_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone)]
pub(crate) struct TelegramCollabProgressSnapshot {
    pub turn_id: String,
    pub revision: u64,
    pub message_id: Option<String>,
    pub entries: Vec<TelegramCollabProgressEntry>,
    pub dropped_entries: usize,
    pub completed: bool,
}

#[derive(Debug, Clone)]
struct TelegramCollabProgressState {
    turn_id: String,
    revision: u64,
    message_id: Option<String>,
    entries: Vec<TelegramCollabProgressEntry>,
    dropped_entries: usize,
    completed: bool,
    dirty: bool,
    sending: bool,
    in_flight_revision: Option<u64>,
    cleanup_after_delivery: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramDraftSendAction {
    Continue,
    Stop,
}

#[derive(Debug, Clone)]
pub struct WecomStreamState {
    pub req_id: String,
    pub stream_id: String,
    pub content: String,
    pub sent_content: String,
    pub finished: bool,
    pub sending: bool,
    pub dirty: bool,
    pub delivered: bool,
    pub cleanup_after_delivery: bool,
    pub revision: u64,
}

impl RuntimeState {
    pub fn start_bridge_generation(&mut self) -> u64 {
        self.bridge_generation = self.bridge_generation.saturating_add(1);
        self.feishu_streaming_cards_by_item.clear();
        self.clear_all_telegram_drafts();
        self.telegram_command_progress_by_thread.clear();
        self.telegram_collab_progress_by_thread.clear();
        self.terminal_notice_failed_by_turn.clear();
        self.terminal_notice_turn_order.clear();
        self.terminal_status_fallback_by_thread.clear();
        self.wecom_streams_by_thread.clear();
        self.pending_attachments_by_conversation.clear();
        self.bridge_generation
    }

    pub fn invalidate_bridge_generation(&mut self) {
        self.bridge_generation = self.bridge_generation.saturating_add(1);
        self.feishu_streaming_cards_by_item.clear();
        self.clear_all_telegram_drafts();
        self.telegram_command_progress_by_thread.clear();
        self.telegram_collab_progress_by_thread.clear();
        self.terminal_notice_failed_by_turn.clear();
        self.terminal_notice_turn_order.clear();
        self.terminal_status_fallback_by_thread.clear();
        self.wecom_streams_by_thread.clear();
        self.pending_attachments_by_conversation.clear();
    }

    #[allow(dead_code)]
    pub fn clear_pending_approvals(&mut self) {
        self.pending_approvals_by_conversation.clear();
        self.pending_approval_request_keys.clear();
    }

    pub fn is_bridge_generation(&self, generation: u64) -> bool {
        self.bridge_generation == generation
    }

    pub fn bind_route(&mut self, thread_id: &str, route: RouteTarget) {
        self.last_route = Some(route.clone());
        let previous = self
            .route_by_thread
            .insert(thread_id.to_string(), route.clone());
        log_route_bind(thread_id, &route, previous.as_ref());
    }

    #[allow(dead_code)]
    pub fn unbind_route(&mut self, thread_id: &str) {
        if let Some(route) = self.route_by_thread.remove(thread_id) {
            log_route_unbind("unbind_thread", "direct", thread_id, &route);
        }
        self.telegram_command_progress_by_thread.remove(thread_id);
        self.telegram_collab_progress_by_thread.remove(thread_id);
        self.terminal_status_fallback_by_thread.remove(thread_id);
    }

    #[allow(dead_code)]
    pub fn unbind_routes_for_conversation(&mut self, conversation_key: &str) -> Vec<String> {
        self.unbind_routes_for_conversation_with_reason(conversation_key, "unspecified")
    }

    pub fn unbind_routes_for_conversation_with_reason(
        &mut self,
        conversation_key: &str,
        reason: &str,
    ) -> Vec<String> {
        let entries = self
            .route_by_thread
            .iter()
            .filter_map(|(thread_id, route)| {
                (route.conversation_key == conversation_key)
                    .then(|| (thread_id.clone(), route.clone()))
            })
            .collect::<Vec<_>>();
        for (thread_id, route) in &entries {
            self.route_by_thread.remove(thread_id);
            if let Some(turn_id) = self.current_turn_by_thread.remove(thread_id) {
                self.turn_origin_by_id.remove(&turn_id);
            }
            self.starting_turn_by_thread.remove(thread_id);
            self.turn_started_at_by_thread.remove(thread_id);
            self.turn_finished_at_by_thread.remove(thread_id);
            self.clear_telegram_drafts_for_thread(thread_id);
            self.telegram_command_progress_by_thread.remove(thread_id);
            self.telegram_collab_progress_by_thread.remove(thread_id);
            self.terminal_status_fallback_by_thread.remove(thread_id);
            log_route_unbind("unbind_conversation", reason, thread_id, route);
        }
        entries
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect()
    }

    pub fn mark_turn_started(&mut self, thread_id: &str, turn_id: &str) {
        self.starting_turn_by_thread.remove(thread_id);
        self.terminal_status_fallback_by_thread.remove(thread_id);
        if self
            .telegram_command_progress_by_thread
            .get(thread_id)
            .is_some_and(|progress| progress.turn_id != turn_id)
        {
            self.telegram_command_progress_by_thread.remove(thread_id);
        }
        if self
            .telegram_collab_progress_by_thread
            .get(thread_id)
            .is_some_and(|progress| progress.turn_id != turn_id)
        {
            self.telegram_collab_progress_by_thread.remove(thread_id);
        }
        self.current_turn_by_thread
            .insert(thread_id.to_string(), turn_id.to_string());
        self.turn_started_at_by_thread
            .insert(thread_id.to_string(), crate::types::now_ms());
    }

    pub fn try_mark_turn_starting(&mut self, thread_id: &str) -> Result<(), ThreadTurnState> {
        if let Some(turn_id) = self.current_turn_by_thread.get(thread_id) {
            return Err(ThreadTurnState::Running(turn_id.clone()));
        }
        if self.starting_turn_by_thread.contains(thread_id) {
            return Err(ThreadTurnState::Starting);
        }
        self.starting_turn_by_thread.insert(thread_id.to_string());
        Ok(())
    }

    pub fn clear_turn_starting(&mut self, thread_id: &str) {
        self.starting_turn_by_thread.remove(thread_id);
    }

    pub fn message_is_stale_for_latest_turn(&self, thread_id: &str, received_at_ms: u128) -> bool {
        received_at_ms > 0
            && (self
                .turn_finished_at_by_thread
                .get(thread_id)
                .is_some_and(|finished_at_ms| received_at_ms < *finished_at_ms)
                || self
                    .turn_started_at_by_thread
                    .get(thread_id)
                    .is_some_and(|started_at_ms| received_at_ms < *started_at_ms))
    }

    pub fn remember_turn_origin(&mut self, turn_id: &str, origin: TurnOrigin) {
        self.turn_origin_by_id.insert(turn_id.to_string(), origin);
    }

    pub(crate) fn claim_terminal_notice(&mut self, turn_id: &str, failed: bool) -> bool {
        if let Some(previous_failed) = self.terminal_notice_failed_by_turn.get_mut(turn_id) {
            // A late failure must still be visible after an optimistic success notice.
            if failed && !*previous_failed {
                *previous_failed = true;
                return true;
            }
            return false;
        }
        self.terminal_notice_failed_by_turn
            .insert(turn_id.to_string(), failed);
        self.terminal_notice_turn_order
            .push_back(turn_id.to_string());
        while self.terminal_notice_turn_order.len() > TERMINAL_NOTICE_TURN_HISTORY_MAX {
            if let Some(oldest) = self.terminal_notice_turn_order.pop_front() {
                self.terminal_notice_failed_by_turn.remove(&oldest);
            }
        }
        true
    }

    pub(crate) fn release_terminal_notice(&mut self, turn_id: &str) {
        if self
            .terminal_notice_failed_by_turn
            .remove(turn_id)
            .is_none()
        {
            return;
        }
        self.terminal_notice_turn_order
            .retain(|current| current != turn_id);
    }

    pub fn turn_origin(&self, turn_id: &str) -> Option<TurnOrigin> {
        self.turn_origin_by_id.get(turn_id).copied()
    }

    pub(crate) fn current_turn_id(&self, thread_id: &str) -> Option<&str> {
        self.current_turn_by_thread
            .get(thread_id)
            .map(String::as_str)
    }

    pub(crate) fn register_terminal_status_fallback(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> bool {
        self.register_terminal_status_fallback_token(thread_id, turn_id)
            .is_some_and(|(_, should_start_server_driver)| should_start_server_driver)
    }

    /// Register the long-lived server-side safety timer for a turn.
    ///
    /// A repeated status notification reuses the existing token instead of
    /// creating another latch. The boolean identifies whether the caller is
    /// responsible for spawning the server-side timer.
    pub(crate) fn register_terminal_status_fallback_token(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<(u64, bool)> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        if let Some(existing) = self.terminal_status_fallback_by_thread.get_mut(thread_id) {
            if existing.turn_id == turn_id {
                let should_start_server_driver = !existing.server_driver_started;
                existing.server_driver_started = true;
                return Some((existing.token, should_start_server_driver));
            }
        }
        let token = self.next_terminal_status_fallback_token();
        self.terminal_status_fallback_by_thread.insert(
            thread_id.to_string(),
            TerminalStatusFallbackState {
                turn_id: turn_id.to_string(),
                token,
                event_driver_started: false,
                server_driver_started: true,
            },
        );
        Some((token, true))
    }

    /// Start the IM-side timer using the same latch as the server safety timer.
    ///
    /// When the server registered the latch first, this marks the existing
    /// entry as owned by the IM driver.  If the IM path is the first observer,
    /// it creates the entry itself.  Only one IM driver can be started.
    pub(crate) fn start_terminal_status_fallback(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<u64> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        if let Some(existing) = self.terminal_status_fallback_by_thread.get_mut(thread_id) {
            if existing.turn_id != turn_id || existing.event_driver_started {
                return None;
            }
            existing.event_driver_started = true;
            return Some(existing.token);
        }
        let token = self.next_terminal_status_fallback_token();
        self.terminal_status_fallback_by_thread.insert(
            thread_id.to_string(),
            TerminalStatusFallbackState {
                turn_id: turn_id.to_string(),
                token,
                event_driver_started: true,
                server_driver_started: false,
            },
        );
        Some(token)
    }

    fn next_terminal_status_fallback_token(&mut self) -> u64 {
        self.next_terminal_status_fallback_token = self
            .next_terminal_status_fallback_token
            .wrapping_add(1)
            .max(1);
        self.next_terminal_status_fallback_token
    }

    pub(crate) fn cancel_terminal_status_fallback(&mut self, thread_id: &str, turn_id: &str) {
        if self
            .terminal_status_fallback_by_thread
            .get(thread_id)
            .is_some_and(|current| current.turn_id == turn_id)
        {
            self.terminal_status_fallback_by_thread.remove(thread_id);
        }
    }

    pub(crate) fn terminal_status_fallback_matches(&self, thread_id: &str, turn_id: &str) -> bool {
        self.terminal_status_fallback_by_thread
            .get(thread_id)
            .is_some_and(|current| current.turn_id == turn_id)
    }

    pub(crate) fn terminal_status_fallback_matches_token(
        &self,
        thread_id: &str,
        turn_id: &str,
        token: u64,
    ) -> bool {
        self.terminal_status_fallback_by_thread
            .get(thread_id)
            .is_some_and(|current| current.turn_id == turn_id && current.token == token)
    }

    /// Atomically consume a fallback latch for the matching current turn.
    pub(crate) fn claim_terminal_status_fallback(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        token: u64,
    ) -> bool {
        if self.current_turn_id(thread_id) != Some(turn_id)
            || !self.terminal_status_fallback_matches_token(thread_id, turn_id, token)
        {
            return false;
        }
        self.terminal_status_fallback_by_thread.remove(thread_id);
        true
    }

    pub fn should_skip_duplicate_text(&self, route_key: &str, text: &str) -> bool {
        self.last_sent_text_by_route
            .get(route_key)
            .map(|last| last == text)
            .unwrap_or(false)
    }

    pub fn remember_sent_text(&mut self, route_key: &str, text: &str) {
        self.last_sent_text_by_route
            .insert(route_key.to_string(), text.to_string());
    }

    pub fn append_telegram_draft_delta(
        &mut self,
        thread_id: &str,
        item_id: &str,
        delta: &str,
    ) -> Option<i64> {
        let key = telegram_draft_key(thread_id, item_id);
        if !self.telegram_drafts_by_item.contains_key(&key) {
            self.next_telegram_draft_id = self.next_telegram_draft_id.saturating_add(1);
            if self.next_telegram_draft_id <= 0 {
                self.next_telegram_draft_id = 1;
            }
            self.telegram_drafts_by_item.insert(
                key.clone(),
                TelegramDraftState {
                    draft_id: self.next_telegram_draft_id,
                    content: String::new(),
                    sending: false,
                    dirty: false,
                    finished: false,
                    revision: 0,
                    last_attempt_at_ms: 0,
                    wake_driver: Arc::new(Notify::new()),
                    completed: Arc::new(Notify::new()),
                },
            );
        }
        let draft = self.telegram_drafts_by_item.get_mut(&key)?;
        draft.content.push_str(delta);
        draft.dirty = true;
        draft.revision = draft.revision.saturating_add(1);
        draft.wake_driver.notify_one();
        if draft.sending {
            return None;
        }
        draft.sending = true;
        Some(draft.draft_id)
    }

    pub fn finish_telegram_draft(
        &mut self,
        thread_id: &str,
        item_id: &str,
        final_text: &str,
    ) -> Option<(i64, bool, Arc<Notify>)> {
        let draft = self
            .telegram_drafts_by_item
            .get_mut(&telegram_draft_key(thread_id, item_id))?;
        draft.content = final_text.to_string();
        draft.dirty = true;
        draft.finished = true;
        draft.revision = draft.revision.saturating_add(1);
        draft.wake_driver.notify_one();
        let should_start = !draft.sending;
        draft.sending = true;
        Some((draft.draft_id, should_start, draft.completed.clone()))
    }

    pub fn take_telegram_draft_snapshot(
        &mut self,
        thread_id: &str,
        item_id: &str,
        draft_id: i64,
        attempted_at_ms: u128,
    ) -> Option<(String, bool, u64)> {
        let draft = self
            .telegram_drafts_by_item
            .get_mut(&telegram_draft_key(thread_id, item_id))?;
        if draft.draft_id != draft_id {
            return None;
        }
        if !draft.sending || !draft.dirty {
            draft.sending = false;
            return None;
        }
        draft.dirty = false;
        draft.last_attempt_at_ms = attempted_at_ms.max(1);
        Some((draft.content.clone(), draft.finished, draft.revision))
    }

    pub fn telegram_draft_send_delay_ms(
        &self,
        thread_id: &str,
        item_id: &str,
        draft_id: i64,
        now_ms: u128,
        throttle_ms: u128,
    ) -> Option<u64> {
        let draft = self
            .telegram_drafts_by_item
            .get(&telegram_draft_key(thread_id, item_id))?;
        if draft.draft_id != draft_id || !draft.sending || !draft.dirty {
            return None;
        }
        if draft.last_attempt_at_ms == 0 {
            return Some(0);
        }
        let remaining = throttle_ms
            .saturating_sub(now_ms.saturating_sub(draft.last_attempt_at_ms))
            .min(u128::from(u64::MAX));
        Some(remaining as u64)
    }

    pub fn telegram_draft_wait_for_update(
        &self,
        thread_id: &str,
        item_id: &str,
        draft_id: i64,
        now_ms: u128,
        heartbeat_ms: u128,
    ) -> Option<(Arc<Notify>, u64)> {
        let draft = self
            .telegram_drafts_by_item
            .get(&telegram_draft_key(thread_id, item_id))?;
        if draft.draft_id != draft_id || !draft.sending || draft.dirty || draft.finished {
            return None;
        }
        let delay_ms = if draft.last_attempt_at_ms == 0 {
            heartbeat_ms
        } else {
            heartbeat_ms.saturating_sub(now_ms.saturating_sub(draft.last_attempt_at_ms))
        }
        .min(u128::from(u64::MAX));
        Some((draft.wake_driver.clone(), delay_ms as u64))
    }

    pub fn mark_telegram_draft_heartbeat_due(
        &mut self,
        thread_id: &str,
        item_id: &str,
        draft_id: i64,
    ) -> bool {
        let Some(draft) = self
            .telegram_drafts_by_item
            .get_mut(&telegram_draft_key(thread_id, item_id))
        else {
            return false;
        };
        if draft.draft_id != draft_id || !draft.sending || draft.finished {
            return false;
        }
        if !draft.dirty {
            draft.dirty = true;
            draft.revision = draft.revision.saturating_add(1);
        }
        true
    }

    pub fn complete_telegram_draft_send(
        &mut self,
        thread_id: &str,
        item_id: &str,
        draft_id: i64,
        revision: u64,
        succeeded: bool,
    ) -> TelegramDraftSendAction {
        let key = telegram_draft_key(thread_id, item_id);
        let Some(draft) = self.telegram_drafts_by_item.get_mut(&key) else {
            return TelegramDraftSendAction::Stop;
        };
        if draft.draft_id != draft_id {
            return TelegramDraftSendAction::Stop;
        }
        if !succeeded {
            if draft.finished {
                if let Some(draft) = self.telegram_drafts_by_item.remove(&key) {
                    draft.completed.notify_one();
                }
            } else {
                draft.sending = false;
                draft.dirty = true;
            }
            return TelegramDraftSendAction::Stop;
        }
        if draft.dirty || draft.revision != revision {
            return TelegramDraftSendAction::Continue;
        }
        if draft.finished {
            if let Some(draft) = self.telegram_drafts_by_item.remove(&key) {
                draft.completed.notify_one();
            }
            return TelegramDraftSendAction::Stop;
        }
        TelegramDraftSendAction::Continue
    }

    pub fn clear_telegram_drafts_for_thread(&mut self, thread_id: &str) {
        let prefix = telegram_draft_thread_prefix(thread_id);
        let keys = self
            .telegram_drafts_by_item
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.remove_telegram_draft(&key);
        }
    }

    fn clear_all_telegram_drafts(&mut self) {
        for (_, draft) in self.telegram_drafts_by_item.drain() {
            draft.wake_driver.notify_one();
            draft.completed.notify_one();
        }
    }

    fn remove_telegram_draft(&mut self, key: &str) {
        if let Some(draft) = self.telegram_drafts_by_item.remove(key) {
            draft.wake_driver.notify_one();
            draft.completed.notify_one();
        }
    }

    pub(crate) fn upsert_telegram_command_progress(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        entry: TelegramCommandProgressEntry,
        deliver_update: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id {
            *progress = telegram_command_progress_state(turn_id);
        }
        if progress.completed {
            return None;
        }

        let (changed, new_entry) = match progress
            .entries
            .iter_mut()
            .find(|current| current.item_id == entry.item_id)
        {
            Some(current) if *current == entry => (false, false),
            Some(current) => {
                *current = entry;
                (true, false)
            }
            None => {
                progress.entries.push(entry);
                if progress.entries.len() > TELEGRAM_COMMAND_PROGRESS_MAX_ENTRIES {
                    let excess = progress.entries.len() - TELEGRAM_COMMAND_PROGRESS_MAX_ENTRIES;
                    progress.entries.drain(..excess);
                    progress.dropped_entries = progress.dropped_entries.saturating_add(excess);
                }
                (true, true)
            }
        };
        if !changed {
            return None;
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        // A newly discovered command must be visible immediately even after
        // the aggregate message has already been created.  Existing entries
        // remain coalesced until completion (or an explicit final flush).
        if deliver_update || new_entry || progress.message_id.is_none() {
            Some(telegram_command_progress_snapshot(progress))
        } else {
            None
        }
    }

    pub(crate) fn record_telegram_retry(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        error: Option<String>,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id {
            *progress = telegram_command_progress_state(turn_id);
        }
        if progress.completed {
            return None;
        }

        progress.retry_count = progress.retry_count.saturating_add(1);
        if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
            progress.retry_error = Some(error);
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        Some(telegram_command_progress_snapshot(progress))
    }

    /// Cache reasoning deltas without touching Telegram for every token-sized
    /// notification. The completed reasoning item flushes this buffer into the
    /// aggregate progress message.
    pub(crate) fn append_telegram_reasoning_delta(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        summary_index: i64,
        delta: &str,
    ) {
        if delta.is_empty() || self.current_turn_id(thread_id) != Some(turn_id) {
            return;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return;
        }
        if progress.reasoning_item_id.as_deref() != Some(item_id) {
            progress.reasoning_item_id = Some(item_id.to_string());
            progress.reasoning_summary_index = Some(summary_index);
            progress.reasoning_summary.clear();
            progress.reasoning_completed = false;
        } else if progress.reasoning_summary_index != Some(summary_index) {
            if !progress.reasoning_summary.trim().is_empty() {
                progress.reasoning_summary.push_str("\n\n");
            }
            progress.reasoning_summary_index = Some(summary_index);
        }
        append_bounded_text(
            &mut progress.reasoning_summary,
            delta,
            TELEGRAM_REASONING_MAX_CHARS,
        );
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
    }

    pub(crate) fn complete_telegram_reasoning(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        summary: Option<String>,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        let mut content_changed = false;
        if let Some(summary) = summary.filter(|value| !value.trim().is_empty()) {
            content_changed = progress.reasoning_item_id.as_deref() != Some(item_id)
                || progress.reasoning_summary != summary;
            progress.reasoning_item_id = Some(item_id.to_string());
            progress.reasoning_summary_index = None;
            progress.reasoning_summary = summary;
        } else if progress.reasoning_item_id.as_deref() != Some(item_id) {
            return None;
        }
        if progress.reasoning_summary.trim().is_empty() {
            return None;
        }
        let completion_changed = !progress.reasoning_completed;
        if !content_changed && !completion_changed {
            return None;
        }
        progress.reasoning_completed = true;
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        Some(telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn update_telegram_plan(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        explanation: Option<String>,
        plan: Vec<TelegramPlanStep>,
        deliver_update: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        let explanation = explanation.filter(|value| !value.trim().is_empty());
        let plan = plan
            .into_iter()
            .take(TELEGRAM_PLAN_MAX_STEPS)
            .collect::<Vec<_>>();
        if progress.plan_explanation == explanation && progress.plan == plan {
            return None;
        }
        progress.plan_explanation = explanation;
        progress.plan = plan;
        if progress.message_id.is_none() && !telegram_command_progress_has_content(progress) {
            progress.dirty = false;
            return None;
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        deliver_update.then(|| telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn update_telegram_diff(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        diff_summary: Option<TelegramDiffSummary>,
        deliver_update: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        let Some(diff_summary) = diff_summary else {
            if progress.diff_summary.take().is_some() {
                progress.revision = progress.revision.saturating_add(1);
                progress.dirty = true;
                return deliver_update.then(|| telegram_command_progress_snapshot(progress));
            }
            return None;
        };
        let diff_summary = limit_telegram_diff_summary(diff_summary);
        if progress.diff_summary.as_ref() == Some(&diff_summary) {
            return None;
        }
        progress.diff_summary = Some(diff_summary);
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        deliver_update.then(|| telegram_command_progress_snapshot(progress))
    }

    /// Flush a file-change completion into the aggregate message. Turn-level
    /// diff notifications are cached first; the item fallback is only used
    /// when no aggregate diff has arrived yet.
    pub(crate) fn complete_telegram_file_change(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        fallback: Option<TelegramDiffSummary>,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        if progress.diff_summary.is_none() {
            if let Some(fallback) = fallback {
                progress.diff_summary = Some(limit_telegram_diff_summary(fallback));
                progress.revision = progress.revision.saturating_add(1);
                progress.dirty = true;
            }
        }
        progress
            .dirty
            .then(|| telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn finish_telegram_command_progress(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCommandProgressSnapshot> {
        self.finish_telegram_command_progress_with_outcome(thread_id, turn_id, false)
    }

    pub(crate) fn finish_telegram_command_progress_with_outcome(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        failed: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id || !telegram_command_progress_has_content(progress) {
            return None;
        }
        let interrupted = progress
            .entries
            .iter_mut()
            .filter(|entry| entry.status == TelegramCommandProgressStatus::Running)
            .map(|entry| {
                entry.status = TelegramCommandProgressStatus::Interrupted;
            })
            .count();
        if failed {
            progress.failed = true;
        }
        if !progress.completed || interrupted > 0 || failed {
            progress.completed = true;
            progress.revision = progress.revision.saturating_add(1);
            progress.dirty = true;
        }
        progress
            .dirty
            .then(|| telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn claim_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let current_turn_id = self.current_turn_id(thread_id).map(str::to_string);
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id
            || progress.sending
            || !progress.dirty
            || (current_turn_id.as_deref() != Some(turn_id) && !progress.cleanup_after_delivery)
        {
            return None;
        }
        progress.sending = true;
        claim_next_telegram_command_progress_snapshot(progress)
    }

    pub(crate) fn telegram_command_progress_delivery_is_current(
        &self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
    ) -> bool {
        let Some(progress) = self.telegram_command_progress_by_thread.get(thread_id) else {
            return false;
        };
        progress.turn_id == turn_id
            && progress.sending
            && progress.in_flight_revision == Some(revision)
            && (self.current_turn_id(thread_id) == Some(turn_id) || progress.cleanup_after_delivery)
    }

    pub(crate) fn complete_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
        message_id: String,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let mut cleanup = false;
        let next = {
            let progress = self
                .telegram_command_progress_by_thread
                .get_mut(thread_id)?;
            if progress.turn_id != turn_id
                || !progress.sending
                || progress.in_flight_revision != Some(revision)
            {
                return None;
            }
            progress.message_id = Some(message_id);
            progress.in_flight_revision = None;
            if progress.dirty {
                claim_next_telegram_command_progress_snapshot(progress)
            } else {
                progress.sending = false;
                cleanup = progress.completed && progress.cleanup_after_delivery;
                None
            }
        };
        if cleanup {
            self.telegram_command_progress_by_thread.remove(thread_id);
        }
        next
    }

    pub(crate) fn fail_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
    ) -> bool {
        let Some(progress) = self.telegram_command_progress_by_thread.get_mut(thread_id) else {
            return false;
        };
        if progress.turn_id != turn_id
            || !progress.sending
            || progress.in_flight_revision != Some(revision)
        {
            return false;
        }
        progress.in_flight_revision = None;
        progress.dirty = true;
        if progress.completed {
            true
        } else {
            progress.sending = false;
            false
        }
    }

    pub(crate) fn retry_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id
            || !progress.sending
            || progress.in_flight_revision.is_some()
            || !progress.dirty
        {
            return None;
        }
        claim_next_telegram_command_progress_snapshot(progress)
    }

    pub(crate) fn remember_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
        message_id: String,
    ) {
        let Some(progress) = self.telegram_command_progress_by_thread.get_mut(thread_id) else {
            return;
        };
        if progress.turn_id != turn_id {
            return;
        }
        progress.message_id = Some(message_id);
        if progress.revision == revision {
            progress.dirty = false;
        }
    }

    pub(crate) fn clear_telegram_command_progress_for_thread(&mut self, thread_id: &str) {
        self.telegram_command_progress_by_thread.remove(thread_id);
    }

    pub(crate) fn apply_telegram_collab_progress_updates(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        updates: Vec<TelegramCollabProgressUpdate>,
    ) -> Option<TelegramCollabProgressSnapshot> {
        if updates.is_empty() || self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_collab_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| TelegramCollabProgressState {
                turn_id: turn_id.to_string(),
                revision: 0,
                message_id: None,
                entries: Vec::new(),
                dropped_entries: 0,
                completed: false,
                dirty: false,
                sending: false,
                in_flight_revision: None,
                cleanup_after_delivery: false,
            });
        if progress.turn_id != turn_id {
            *progress = TelegramCollabProgressState {
                turn_id: turn_id.to_string(),
                revision: 0,
                message_id: None,
                entries: Vec::new(),
                dropped_entries: 0,
                completed: false,
                dirty: false,
                sending: false,
                in_flight_revision: None,
                cleanup_after_delivery: false,
            };
        }
        if progress.completed {
            return None;
        }

        let mut changed = false;
        for update in updates {
            let timestamp = update.occurred_at_ms.max(1);
            if let Some(entry) = progress
                .entries
                .iter_mut()
                .find(|entry| entry.agent_id == update.agent_id)
            {
                if timestamp < entry.updated_at_ms {
                    continue;
                }
                let mut entry_changed = false;
                let mut clear_stale_detail = false;
                if let Some(name) = update.name.filter(|name| !name.trim().is_empty())
                    && entry.name != name
                {
                    entry.name = name;
                    entry_changed = true;
                }
                if let Some(status) = update.status {
                    let should_change = if entry.status == status {
                        false
                    } else if update.restart && status == TelegramCollabProgressStatus::Running {
                        true
                    } else {
                        match entry.status {
                            TelegramCollabProgressStatus::Running => true,
                            TelegramCollabProgressStatus::Responded => matches!(
                                status,
                                TelegramCollabProgressStatus::Succeeded
                                    | TelegramCollabProgressStatus::Failed
                            ),
                            TelegramCollabProgressStatus::Succeeded
                            | TelegramCollabProgressStatus::Interrupted => {
                                status == TelegramCollabProgressStatus::Failed
                            }
                            TelegramCollabProgressStatus::Failed => false,
                        }
                    };
                    clear_stale_detail = (entry.status == status || should_change)
                        && (status == TelegramCollabProgressStatus::Succeeded
                            || (update.restart && status == TelegramCollabProgressStatus::Running));
                    if should_change {
                        if status == TelegramCollabProgressStatus::Running {
                            entry.started_at_ms = timestamp;
                        }
                        entry.status = status;
                        entry_changed = true;
                    }
                }
                if let Some(detail) = update.detail.filter(|detail| !detail.trim().is_empty()) {
                    if entry.detail.as_deref() != Some(detail.as_str()) {
                        entry.detail = Some(detail);
                        entry_changed = true;
                    }
                } else if clear_stale_detail && entry.detail.take().is_some() {
                    entry_changed = true;
                }
                if entry_changed {
                    entry.updated_at_ms =
                        entry.updated_at_ms.max(timestamp).max(entry.started_at_ms);
                    changed = true;
                }
                continue;
            }

            if !update.create_if_missing {
                continue;
            }
            let Some(name) = update.name.filter(|name| !name.trim().is_empty()) else {
                continue;
            };
            progress.entries.push(TelegramCollabProgressEntry {
                agent_id: update.agent_id,
                name,
                status: update
                    .status
                    .unwrap_or(TelegramCollabProgressStatus::Running),
                detail: update.detail.filter(|detail| !detail.trim().is_empty()),
                started_at_ms: timestamp,
                updated_at_ms: timestamp,
            });
            if progress.entries.len() > TELEGRAM_COLLAB_PROGRESS_MAX_AGENTS {
                let excess = progress.entries.len() - TELEGRAM_COLLAB_PROGRESS_MAX_AGENTS;
                progress.entries.drain(..excess);
                progress.dropped_entries = progress.dropped_entries.saturating_add(excess);
            }
            changed = true;
        }

        if !changed {
            return None;
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        claim_telegram_collab_progress_snapshot(progress)
    }

    pub(crate) fn finish_telegram_collab_progress(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCollabProgressSnapshot> {
        let progress = self.telegram_collab_progress_by_thread.get_mut(thread_id)?;
        if progress.turn_id != turn_id || progress.entries.is_empty() {
            return None;
        }
        let now_ms = crate::types::now_ms();
        let interrupted = progress
            .entries
            .iter_mut()
            .filter(|entry| entry.status == TelegramCollabProgressStatus::Running)
            .map(|entry| {
                entry.status = TelegramCollabProgressStatus::Interrupted;
                entry.updated_at_ms = entry.updated_at_ms.max(now_ms).max(entry.started_at_ms);
            })
            .count();
        if !progress.completed || interrupted > 0 {
            progress.completed = true;
            progress.revision = progress.revision.saturating_add(1);
            progress.dirty = true;
        }
        claim_telegram_collab_progress_snapshot(progress)
    }

    pub(crate) fn complete_telegram_collab_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
        message_id: String,
    ) -> Option<TelegramCollabProgressSnapshot> {
        let mut cleanup = false;
        let next = {
            let progress = self.telegram_collab_progress_by_thread.get_mut(thread_id)?;
            if progress.turn_id != turn_id
                || !progress.sending
                || progress.in_flight_revision != Some(revision)
            {
                return None;
            }
            progress.message_id = Some(message_id);
            progress.in_flight_revision = None;
            if progress.dirty {
                claim_next_telegram_collab_progress_snapshot(progress)
            } else {
                progress.sending = false;
                cleanup = progress.completed && progress.cleanup_after_delivery;
                None
            }
        };
        if cleanup {
            self.telegram_collab_progress_by_thread.remove(thread_id);
        }
        next
    }

    pub(crate) fn fail_telegram_collab_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
    ) -> bool {
        let Some(progress) = self.telegram_collab_progress_by_thread.get_mut(thread_id) else {
            return false;
        };
        if progress.turn_id != turn_id
            || !progress.sending
            || progress.in_flight_revision != Some(revision)
        {
            return false;
        }
        progress.in_flight_revision = None;
        progress.dirty = true;
        if progress.completed {
            true
        } else {
            progress.sending = false;
            false
        }
    }

    pub(crate) fn retry_telegram_collab_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCollabProgressSnapshot> {
        let progress = self.telegram_collab_progress_by_thread.get_mut(thread_id)?;
        if progress.turn_id != turn_id
            || !progress.sending
            || progress.in_flight_revision.is_some()
            || !progress.dirty
        {
            return None;
        }
        claim_next_telegram_collab_progress_snapshot(progress)
    }

    pub(crate) fn clear_telegram_collab_progress_for_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) {
        if self
            .telegram_collab_progress_by_thread
            .get(thread_id)
            .is_some_and(|progress| progress.turn_id == turn_id)
        {
            self.telegram_collab_progress_by_thread.remove(thread_id);
        }
    }

    pub(crate) fn clear_telegram_collab_progress_for_thread(&mut self, thread_id: &str) {
        self.telegram_collab_progress_by_thread.remove(thread_id);
    }

    pub fn hold_pending_attachments(
        &mut self,
        conversation_key: &str,
        attachments: Vec<InboundAttachment>,
        received_at_ms: u128,
    ) -> usize {
        let entry = self
            .pending_attachments_by_conversation
            .entry(conversation_key.to_string())
            .or_insert_with(|| PendingAttachments {
                attachments: Vec::new(),
                received_at_ms,
            });
        if received_at_ms.saturating_sub(entry.received_at_ms) > PENDING_ATTACHMENTS_MAX_AGE_MS {
            entry.attachments.clear();
        }
        entry.received_at_ms = received_at_ms;
        entry.attachments.extend(attachments);
        if entry.attachments.len() > PENDING_ATTACHMENTS_MAX_COUNT {
            let excess = entry.attachments.len() - PENDING_ATTACHMENTS_MAX_COUNT;
            entry.attachments.drain(..excess);
        }
        entry.attachments.len()
    }

    pub fn take_pending_attachments(
        &mut self,
        conversation_key: &str,
        received_at_ms: u128,
    ) -> Vec<InboundAttachment> {
        self.pending_attachments_by_conversation
            .remove(conversation_key)
            .filter(|entry| {
                received_at_ms.saturating_sub(entry.received_at_ms)
                    <= PENDING_ATTACHMENTS_MAX_AGE_MS
            })
            .map(|entry| entry.attachments)
            .unwrap_or_default()
    }

    pub fn mark_turn_completed(&mut self, thread_id: &str, turn_id: Option<&str>) -> bool {
        let current_turn_id = self
            .current_turn_by_thread
            .get(thread_id)
            .map(String::as_str);
        if let Some(turn_id) = turn_id
            && current_turn_id != Some(turn_id)
        {
            self.turn_origin_by_id.remove(turn_id);
            return false;
        }
        self.terminal_status_fallback_by_thread.remove(thread_id);
        self.starting_turn_by_thread.remove(thread_id);
        let completed_turn_id = self.current_turn_by_thread.remove(thread_id);
        if let Some(turn_id) = turn_id.or(completed_turn_id.as_deref()) {
            self.turn_origin_by_id.remove(turn_id);
        }
        self.clear_telegram_drafts_for_thread(thread_id);
        let retain_command_progress =
            if let Some(progress) = self.telegram_command_progress_by_thread.get_mut(thread_id) {
                if progress.completed && progress.sending {
                    progress.cleanup_after_delivery = true;
                    true
                } else {
                    false
                }
            } else {
                false
            };
        if !retain_command_progress {
            self.clear_telegram_command_progress_for_thread(thread_id);
        }
        let retain_collab_progress =
            if let Some(progress) = self.telegram_collab_progress_by_thread.get_mut(thread_id) {
                if progress.completed && (progress.sending || progress.dirty) {
                    progress.cleanup_after_delivery = true;
                    true
                } else {
                    false
                }
            } else {
                false
            };
        if !retain_collab_progress {
            self.clear_telegram_collab_progress_for_thread(thread_id);
        }
        self.turn_finished_at_by_thread
            .insert(thread_id.to_string(), crate::types::now_ms());
        true
    }

    pub fn route_for_thread(&self, thread_id: &str) -> Option<RouteTarget> {
        self.route_by_thread.get(thread_id).cloned()
    }

    pub fn has_pending_approvals(&self, conversation_key: &str) -> bool {
        self.pending_approvals_by_conversation
            .get(conversation_key)
            .is_some_and(|approvals| !approvals.is_empty())
    }

    pub fn push_approval(&mut self, conversation_key: String, approval: PendingApproval) -> bool {
        let request_key = approval.request_key();
        if !self.pending_approval_request_keys.insert(request_key) {
            return false;
        }
        self.pending_approvals_by_conversation
            .entry(conversation_key)
            .or_default()
            .push(approval);
        true
    }

    pub fn current_approval(&self, conversation_key: &str) -> Option<PendingApproval> {
        self.pending_approvals_by_conversation
            .get(conversation_key)
            .and_then(|approvals| approvals.first())
            .cloned()
    }

    pub fn is_current_approval(&self, conversation_key: &str, request_key: &str) -> bool {
        self.current_approval(conversation_key)
            .is_some_and(|approval| approval.request_key() == request_key)
    }

    #[allow(dead_code)]
    pub fn approval_by_request_key(
        &self,
        conversation_key: &str,
        request_key: &str,
    ) -> Option<PendingApproval> {
        self.pending_approvals_by_conversation
            .get(conversation_key)
            .and_then(|approvals| {
                approvals
                    .iter()
                    .find(|approval| approval.request_key() == request_key)
                    .cloned()
            })
    }

    pub fn approval_by_request_key_anywhere(
        &self,
        request_key: &str,
    ) -> Option<(String, PendingApproval)> {
        self.pending_approvals_by_conversation
            .iter()
            .find_map(|(conversation_key, approvals)| {
                approvals
                    .iter()
                    .find(|approval| approval.request_key() == request_key)
                    .cloned()
                    .map(|approval| (conversation_key.clone(), approval))
            })
    }

    pub fn remember_approval_message_id(&mut self, request_id: &Value, message_id: String) -> bool {
        let request_key = approval_request_key(request_id);
        for approvals in self.pending_approvals_by_conversation.values_mut() {
            if let Some(approval) = approvals
                .iter_mut()
                .find(|approval| approval.request_key() == request_key)
            {
                approval.message_id = Some(message_id);
                return true;
            }
        }
        false
    }

    #[allow(dead_code)]
    pub fn resolve_approval_request(&mut self, request_id: &Value) -> Option<PendingApproval> {
        self.resolve_approval_request_with_context(request_id)
            .map(|resolved| resolved.approval)
    }

    pub fn resolve_approval_request_with_context(
        &mut self,
        request_id: &Value,
    ) -> Option<ResolvedApproval> {
        let request_key = approval_request_key(request_id);
        self.pending_approval_request_keys.remove(&request_key);

        let mut resolved = None;
        let mut empty_key = None;
        for (conversation_key, approvals) in &mut self.pending_approvals_by_conversation {
            if let Some(index) = approvals
                .iter()
                .position(|approval| approval.request_key() == request_key)
            {
                let approval = approvals.remove(index);
                let was_current = index == 0;
                let next_current = was_current.then(|| approvals.first().cloned()).flatten();
                if approvals.is_empty() {
                    empty_key = Some(conversation_key.clone());
                }
                resolved = Some(ResolvedApproval {
                    conversation_key: conversation_key.clone(),
                    approval,
                    was_current,
                    next_current,
                });
                break;
            }
        }
        if let Some(conversation_key) = empty_key {
            self.pending_approvals_by_conversation
                .remove(&conversation_key);
        }
        resolved
    }

    pub fn remember_thread_routing_request(&mut self, request: ThreadRoutingRequestState) {
        self.thread_routing_requests
            .insert(request.request_id.clone(), request);
    }

    pub fn thread_routing_request(&self, request_id: &str) -> Option<ThreadRoutingRequestState> {
        self.thread_routing_requests.get(request_id).cloned()
    }

    pub fn update_thread_routing_request_message_id(
        &mut self,
        request_id: &str,
        message_id: String,
    ) -> bool {
        let Some(request) = self.thread_routing_requests.get_mut(request_id) else {
            return false;
        };
        request.message_id = Some(message_id);
        true
    }

    #[allow(dead_code)]
    pub fn update_thread_routing_request_page(
        &mut self,
        request_id: &str,
        page: usize,
        page_cursors: Vec<Option<String>>,
        thread_ids_by_page: Vec<Vec<String>>,
        history_cursor: Option<String>,
        history_has_next: bool,
    ) -> Option<ThreadRoutingRequestState> {
        let request = self.thread_routing_requests.get_mut(request_id)?;
        request.page = page;
        request.page_cursors = page_cursors;
        request.thread_ids_by_page = thread_ids_by_page;
        request.history_cursor = history_cursor;
        request.history_has_next = history_has_next;
        Some(request.clone())
    }

    pub fn clear_thread_routing_request(
        &mut self,
        request_id: &str,
    ) -> Option<ThreadRoutingRequestState> {
        self.thread_routing_requests.remove(request_id)
    }
}

fn telegram_command_progress_snapshot(
    progress: &TelegramCommandProgressState,
) -> TelegramCommandProgressSnapshot {
    TelegramCommandProgressSnapshot {
        turn_id: progress.turn_id.clone(),
        revision: progress.revision,
        message_id: progress.message_id.clone(),
        entries: progress.entries.clone(),
        dropped_entries: progress.dropped_entries,
        retry_count: progress.retry_count,
        retry_error: progress.retry_error.clone(),
        reasoning_summary: progress
            .reasoning_completed
            .then(|| progress.reasoning_summary.clone())
            .filter(|value| !value.trim().is_empty()),
        plan_explanation: progress.plan_explanation.clone(),
        plan: progress.plan.clone(),
        diff_summary: progress.diff_summary.clone(),
        completed: progress.completed,
        failed: progress.failed,
    }
}

fn telegram_command_progress_state(turn_id: &str) -> TelegramCommandProgressState {
    TelegramCommandProgressState {
        turn_id: turn_id.to_string(),
        revision: 0,
        message_id: None,
        entries: Vec::new(),
        dropped_entries: 0,
        retry_count: 0,
        retry_error: None,
        reasoning_item_id: None,
        reasoning_summary_index: None,
        reasoning_summary: String::new(),
        reasoning_completed: false,
        plan_explanation: None,
        plan: Vec::new(),
        diff_summary: None,
        completed: false,
        failed: false,
        dirty: false,
        sending: false,
        in_flight_revision: None,
        cleanup_after_delivery: false,
    }
}

fn telegram_command_progress_has_content(progress: &TelegramCommandProgressState) -> bool {
    !progress.entries.is_empty()
        || progress.retry_count > 0
        || (progress.reasoning_completed && !progress.reasoning_summary.trim().is_empty())
        || progress.plan_explanation.is_some()
        || !progress.plan.is_empty()
        || progress.diff_summary.is_some()
}

fn claim_next_telegram_command_progress_snapshot(
    progress: &mut TelegramCommandProgressState,
) -> Option<TelegramCommandProgressSnapshot> {
    if !progress.sending || progress.in_flight_revision.is_some() || !progress.dirty {
        return None;
    }
    progress.dirty = false;
    progress.in_flight_revision = Some(progress.revision);
    Some(telegram_command_progress_snapshot(progress))
}

fn append_bounded_text(target: &mut String, delta: &str, max_chars: usize) {
    target.push_str(delta);
    let overflow = target.chars().count().saturating_sub(max_chars);
    if overflow == 0 {
        return;
    }
    *target = target.chars().skip(overflow).collect();
}

fn limit_telegram_diff_summary(mut summary: TelegramDiffSummary) -> TelegramDiffSummary {
    if summary.paths.len() > TELEGRAM_DIFF_MAX_PATHS {
        let omitted = summary.paths.len() - TELEGRAM_DIFF_MAX_PATHS;
        summary.paths.truncate(TELEGRAM_DIFF_MAX_PATHS);
        summary.omitted_paths = summary.omitted_paths.saturating_add(omitted);
    }
    summary
}

fn telegram_collab_progress_snapshot(
    progress: &TelegramCollabProgressState,
) -> TelegramCollabProgressSnapshot {
    TelegramCollabProgressSnapshot {
        turn_id: progress.turn_id.clone(),
        revision: progress.revision,
        message_id: progress.message_id.clone(),
        entries: progress.entries.clone(),
        dropped_entries: progress.dropped_entries,
        completed: progress.completed,
    }
}

fn claim_telegram_collab_progress_snapshot(
    progress: &mut TelegramCollabProgressState,
) -> Option<TelegramCollabProgressSnapshot> {
    if progress.sending || !progress.dirty {
        return None;
    }
    progress.sending = true;
    claim_next_telegram_collab_progress_snapshot(progress)
}

fn claim_next_telegram_collab_progress_snapshot(
    progress: &mut TelegramCollabProgressState,
) -> Option<TelegramCollabProgressSnapshot> {
    if !progress.sending || progress.in_flight_revision.is_some() || !progress.dirty {
        return None;
    }
    progress.dirty = false;
    progress.in_flight_revision = Some(progress.revision);
    Some(telegram_collab_progress_snapshot(progress))
}

fn telegram_draft_key(thread_id: &str, item_id: &str) -> String {
    format!("{}{item_id}", telegram_draft_thread_prefix(thread_id))
}

fn telegram_draft_thread_prefix(thread_id: &str) -> String {
    format!("{thread_id}\u{1f}")
}

impl RouteTarget {
    pub fn deterministic_remote_client_key_for(
        platform: ImPlatformKind,
        account_id: &str,
        chat_id: &str,
    ) -> String {
        let source = format!(
            "{}:{}:{}",
            platform.key(),
            account_id.trim(),
            chat_id.trim()
        );
        let digest = Sha256::digest(source.as_bytes());
        let mut suffix = String::with_capacity(16);
        for byte in digest.iter().take(8) {
            let _ = write!(&mut suffix, "{byte:02x}");
        }
        format!("im:{}:{suffix}", platform.key())
    }

    pub fn deterministic_remote_client_key(&self) -> String {
        Self::deterministic_remote_client_key_for(self.platform, &self.account_id, &self.chat_id)
    }

    pub fn with_deterministic_remote_client_key(mut self) -> Self {
        self.remote_client_key = self.deterministic_remote_client_key();
        self
    }
}

fn log_route_bind(thread_id: &str, route: &RouteTarget, previous: Option<&RouteTarget>) {
    match previous {
        Some(previous) => chain_log::write_line(format!(
            "[im_route] level=warn event=bind_overwrite thread={} platform={} account={} chat={} conversation={} previous_platform={} previous_account={} previous_chat={} previous_conversation={}",
            thread_id,
            route.platform.key(),
            route.account_id,
            route.chat_id,
            route.conversation_key,
            previous.platform.key(),
            previous.account_id,
            previous.chat_id,
            previous.conversation_key
        )),
        None => chain_log::write_diagnostic_lazy(|| {
            format!(
                "[im_route] event=bind thread={} platform={} account={} chat={} conversation={}",
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            )
        }),
    }
}

fn log_route_unbind(event: &str, reason: &str, thread_id: &str, route: &RouteTarget) {
    chain_log::write_line(format!(
        "[im_route] level=warn event={} reason={} thread={} platform={} account={} chat={} conversation={}",
        event,
        reason,
        thread_id,
        route.platform.key(),
        route.account_id,
        route.chat_id,
        route.conversation_key
    ));
}

impl PendingApproval {
    pub fn request_key(&self) -> String {
        approval_request_key(&self.request_id)
    }
}

pub fn approval_request_key(request_id: &Value) -> String {
    match request_id {
        Value::Number(value) => format!("number:{value}"),
        Value::String(value) => format!("string:{value}"),
        other => format!("json:{other}"),
    }
}

pub fn approval_request_fingerprint(request_key: &str) -> String {
    let digest = Sha256::digest(request_key.as_bytes());
    let mut fingerprint = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(&mut fingerprint, "{byte:02x}");
    }
    fingerprint
}

pub fn route_from_conversation_key(conversation_key: &str) -> Option<RouteTarget> {
    let mut parts = conversation_key.splitn(3, ':');
    let channel = parts.next()?;
    let platform = match channel {
        "feishu" => ImPlatformKind::Feishu,
        "telegram" => ImPlatformKind::Telegram,
        "wechat" => ImPlatformKind::Wechat,
        "wecom" => ImPlatformKind::Wecom,
        _ => return None,
    };
    let account_id = parts.next()?.to_string();
    let chat_id = parts.next()?.to_string();
    Some(
        RouteTarget {
            platform,
            conversation_key: conversation_key.to_string(),
            account_id,
            chat_id,
            remote_client_key: String::new(),
        }
        .with_deterministic_remote_client_key(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::types::{ImPlatformKind, InboundAttachment};

    use super::{
        PENDING_ATTACHMENTS_MAX_AGE_MS, PendingApproval, RouteTarget, RuntimeState,
        TelegramCollabProgressStatus, TelegramCollabProgressUpdate, TelegramCommandProgressEntry,
        TelegramCommandProgressStatus, TelegramDiffSummary, TelegramDraftSendAction,
        TelegramPlanStep, TelegramPlanStepStatus, ThreadTurnState, TurnOrigin,
        route_from_conversation_key,
    };

    fn approval(id: i64) -> PendingApproval {
        PendingApproval {
            request_id: json!(id),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({
                "threadId": "thread",
                "turnId": "turn",
                "itemId": "item",
                "command": "test",
                "cwd": "D:\\test"
            }),
            summary: "command: `test`".to_string(),
            decisions: vec![],
            message_id: None,
            remote_client_key: None,
        }
    }

    fn image_attachment(index: usize) -> InboundAttachment {
        InboundAttachment {
            kind: "image".to_string(),
            name: Some(format!("image-{index}.png")),
            mime_type: Some("image/png".to_string()),
            text_hint: None,
            local_path: Some(format!("C:\\temp\\image-{index}.png")),
        }
    }

    fn command_progress_entry(
        item_id: &str,
        command: &str,
        status: TelegramCommandProgressStatus,
    ) -> TelegramCommandProgressEntry {
        TelegramCommandProgressEntry {
            item_id: item_id.to_string(),
            command: command.to_string(),
            status,
            exit_code: None,
            duration_ms: None,
            failure_output: None,
        }
    }

    fn collab_progress_update(
        agent_id: &str,
        name: Option<&str>,
        status: Option<TelegramCollabProgressStatus>,
        occurred_at_ms: u128,
    ) -> TelegramCollabProgressUpdate {
        TelegramCollabProgressUpdate {
            agent_id: agent_id.to_string(),
            name: name.map(str::to_string),
            status,
            detail: None,
            occurred_at_ms,
            create_if_missing: name.is_some(),
            restart: false,
        }
    }

    #[test]
    fn pending_attachments_accumulate_until_description_arrives() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:chat-1";

        assert_eq!(
            runtime.hold_pending_attachments(route, vec![image_attachment(1)], 1_000),
            1
        );
        assert_eq!(
            runtime.hold_pending_attachments(route, vec![image_attachment(2)], 2_000),
            2
        );

        let attachments = runtime.take_pending_attachments(route, 3_000);
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].name.as_deref(), Some("image-1.png"));
        assert_eq!(attachments[1].name.as_deref(), Some("image-2.png"));
        assert!(runtime.take_pending_attachments(route, 4_000).is_empty());
    }

    #[test]
    fn pending_attachments_keep_only_the_latest_eight_images() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:chat-1";
        let attachments = (0..10).map(image_attachment).collect();

        assert_eq!(
            runtime.hold_pending_attachments(route, attachments, 1_000),
            8
        );

        let attachments = runtime.take_pending_attachments(route, 2_000);
        assert_eq!(attachments.len(), 8);
        assert_eq!(attachments[0].name.as_deref(), Some("image-2.png"));
        assert_eq!(attachments[7].name.as_deref(), Some("image-9.png"));
    }

    #[test]
    fn pending_attachments_expire_after_ten_minutes() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:chat-1";
        runtime.hold_pending_attachments(route, vec![image_attachment(1)], 1_000);

        assert!(
            runtime
                .take_pending_attachments(route, 1_000 + PENDING_ATTACHMENTS_MAX_AGE_MS + 1)
                .is_empty()
        );
    }

    #[test]
    fn telegram_command_progress_reuses_one_message_and_deduplicates_items() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item-1",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                false,
            )
            .expect("first command should create a message");
        assert_eq!(first.message_id, None);
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            first.revision,
            "77".to_string(),
        );

        let second = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item-2",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                false,
            )
            .expect("new running command should edit the aggregate immediately");
        assert_eq!(second.message_id.as_deref(), Some("77"));
        assert_eq!(second.entries.len(), 2);
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            second.revision,
            "77".to_string(),
        );
        let completed = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item-2",
                    "cargo test",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("completion should edit the aggregate");
        assert_eq!(completed.message_id.as_deref(), Some("77"));
        assert_eq!(completed.entries.len(), 2);

        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            completed.revision,
            "77".to_string(),
        );
        assert!(
            runtime
                .upsert_telegram_command_progress(
                    "thread",
                    "turn",
                    command_progress_entry(
                        "item-2",
                        "cargo test",
                        TelegramCommandProgressStatus::Succeeded,
                    ),
                    true,
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_command_progress_allows_only_one_in_flight_snapshot() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let created = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");
        assert_eq!(claimed.revision, created.revision);
        assert!(
            runtime
                .claim_telegram_command_progress_delivery("thread", "turn")
                .is_none()
        );
        assert!(runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "turn",
            claimed.revision
        ));
        assert!(!runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "turn",
            claimed.revision.saturating_add(1)
        ));
    }

    #[test]
    fn telegram_command_progress_queues_latest_revision_behind_delivery() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");

        let updated = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("new state should remain dirty while the first send is in flight");
        assert!(updated.revision > first.revision);
        assert!(runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "turn",
            claimed.revision
        ));

        let next = runtime
            .complete_telegram_command_progress_delivery(
                "thread",
                "turn",
                claimed.revision,
                "42".to_string(),
            )
            .expect("latest revision should be claimed after the first send");
        assert_eq!(next.revision, updated.revision);
        assert_eq!(next.message_id.as_deref(), Some("42"));
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    claimed.revision,
                    "stale".to_string(),
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_command_progress_rejects_old_delivery_after_turn_replacement() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "old-turn");
        let first = runtime
            .upsert_telegram_command_progress(
                "thread",
                "old-turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "old-turn")
            .expect("initial progress should be claimed");
        assert_eq!(claimed.revision, first.revision);

        runtime.mark_turn_completed("thread", Some("old-turn"));
        runtime.mark_turn_started("thread", "new-turn");
        assert!(!runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "old-turn",
            claimed.revision
        ));
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "old-turn",
                    claimed.revision,
                    "stale".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_command_progress_cleans_terminal_state_after_final_delivery() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");
        let final_snapshot = runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("terminal progress should be dirty");
        assert!(final_snapshot.completed);
        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert!(
            runtime
                .telegram_command_progress_by_thread
                .contains_key("thread")
        );

        let queued = runtime
            .complete_telegram_command_progress_delivery(
                "thread",
                "turn",
                first.revision,
                "42".to_string(),
            )
            .expect("terminal revision should follow the initial delivery");
        assert!(queued.completed);
        assert_eq!(queued.message_id.as_deref(), Some("42"));
        assert!(
            runtime
                .telegram_command_progress_by_thread
                .contains_key("thread")
        );
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    queued.revision,
                    "42".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_command_progress_keeps_terminal_failure_retryable_before_cleanup() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");
        runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("terminal progress should be queued");

        assert!(runtime.fail_telegram_command_progress_delivery("thread", "turn", first.revision));
        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        let retry = runtime
            .retry_telegram_command_progress_delivery("thread", "turn")
            .expect("terminal progress should remain retryable");
        assert!(retry.completed);
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    retry.revision,
                    "43".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_command_progress_can_reclaim_retryable_active_failure() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");

        assert!(!runtime.fail_telegram_command_progress_delivery("thread", "turn", first.revision));
        let retry = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("retryable API failures should be able to reclaim the snapshot");
        assert_eq!(retry.revision, first.revision);
    }

    #[test]
    fn telegram_command_progress_ignores_duplicate_detail_events() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning", 0, "Review");
        let reasoning = runtime
            .complete_telegram_reasoning("thread", "turn", "reasoning", None)
            .expect("reasoning should be flushed");
        assert!(
            runtime
                .complete_telegram_reasoning(
                    "thread",
                    "turn",
                    "reasoning",
                    Some("Review".to_string()),
                )
                .is_none()
        );

        let plan = vec![TelegramPlanStep {
            step: "Inspect".to_string(),
            status: TelegramPlanStepStatus::InProgress,
        }];
        let planned = runtime
            .update_telegram_plan("thread", "turn", None, plan.clone(), true)
            .expect("plan should be dirty");
        assert!(planned.revision > reasoning.revision);
        assert!(
            runtime
                .update_telegram_plan("thread", "turn", None, plan, true)
                .is_none()
        );

        let diff = TelegramDiffSummary {
            file_count: 1,
            additions: 2,
            deletions: 1,
            paths: vec!["src/lib.rs".to_string()],
            omitted_paths: 0,
        };
        let diff_snapshot = runtime
            .update_telegram_diff("thread", "turn", Some(diff.clone()), true)
            .expect("diff should be dirty");
        assert!(diff_snapshot.revision > planned.revision);
        assert!(
            runtime
                .update_telegram_diff("thread", "turn", Some(diff), true)
                .is_none()
        );
    }

    #[test]
    fn telegram_command_progress_final_flush_and_turn_cleanup() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");
        let started = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn-1",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                false,
            )
            .expect("started command");
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn-1",
            started.revision,
            "42".to_string(),
        );

        let final_snapshot = runtime
            .finish_telegram_command_progress("thread", "turn-1")
            .expect("final progress update");
        assert!(final_snapshot.completed);
        assert_eq!(final_snapshot.message_id.as_deref(), Some("42"));
        assert_eq!(
            final_snapshot.entries[0].status,
            TelegramCommandProgressStatus::Interrupted
        );
        assert!(
            runtime
                .finish_telegram_command_progress("thread", "turn-2")
                .is_none()
        );

        runtime.mark_turn_completed("thread", Some("turn-1"));
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_turn_details_share_the_command_progress_message() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning-1", 0, "first");
        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning-1", 0, " second");
        assert!(
            runtime
                .complete_telegram_reasoning("thread", "turn", "reasoning-1", None)
                .is_some()
        );
        let plan_snapshot = runtime
            .update_telegram_plan(
                "thread",
                "turn",
                Some("Implement and verify".to_string()),
                vec![TelegramPlanStep {
                    step: "Implement".to_string(),
                    status: TelegramPlanStepStatus::InProgress,
                }],
                true,
            )
            .expect("plan should create the aggregate");
        assert_eq!(
            plan_snapshot.reasoning_summary.as_deref(),
            Some("first second")
        );
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            plan_snapshot.revision,
            "99".to_string(),
        );

        let diff = TelegramDiffSummary {
            file_count: 2,
            additions: 3,
            deletions: 1,
            paths: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            omitted_paths: 0,
        };
        assert!(
            runtime
                .update_telegram_diff("thread", "turn", Some(diff.clone()), false)
                .is_none()
        );
        let file_snapshot = runtime
            .complete_telegram_file_change("thread", "turn", None)
            .expect("file change should flush cached diff");
        assert_eq!(file_snapshot.message_id.as_deref(), Some("99"));
        assert_eq!(file_snapshot.diff_summary, Some(diff));

        let duplicate_revision = file_snapshot.revision;
        assert!(
            runtime
                .update_telegram_diff("thread", "turn", file_snapshot.diff_summary.clone(), false,)
                .is_none()
        );
        let final_snapshot = runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("details-only turn should flush at completion");
        assert!(final_snapshot.completed);
        assert!(final_snapshot.revision > duplicate_revision);
    }

    #[test]
    fn telegram_turn_details_ignore_stale_turns_and_empty_diff() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "current");
        assert!(
            runtime
                .update_telegram_plan("thread", "old", None, Vec::new(), true,)
                .is_none()
        );
        assert!(
            runtime
                .update_telegram_diff("thread", "current", None, true)
                .is_none()
        );
        assert!(
            runtime
                .complete_telegram_file_change(
                    "thread",
                    "old",
                    Some(TelegramDiffSummary {
                        file_count: 1,
                        additions: 1,
                        deletions: 0,
                        paths: vec!["old.rs".to_string()],
                        omitted_paths: 0,
                    }),
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_collab_progress_reuses_message_and_merges_by_agent() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let started = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-secret",
                    Some("api_review"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("first agent should create progress");
        assert_eq!(started.entries.len(), 1);
        assert_eq!(started.message_id, None);
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    started.revision,
                    "88".to_string(),
                )
                .is_none()
        );

        let completed = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-secret",
                    None,
                    Some(TelegramCollabProgressStatus::Succeeded),
                    43_000,
                )],
            )
            .expect("agent completion should edit progress");
        assert_eq!(completed.entries.len(), 1);
        assert_eq!(completed.message_id.as_deref(), Some("88"));
        assert_eq!(
            completed.entries[0].status,
            TelegramCollabProgressStatus::Succeeded
        );
        assert_eq!(completed.entries[0].updated_at_ms, 43_000);
    }

    #[test]
    fn telegram_collab_progress_serializes_updates_and_rejects_stale_ack() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("review"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("first update should claim the delivery driver");
        assert!(
            runtime
                .apply_telegram_collab_progress_updates(
                    "thread",
                    "turn",
                    vec![collab_progress_update(
                        "agent-a",
                        None,
                        Some(TelegramCollabProgressStatus::Succeeded),
                        2_000,
                    )],
                )
                .is_none()
        );
        assert!(
            runtime
                .finish_telegram_collab_progress("thread", "turn")
                .is_none()
        );

        let final_snapshot = runtime
            .complete_telegram_collab_progress_delivery(
                "thread",
                "turn",
                first.revision,
                "100".to_string(),
            )
            .expect("queued terminal state should follow the first delivery");
        assert!(final_snapshot.completed);
        assert_eq!(final_snapshot.message_id.as_deref(), Some("100"));
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    first.revision,
                    "stale".to_string(),
                )
                .is_none()
        );
        assert_eq!(
            runtime
                .telegram_collab_progress_by_thread
                .get("thread")
                .and_then(|progress| progress.message_id.as_deref()),
            Some("100")
        );

        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert!(
            runtime
                .telegram_collab_progress_by_thread
                .contains_key("thread")
        );
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    final_snapshot.revision,
                    "100".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_collab_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_collab_progress_retries_failed_terminal_delivery_after_turn_cleanup() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        let first = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("review"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("first update should start delivery");
        assert!(
            runtime
                .finish_telegram_collab_progress("thread", "turn")
                .is_none()
        );
        assert!(runtime.mark_turn_completed("thread", Some("turn")));

        assert!(runtime.fail_telegram_collab_progress_delivery("thread", "turn", first.revision,));
        let retry = runtime
            .retry_telegram_collab_progress_delivery("thread", "turn")
            .expect("terminal state should remain retryable");
        assert!(retry.completed);
        assert_eq!(
            retry.entries[0].status,
            TelegramCollabProgressStatus::Interrupted
        );
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    retry.revision,
                    "101".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_collab_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_collab_progress_nonterminal_failure_releases_the_driver() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        let first = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("review"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("first update should start delivery");
        assert!(!runtime.fail_telegram_collab_progress_delivery("thread", "turn", first.revision,));

        let completed = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    None,
                    Some(TelegramCollabProgressStatus::Succeeded),
                    2_000,
                )],
            )
            .expect("a later update should restart delivery");
        assert_eq!(
            completed.entries[0].status,
            TelegramCollabProgressStatus::Succeeded
        );
    }

    #[test]
    fn telegram_collab_progress_ignores_updates_older_than_the_entry() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        let initial = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("initial_name"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("agent should create progress");
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    initial.revision,
                    "90".to_string(),
                )
                .is_none()
        );

        let mut newest = collab_progress_update(
            "agent-a",
            Some("current_name"),
            Some(TelegramCollabProgressStatus::Succeeded),
            3_000,
        );
        newest.detail = Some("current detail".to_string());
        let newest = runtime
            .apply_telegram_collab_progress_updates("thread", "turn", vec![newest])
            .expect("newer event should update progress");

        let mut stale = collab_progress_update(
            "agent-a",
            Some("stale_name"),
            Some(TelegramCollabProgressStatus::Failed),
            2_000,
        );
        stale.detail = Some("stale detail".to_string());
        assert!(
            runtime
                .apply_telegram_collab_progress_updates("thread", "turn", vec![stale])
                .is_none()
        );

        let retained = runtime
            .telegram_collab_progress_by_thread
            .get("thread")
            .expect("collaboration progress should remain");
        assert_eq!(retained.revision, newest.revision);
        assert_eq!(retained.entries, newest.entries);
        assert_eq!(retained.entries[0].name, "current_name");
        assert_eq!(
            retained.entries[0].status,
            TelegramCollabProgressStatus::Succeeded
        );
        assert_eq!(
            retained.entries[0].detail.as_deref(),
            Some("current detail")
        );
        assert_eq!(retained.entries[0].updated_at_ms, 3_000);
    }

    #[test]
    fn telegram_collab_progress_restart_requires_a_current_event() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        let initial = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("review"),
                    Some(TelegramCollabProgressStatus::Succeeded),
                    3_000,
                )],
            )
            .expect("agent should create terminal progress");
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    initial.revision,
                    "91".to_string(),
                )
                .is_none()
        );

        let mut stale_restart = collab_progress_update(
            "agent-a",
            None,
            Some(TelegramCollabProgressStatus::Running),
            2_000,
        );
        stale_restart.restart = true;
        assert!(
            runtime
                .apply_telegram_collab_progress_updates("thread", "turn", vec![stale_restart])
                .is_none()
        );

        let mut current_restart = collab_progress_update(
            "agent-a",
            None,
            Some(TelegramCollabProgressStatus::Running),
            4_000,
        );
        current_restart.restart = true;
        let restarted = runtime
            .apply_telegram_collab_progress_updates("thread", "turn", vec![current_restart])
            .expect("newer restart should reopen the agent");
        assert_eq!(
            restarted.entries[0].status,
            TelegramCollabProgressStatus::Running
        );
        assert_eq!(restarted.entries[0].started_at_ms, 4_000);
        assert_eq!(restarted.entries[0].updated_at_ms, 4_000);
    }

    #[test]
    fn telegram_collab_progress_success_and_restart_clear_stale_detail() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        let mut started = collab_progress_update(
            "agent-a",
            Some("review"),
            Some(TelegramCollabProgressStatus::Running),
            1_000,
        );
        started.detail = Some("temporary error".to_string());
        let initial = runtime
            .apply_telegram_collab_progress_updates("thread", "turn", vec![started])
            .expect("agent should create progress");
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    initial.revision,
                    "92".to_string(),
                )
                .is_none()
        );

        let succeeded = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    None,
                    Some(TelegramCollabProgressStatus::Succeeded),
                    2_000,
                )],
            )
            .expect("success should clear the old detail");
        assert_eq!(succeeded.entries[0].detail, None);
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    succeeded.revision,
                    "92".to_string(),
                )
                .is_none()
        );

        let mut failed = collab_progress_update(
            "agent-a",
            None,
            Some(TelegramCollabProgressStatus::Failed),
            3_000,
        );
        failed.detail = Some("retry failed".to_string());
        let failed = runtime
            .apply_telegram_collab_progress_updates("thread", "turn", vec![failed])
            .expect("failure should attach its detail");
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    failed.revision,
                    "92".to_string(),
                )
                .is_none()
        );

        let mut restarted = collab_progress_update(
            "agent-a",
            None,
            Some(TelegramCollabProgressStatus::Running),
            4_000,
        );
        restarted.restart = true;
        let restarted = runtime
            .apply_telegram_collab_progress_updates("thread", "turn", vec![restarted])
            .expect("restart should clear the old detail");
        assert_eq!(
            restarted.entries[0].status,
            TelegramCollabProgressStatus::Running
        );
        assert_eq!(restarted.entries[0].detail, None);
    }

    #[test]
    fn telegram_collab_progress_terminal_state_is_monotonic_and_finish_interrupts_running() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        let initial = runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![
                    collab_progress_update(
                        "agent-a",
                        Some("completed_agent"),
                        Some(TelegramCollabProgressStatus::Succeeded),
                        1_000,
                    ),
                    collab_progress_update(
                        "agent-b",
                        Some("running_agent"),
                        Some(TelegramCollabProgressStatus::Running),
                        2_000,
                    ),
                    collab_progress_update(
                        "agent-c",
                        Some("failed_agent"),
                        Some(TelegramCollabProgressStatus::Failed),
                        2_000,
                    ),
                ],
            )
            .expect("agents should create progress");
        assert!(
            runtime
                .complete_telegram_collab_progress_delivery(
                    "thread",
                    "turn",
                    initial.revision,
                    "93".to_string(),
                )
                .is_none()
        );

        assert!(
            runtime
                .apply_telegram_collab_progress_updates(
                    "thread",
                    "turn",
                    vec![
                        collab_progress_update(
                            "agent-a",
                            None,
                            Some(TelegramCollabProgressStatus::Running),
                            3_000,
                        ),
                        collab_progress_update(
                            "agent-c",
                            None,
                            Some(TelegramCollabProgressStatus::Succeeded),
                            3_000,
                        ),
                    ],
                )
                .is_none()
        );
        let final_snapshot = runtime
            .finish_telegram_collab_progress("thread", "turn")
            .expect("turn finish should edit collaboration progress");
        assert!(final_snapshot.completed);
        assert_eq!(
            final_snapshot.entries[0].status,
            TelegramCollabProgressStatus::Succeeded
        );
        assert_eq!(
            final_snapshot.entries[1].status,
            TelegramCollabProgressStatus::Interrupted
        );
        assert_eq!(
            final_snapshot.entries[2].status,
            TelegramCollabProgressStatus::Failed
        );
    }

    #[test]
    fn telegram_collab_progress_does_not_reappear_after_turn_cleanup() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .apply_telegram_collab_progress_updates(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("review"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("agent should create progress");
        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert!(runtime.telegram_collab_progress_by_thread.is_empty());
        assert!(
            runtime
                .apply_telegram_collab_progress_updates(
                    "thread",
                    "turn",
                    vec![collab_progress_update(
                        "agent-a",
                        None,
                        Some(TelegramCollabProgressStatus::Succeeded),
                        2_000,
                    )],
                )
                .is_none()
        );
        assert!(runtime.telegram_collab_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_retry_progress_counts_attempts_and_reuses_delivery() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .record_telegram_retry(
                "thread",
                "turn",
                Some("503 Service Unavailable".to_string()),
            )
            .expect("first retry should create progress");
        assert_eq!(first.retry_count, 1);
        assert_eq!(first.message_id, None);
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            first.revision,
            "77".to_string(),
        );

        let second = runtime
            .record_telegram_retry(
                "thread",
                "turn",
                Some("503 Service Unavailable".to_string()),
            )
            .expect("second retry should update progress");
        assert_eq!(second.retry_count, 2);
        assert_eq!(second.message_id.as_deref(), Some("77"));

        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            second.revision,
            "77".to_string(),
        );
        let final_snapshot = runtime
            .finish_telegram_command_progress_with_outcome("thread", "turn", true)
            .expect("retry-only progress must receive a terminal update");
        assert!(final_snapshot.completed);
        assert!(final_snapshot.failed);
        assert_eq!(final_snapshot.retry_count, 2);
        assert_eq!(final_snapshot.message_id.as_deref(), Some("77"));
    }

    #[test]
    fn telegram_retry_after_turn_completion_does_not_recreate_progress() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        assert!(
            runtime
                .record_telegram_retry("thread", "turn", None)
                .is_some()
        );

        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert!(runtime.telegram_command_progress_by_thread.is_empty());

        assert!(
            runtime
                .record_telegram_retry(
                    "thread",
                    "turn",
                    Some("503 Service Unavailable".to_string()),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn terminal_notice_deduplicates_equal_outcomes_and_prioritizes_failure() {
        let mut runtime = RuntimeState::default();

        assert!(runtime.claim_terminal_notice("turn-1", false));
        assert!(!runtime.claim_terminal_notice("turn-1", false));
        assert!(runtime.claim_terminal_notice("turn-1", true));
        assert!(!runtime.claim_terminal_notice("turn-1", true));
        assert!(!runtime.claim_terminal_notice("turn-1", false));

        assert!(runtime.claim_terminal_notice("turn-2", true));
        assert!(!runtime.claim_terminal_notice("turn-2", false));

        runtime.release_terminal_notice("turn-2");
        assert!(runtime.claim_terminal_notice("turn-2", false));
    }

    #[test]
    fn stale_turn_completion_does_not_clear_the_new_turn() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "old-turn");
        runtime.mark_turn_started("thread", "new-turn");

        assert!(!runtime.mark_turn_completed("thread", Some("old-turn")));
        assert_eq!(runtime.current_turn_id("thread"), Some("new-turn"));

        assert!(runtime.mark_turn_completed("thread", Some("new-turn")));
        assert_eq!(runtime.current_turn_id("thread"), None);
    }

    #[test]
    fn terminal_status_fallback_is_scoped_to_one_turn() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");

        assert!(runtime.register_terminal_status_fallback("thread", "turn-1"));
        assert!(runtime.terminal_status_fallback_matches("thread", "turn-1"));
        runtime.cancel_terminal_status_fallback("thread", "other-turn");
        assert!(runtime.terminal_status_fallback_matches("thread", "turn-1"));
        runtime.cancel_terminal_status_fallback("thread", "turn-1");
        assert!(!runtime.terminal_status_fallback_matches("thread", "turn-1"));

        assert!(runtime.register_terminal_status_fallback("thread", "turn-1"));
        runtime.mark_turn_started("thread", "turn-2");
        assert!(!runtime.terminal_status_fallback_matches("thread", "turn-1"));
        assert!(!runtime.register_terminal_status_fallback("thread", "turn-1"));
    }

    #[test]
    fn terminal_status_fallback_token_is_single_owner_across_drivers() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");

        let (server_token, inserted) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("server registration");
        assert!(inserted);
        let (same_token, inserted_again) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("duplicate registration should reuse latch");
        assert_eq!(same_token, server_token);
        assert!(!inserted_again);

        assert_eq!(
            runtime.start_terminal_status_fallback("thread", "turn-1"),
            Some(server_token)
        );
        assert_eq!(
            runtime.start_terminal_status_fallback("thread", "turn-1"),
            None
        );
        assert!(runtime.terminal_status_fallback_matches_token("thread", "turn-1", server_token));

        assert!(runtime.claim_terminal_status_fallback("thread", "turn-1", server_token));
        assert!(!runtime.claim_terminal_status_fallback("thread", "turn-1", server_token));
    }

    #[test]
    fn server_driver_starts_after_im_created_the_shared_latch() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");

        let token = runtime
            .start_terminal_status_fallback("thread", "turn-1")
            .expect("IM driver should create latch");
        let (same_token, should_start_server) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("server driver should reuse latch");
        assert_eq!(same_token, token);
        assert!(should_start_server);
        let (_, should_start_server_again) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("server driver registration");
        assert!(!should_start_server_again);
    }

    #[test]
    fn failed_command_progress_snapshot_is_marked_failed() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("command progress");

        let snapshot = runtime
            .finish_telegram_command_progress_with_outcome("thread", "turn", true)
            .expect("failed terminal snapshot");
        assert!(snapshot.failed);
        assert!(snapshot.completed);
    }

    #[test]
    fn starting_a_new_turn_discards_stale_command_progress() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn-1",
                command_progress_entry(
                    "old-item",
                    "old command",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("old command");

        runtime.mark_turn_started("thread", "turn-2");

        assert!(runtime.telegram_command_progress_by_thread.is_empty());
        let next = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn-2",
                command_progress_entry(
                    "new-item",
                    "new command",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("new command");
        assert_eq!(next.turn_id, "turn-2");
        assert_eq!(next.entries.len(), 1);
        assert_eq!(next.entries[0].command, "new command");
    }

    #[test]
    fn telegram_draft_coalesces_updates_while_the_sender_is_active() {
        let mut runtime = RuntimeState::default();

        let first = runtime
            .append_telegram_draft_delta("thread", "item", "hello")
            .expect("first draft update");
        let (content, finished, revision) = runtime
            .take_telegram_draft_snapshot("thread", "item", first, 1_000)
            .expect("first draft snapshot");
        assert_eq!(content, "hello");
        assert!(!finished);
        assert!(
            runtime
                .append_telegram_draft_delta("thread", "item", " world")
                .is_none()
        );
        assert_eq!(
            runtime.complete_telegram_draft_send("thread", "item", first, revision, true),
            TelegramDraftSendAction::Continue
        );
        assert_eq!(
            runtime.telegram_draft_send_delay_ms("thread", "item", first, 1_100, 300),
            Some(200)
        );
        let (content, finished, revision) = runtime
            .take_telegram_draft_snapshot("thread", "item", first, 1_300)
            .expect("coalesced draft snapshot");
        assert_eq!(content, "hello world");
        assert!(!finished);
        assert_eq!(
            runtime.complete_telegram_draft_send("thread", "item", first, revision, true),
            TelegramDraftSendAction::Continue
        );

        assert!(
            runtime
                .append_telegram_draft_delta("thread", "item", "!")
                .is_none()
        );
        assert_eq!(
            runtime.telegram_draft_send_delay_ms("thread", "item", first, 1_400, 300),
            Some(200)
        );
        assert_eq!(
            runtime
                .take_telegram_draft_snapshot("thread", "item", first, 1_600)
                .expect("next draft snapshot")
                .0,
            "hello world!"
        );
    }

    #[test]
    fn telegram_draft_heartbeat_reuses_the_latest_content() {
        let mut runtime = RuntimeState::default();
        let draft_id = runtime
            .append_telegram_draft_delta("thread", "item", "working")
            .expect("draft update");
        let (_, _, revision) = runtime
            .take_telegram_draft_snapshot("thread", "item", draft_id, 1_000)
            .expect("initial snapshot");
        assert_eq!(
            runtime.complete_telegram_draft_send("thread", "item", draft_id, revision, true),
            TelegramDraftSendAction::Continue
        );

        let (_, delay_ms) = runtime
            .telegram_draft_wait_for_update("thread", "item", draft_id, 1_100, 20_000)
            .expect("idle draft should wait for an update or heartbeat");
        assert_eq!(delay_ms, 19_900);
        assert!(runtime.mark_telegram_draft_heartbeat_due("thread", "item", draft_id));
        assert_eq!(
            runtime.telegram_draft_send_delay_ms("thread", "item", draft_id, 21_000, 300),
            Some(0)
        );
        assert_eq!(
            runtime
                .take_telegram_draft_snapshot("thread", "item", draft_id, 21_000)
                .expect("heartbeat snapshot")
                .0,
            "working"
        );
    }

    #[test]
    fn telegram_draft_is_removed_after_the_final_snapshot_is_delivered() {
        let mut runtime = RuntimeState::default();
        let draft_id = runtime
            .append_telegram_draft_delta("thread", "item", "hello")
            .expect("draft update");
        let (_, _, first_revision) = runtime
            .take_telegram_draft_snapshot("thread", "item", draft_id, 1_000)
            .expect("initial snapshot");
        assert_eq!(
            runtime.complete_telegram_draft_send("thread", "item", draft_id, first_revision, true,),
            TelegramDraftSendAction::Continue
        );

        let (finished_draft_id, should_start, _completed) = runtime
            .finish_telegram_draft("thread", "item", "final answer")
            .expect("final draft");
        assert_eq!(finished_draft_id, draft_id);
        assert!(!should_start);
        let (content, finished, final_revision) = runtime
            .take_telegram_draft_snapshot("thread", "item", draft_id, 1_300)
            .expect("final snapshot");
        assert_eq!(content, "final answer");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_draft_send("thread", "item", draft_id, final_revision, true,),
            TelegramDraftSendAction::Stop
        );
        assert!(
            runtime
                .finish_telegram_draft("thread", "item", "again")
                .is_none()
        );
    }

    #[tokio::test]
    async fn telegram_final_draft_notifies_the_persistent_message_waiter() {
        let mut runtime = RuntimeState::default();
        let draft_id = runtime
            .append_telegram_draft_delta("thread", "item", "partial")
            .expect("draft update");
        let (_, _, first_revision) = runtime
            .take_telegram_draft_snapshot("thread", "item", draft_id, 1_000)
            .expect("initial snapshot");
        assert_eq!(
            runtime.complete_telegram_draft_send("thread", "item", draft_id, first_revision, true),
            TelegramDraftSendAction::Continue
        );
        let (_, should_start, completed) = runtime
            .finish_telegram_draft("thread", "item", "final")
            .expect("final draft");
        assert!(!should_start);
        let (_, finished, final_revision) = runtime
            .take_telegram_draft_snapshot("thread", "item", draft_id, 1_300)
            .expect("final snapshot");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_draft_send("thread", "item", draft_id, final_revision, true),
            TelegramDraftSendAction::Stop
        );

        tokio::time::timeout(std::time::Duration::from_millis(50), completed.notified())
            .await
            .expect("final draft completion should wake the persistent message sender");
    }

    #[test]
    fn completing_a_turn_clears_all_of_its_telegram_drafts() {
        let mut runtime = RuntimeState::default();
        let first = runtime
            .append_telegram_draft_delta("thread", "item-1", "one")
            .expect("first draft");
        let second = runtime
            .append_telegram_draft_delta("thread", "item-2", "two")
            .expect("second draft");
        runtime
            .append_telegram_draft_delta("other-thread", "item", "other")
            .expect("other draft");

        runtime.mark_turn_completed("thread", None);

        assert!(
            runtime
                .take_telegram_draft_snapshot("thread", "item-1", first, 1_000)
                .is_none()
        );
        assert!(
            runtime
                .take_telegram_draft_snapshot("thread", "item-2", second, 1_000)
                .is_none()
        );
        assert_eq!(runtime.telegram_drafts_by_item.len(), 1);
    }

    #[test]
    fn approval_request_lifecycle_survives_replay_until_resolved() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:open_id:ou_test".to_string();

        assert!(runtime.push_approval(route.clone(), approval(7)));
        assert!(!runtime.push_approval(route.clone(), approval(7)));
        assert_eq!(
            runtime
                .pending_approvals_by_conversation
                .get(&route)
                .map(Vec::len),
            Some(1)
        );

        runtime.resolve_approval_request(&json!(7));
        assert!(runtime.push_approval(route, approval(7)));
    }

    #[test]
    fn approval_reply_targets_current_request() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:open_id:ou_test".to_string();

        assert!(runtime.push_approval(route.clone(), approval(1)));
        assert!(runtime.push_approval(route.clone(), approval(2)));

        let current = runtime
            .current_approval(&route)
            .expect("current approval should exist");
        assert_eq!(current.request_id, json!(1));

        let resolved = runtime
            .resolve_approval_request_with_context(&json!(1))
            .expect("current approval should resolve");
        assert_eq!(resolved.approval.request_id, json!(1));
        assert!(resolved.was_current);
        assert_eq!(
            resolved
                .next_current
                .expect("queued approval should become current")
                .request_id,
            json!(2)
        );

        let remaining = runtime
            .current_approval(&route)
            .expect("queued approval should remain until resolved");
        assert_eq!(remaining.request_id, json!(2));
    }

    #[test]
    fn approval_can_be_resolved_by_request_key_without_chat_key() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:open_id:ou_test".to_string();
        assert!(runtime.push_approval(route.clone(), approval(42)));

        let (found_route, pending) = runtime
            .approval_by_request_key_anywhere("number:42")
            .expect("approval should be found globally");
        assert_eq!(found_route, route);
        assert_eq!(pending.request_id, json!(42));
    }

    #[test]
    fn turn_origin_is_removed_when_turn_completes() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread-1", "turn-1");
        runtime.remember_turn_origin("turn-1", TurnOrigin::Feishu);

        assert_eq!(runtime.turn_origin("turn-1"), Some(TurnOrigin::Feishu));

        runtime.mark_turn_completed("thread-1", Some("turn-1"));

        assert_eq!(runtime.turn_origin("turn-1"), None);
    }

    #[test]
    fn turn_starting_blocks_parallel_starts_and_expires_old_messages() {
        let mut runtime = RuntimeState::default();
        assert!(runtime.try_mark_turn_starting("thread-1").is_ok());
        assert_eq!(
            runtime.try_mark_turn_starting("thread-1"),
            Err(ThreadTurnState::Starting)
        );

        let before_turn = 1;
        runtime.mark_turn_started("thread-1", "turn-1");
        assert_eq!(
            runtime.try_mark_turn_starting("thread-1"),
            Err(ThreadTurnState::Running("turn-1".to_string()))
        );

        runtime.mark_turn_completed("thread-1", Some("turn-1"));
        assert!(runtime.message_is_stale_for_latest_turn("thread-1", before_turn));
        assert!(runtime.try_mark_turn_starting("thread-1").is_ok());
    }

    #[test]
    fn route_from_conversation_key_preserves_platform() {
        let feishu =
            route_from_conversation_key("feishu:default:open_id:ou_test").expect("feishu route");
        assert_eq!(feishu.platform, ImPlatformKind::Feishu);
        assert_eq!(feishu.account_id, "default");
        assert_eq!(feishu.chat_id, "open_id:ou_test");
        assert!(feishu.remote_client_key.starts_with("im:feishu:"));

        let telegram =
            route_from_conversation_key("telegram:bot:chat:123").expect("telegram route");
        assert_eq!(telegram.platform, ImPlatformKind::Telegram);
        assert_eq!(telegram.account_id, "bot");
        assert_eq!(telegram.chat_id, "chat:123");
        assert!(telegram.remote_client_key.starts_with("im:telegram:"));

        let wecom =
            route_from_conversation_key("wecom:corp-bot:group:room-1").expect("wecom route");
        assert_eq!(wecom.platform, ImPlatformKind::Wecom);
        assert_eq!(wecom.account_id, "corp-bot");
        assert_eq!(wecom.chat_id, "group:room-1");
        assert!(wecom.remote_client_key.starts_with("im:wecom:"));

        assert!(route_from_conversation_key("slack:team:channel").is_none());
    }

    #[test]
    fn im_remote_client_key_is_deterministic_and_route_scoped() {
        let feishu_key = RouteTarget::deterministic_remote_client_key_for(
            ImPlatformKind::Feishu,
            "default",
            "chat-1",
        );
        assert_eq!(
            feishu_key,
            RouteTarget::deterministic_remote_client_key_for(
                ImPlatformKind::Feishu,
                "default",
                "chat-1"
            )
        );
        assert!(feishu_key.starts_with("im:feishu:"));
        assert_ne!(
            feishu_key,
            RouteTarget::deterministic_remote_client_key_for(
                ImPlatformKind::Wechat,
                "default",
                "chat-1"
            )
        );
        assert_ne!(
            feishu_key,
            RouteTarget::deterministic_remote_client_key_for(
                ImPlatformKind::Feishu,
                "default",
                "chat-2"
            )
        );
    }
}
