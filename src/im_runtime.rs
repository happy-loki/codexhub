use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{chain_log, im::feishu::FeishuStreamingCardState, types::ImPlatformKind};

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
    pub last_sent_text_by_route: HashMap<String, String>,
    pub route_by_thread: HashMap<String, RouteTarget>,
    pub last_route: Option<RouteTarget>,
    pub pending_approvals_by_conversation: HashMap<String, Vec<PendingApproval>>,
    pub pending_approval_request_keys: HashSet<String>,
    pub feishu_streaming_cards_by_item: HashMap<String, FeishuStreamingCardState>,
    pub thread_routing_requests: HashMap<String, ThreadRoutingRequestState>,
}

impl RuntimeState {
    pub fn start_bridge_generation(&mut self) -> u64 {
        self.bridge_generation = self.bridge_generation.saturating_add(1);
        self.feishu_streaming_cards_by_item.clear();
        self.bridge_generation
    }

    pub fn invalidate_bridge_generation(&mut self) {
        self.bridge_generation = self.bridge_generation.saturating_add(1);
        self.feishu_streaming_cards_by_item.clear();
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
            log_route_unbind("unbind_conversation", reason, thread_id, route);
        }
        entries
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect()
    }

    pub fn mark_turn_started(&mut self, thread_id: &str, turn_id: &str) {
        self.starting_turn_by_thread.remove(thread_id);
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

    pub fn turn_origin(&self, turn_id: &str) -> Option<TurnOrigin> {
        self.turn_origin_by_id.get(turn_id).copied()
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

    pub fn mark_turn_completed(&mut self, thread_id: &str, _turn_id: Option<&str>) {
        self.starting_turn_by_thread.remove(thread_id);
        let completed_turn_id = self.current_turn_by_thread.remove(thread_id);
        if let Some(turn_id) = _turn_id.or(completed_turn_id.as_deref()) {
            self.turn_origin_by_id.remove(turn_id);
        }
        self.turn_finished_at_by_thread
            .insert(thread_id.to_string(), crate::types::now_ms());
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

    use crate::types::ImPlatformKind;

    use super::{
        PendingApproval, RouteTarget, RuntimeState, ThreadTurnState, TurnOrigin,
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
