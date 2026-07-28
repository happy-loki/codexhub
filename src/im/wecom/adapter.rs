use std::path::Path;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;

use crate::{app_state::SharedState, im::core::text_adapter::TextChatAdapter};

use super::api::WecomApi;
use crate::{
    im::core::{
        i18n::ImText,
        thread::{ThreadCreateDefaults, create_options_for_field},
        thread_list::ThreadRoutingPage,
    },
    im_runtime::{PendingApproval, ThreadCreateDraftState, approval_request_fingerprint},
    types::{InboundAction, InboundCallbackKind, InboundMessage},
};

const WECOM_TEXT_CHUNK_CHARS: usize = 4000;

#[derive(Clone)]
pub struct WecomAdapter {
    api: WecomApi,
}

impl WecomAdapter {
    pub fn new(api: WecomApi) -> Self {
        Self { api }
    }

    pub async fn send_text(
        &self,
        _state: &SharedState,
        _account_id: &str,
        target: &str,
        text: &str,
    ) -> Result<String> {
        let chunks = text_chunks(text);
        let mut last_id = String::new();
        for chunk in chunks {
            last_id = self.api.send_markdown(target, &chunk).await?;
        }
        Ok(last_id)
    }

    pub async fn send_media(&self, target: &str, path: &Path) -> Result<String> {
        let media_type = if mime_guess::from_path(path)
            .first_raw()
            .is_some_and(|mime| mime.starts_with("image/"))
        {
            "image"
        } else {
            "file"
        };
        self.api
            .upload_and_send_media(target, media_type, path)
            .await
    }

    pub async fn send_approval_card(
        &self,
        target: &str,
        approval: &PendingApproval,
    ) -> Result<String> {
        let fingerprint = approval_request_fingerprint(&approval.request_key());
        let task_id = format!("approval_{fingerprint}");
        let buttons = approval
            .decisions
            .iter()
            .enumerate()
            .take(6)
            .map(|(index, decision)| {
                serde_json::json!({
                    "text": decision.label,
                    "style": if index == 0 { 1 } else { 2 },
                    "key": format!("approval:{fingerprint}:{}", index + 1)
                })
            })
            .collect::<Vec<_>>();
        let card = serde_json::json!({
            "card_type": "button_interaction",
            "source": { "desc": "CodexHub", "desc_color": 0 },
            "main_title": {
                "title": "Codex 审批请求",
                "desc": approval.request_kind
            },
            "sub_title_text": approval.summary,
            "button_list": buttons,
            "task_id": task_id
        });
        self.api.send_template_card(target, card).await
    }

    async fn send_thread_routing_choice_card_inner(
        &self,
        message: &InboundMessage,
        request_id: &str,
        text: ImText,
    ) -> Result<String> {
        self.reply_routing_card(message, thread_routing_choice_card(request_id, text))
            .await
    }

    async fn send_thread_routing_list_cards_inner(
        &self,
        message: &InboundMessage,
        page: &ThreadRoutingPage,
        text: ImText,
    ) -> Result<String> {
        self.reply_routing_card(message, thread_routing_list_card(page, text))
            .await
    }

    async fn send_thread_create_settings_card_inner(
        &self,
        message: &InboundMessage,
        request_id: &str,
        defaults: &ThreadCreateDefaults,
        draft: &ThreadCreateDraftState,
        text: ImText,
    ) -> Result<String> {
        self.reply_routing_card(
            message,
            thread_create_settings_card(request_id, defaults, draft, text)?,
        )
        .await
    }

    async fn reply_routing_card(
        &self,
        message: &InboundMessage,
        card: serde_json::Value,
    ) -> Result<String> {
        let callback_req_id = message
            .callback_req_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .context("WeCom routing card requires a callback req_id")?;
        match message.callback_kind {
            Some(InboundCallbackKind::Welcome) => {
                self.api
                    .reply_welcome_template_card(callback_req_id, card)
                    .await
            }
            Some(InboundCallbackKind::Message) => {
                self.api.reply_template_card(callback_req_id, card).await
            }
            Some(InboundCallbackKind::CardEvent) => {
                let context = message
                    .card_message_id
                    .as_deref()
                    .context("WeCom card event context is missing")?;
                let (_, task_id) = context
                    .split_once('|')
                    .context("WeCom card event context is invalid")?;
                if task_id.is_empty() {
                    bail!("WeCom card event task_id is missing");
                }
                let mut card = card;
                card["task_id"] = serde_json::json!(task_id);
                self.api
                    .update_template_card(callback_req_id, card, Some(&message.sender_id))
                    .await
            }
            None => bail!("WeCom routing card callback kind is missing"),
        }
    }

    async fn acknowledge_thread_routing_action_inner(
        &self,
        message: &InboundMessage,
        action: &InboundAction,
        text: ImText,
    ) -> Result<()> {
        let Some(context) = message.card_message_id.as_deref() else {
            return Ok(());
        };
        let Some((callback_req_id, task_id)) = context.split_once('|') else {
            return Ok(());
        };
        if callback_req_id.is_empty() || task_id.is_empty() {
            return Ok(());
        }
        let (title, body) = match action {
            InboundAction::ThreadRouteChoice { action, .. } if action == "create_new" => {
                return Ok(());
            }
            InboundAction::ThreadRouteChoice { action, .. }
                if action == "resume_history" || action == "back" =>
            {
                return Ok(());
            }
            InboundAction::ThreadRouteResumeIndex { .. } => (
                text.subscribing_session_title(),
                text.restore_history_description_feishu(),
            ),
            InboundAction::ThreadRouteListPage { .. } => return Ok(()),
            _ => return Ok(()),
        };
        self.api
            .update_template_card(
                callback_req_id,
                resolved_routing_card(task_id, title, body),
                Some(&message.sender_id),
            )
            .await?;
        Ok(())
    }
}

#[async_trait]
impl TextChatAdapter for WecomAdapter {
    async fn send_text(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        text: &str,
    ) -> Result<String> {
        WecomAdapter::send_text(self, state, account_id, target, text).await
    }

    async fn send_thread_routing_choice_card(
        &self,
        _state: &SharedState,
        message: &InboundMessage,
        request_id: &str,
        text: ImText,
    ) -> Result<Option<String>> {
        self.send_thread_routing_choice_card_inner(message, request_id, text)
            .await
            .map(Some)
    }

    async fn send_thread_routing_list_cards(
        &self,
        _state: &SharedState,
        message: &InboundMessage,
        page: &ThreadRoutingPage,
        text: ImText,
    ) -> Result<Option<String>> {
        self.send_thread_routing_list_cards_inner(message, page, text)
            .await
            .map(Some)
    }

    async fn send_thread_create_settings_card(
        &self,
        _state: &SharedState,
        message: &InboundMessage,
        request_id: &str,
        defaults: &ThreadCreateDefaults,
        draft: &ThreadCreateDraftState,
        text: ImText,
    ) -> Result<Option<String>> {
        self.send_thread_create_settings_card_inner(message, request_id, defaults, draft, text)
            .await
            .map(Some)
    }

    fn thread_routing_page_size(&self) -> u32 {
        8
    }

    async fn acknowledge_thread_routing_action(
        &self,
        message: &InboundMessage,
        action: &InboundAction,
        text: ImText,
    ) -> Result<()> {
        self.acknowledge_thread_routing_action_inner(message, action, text)
            .await
    }
}

fn thread_routing_choice_card(request_id: &str, text: ImText) -> serde_json::Value {
    serde_json::json!({
        "card_type": "button_interaction",
        "source": { "desc": "CodexHub", "desc_color": 0 },
        "main_title": {
            "title": text.create_choice_title_feishu(),
            "desc": text.thread_list_title_feishu()
        },
        "sub_title_text": text.create_choice_body_wecom(),
        "button_list": [
            {
                "text": text.create_new_session_button(),
                "style": 1,
                "key": format!("thread-choice:{request_id}:create_new")
            },
            {
                "text": text.restore_history_button(),
                "style": 2,
                "key": format!("thread-choice:{request_id}:resume_history")
            }
        ],
        "task_id": routing_task_id(request_id, "choice")
    })
}

fn thread_routing_list_card(page: &ThreadRoutingPage, text: ImText) -> serde_json::Value {
    let option_list = page
        .entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.thread_id,
                "text": truncate_card_text(&entry.title, 20)
            })
        })
        .collect::<Vec<_>>();
    let button_list = vec![serde_json::json!({
        "text": text.restore_history_button(),
        "style": 1,
        "key": format!("thread-select:{}", page.request_id)
    })];
    let mut action_list = Vec::new();
    if page.next_cursor.is_some() {
        action_list.push(serde_json::json!({
            "text": text.next_page_button(),
            "key": format!("thread-page:{}:next", page.request_id)
        }));
    }
    if page.page > 1 {
        action_list.push(serde_json::json!({
            "text": text.previous_page_button(),
            "key": format!("thread-page:{}:prev", page.request_id)
        }));
    }
    action_list.push(serde_json::json!({
        "text": text.back_button(),
        "key": format!("thread-choice:{}:back", page.request_id)
    }));
    let body = if page.entries.is_empty() {
        text.no_restorable_history_workspace()
    } else {
        text.create_choice_tip_feishu()
    };
    serde_json::json!({
        "card_type": "button_interaction",
        "source": { "desc": "CodexHub", "desc_color": 0 },
        "main_title": {
            "title": text.thread_list_title_feishu(),
            "desc": text.page_label(page.page)
        },
        "sub_title_text": body,
        "action_menu": {
            "desc": text.more_actions(),
            "action_list": action_list
        },
        "button_selection": {
            "question_key": "thread_session",
            "title": text.select_session_action(),
            "option_list": option_list
        },
        "button_list": button_list,
        "task_id": routing_task_id(&page.request_id, &format!("list_{}", page.page))
    })
}

fn thread_create_settings_card(
    request_id: &str,
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
    text: ImText,
) -> Result<serde_json::Value> {
    let model_options = create_options_for_field(defaults, draft, "model", text)?.2;
    let effort_options = create_options_for_field(defaults, draft, "effort", text)?.2;
    let permission_selected = draft
        .permission
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(defaults.permission.as_deref())
        .unwrap_or("workspace_user");
    let (submit_text, submit_key) = if draft.cwd_custom.is_some() {
        (
            text.confirm_create_button(),
            format!("thread-create-submit:{request_id}:{permission_selected}"),
        )
    } else {
        (
            text.custom_cwd_label(),
            format!("thread-create-custom-cwd:{request_id}:{permission_selected}"),
        )
    };
    let cwd_display = draft
        .cwd_custom
        .as_deref()
        .unwrap_or(text.waiting_custom_cwd());
    Ok(serde_json::json!({
        "card_type": "multiple_interaction",
        "source": { "desc": "CodexHub", "desc_color": 0 },
        "main_title": {
            "title": text.create_settings_card_title(),
            "desc": defaults.remote_name.as_deref().unwrap_or(text.not_connected())
        },
        "sub_title_text": text.create_settings_card_intro(),
        "select_list": [
            {
                "question_key": "cwd_display",
                "title": text.cwd_section(),
                "selected_id": "cwd_display",
                "option_list": [{
                    "id": "cwd_display",
                    "text": truncate_card_text(cwd_display, 20)
                }]
            },
            create_selection(
                "model",
                text.model_section(),
                &model_options,
                draft.model.as_deref().unwrap_or("__default__")
            ),
            create_selection(
                "effort",
                text.effort_section(),
                &effort_options,
                draft.effort.as_deref().unwrap_or("__default__")
            )
        ],
        "submit_button": {
            "text": submit_text,
            "key": submit_key
        },
        "task_id": routing_task_id(request_id, "create")
    }))
}

fn create_selection(
    question_key: &str,
    title: &str,
    options: &[(String, crate::im::core::thread::ThreadCreateOption)],
    selected_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "question_key": question_key,
        "title": title,
        "selected_id": selected_id,
        "option_list": options
            .iter()
            .take(10)
            .map(|(value, option)| serde_json::json!({
                "id": value,
                "text": truncate_card_text(&option.label, 20)
            }))
            .collect::<Vec<_>>()
    })
}

fn resolved_routing_card(task_id: &str, title: &str, body: &str) -> serde_json::Value {
    serde_json::json!({
        "card_type": "text_notice",
        "source": { "desc": "CodexHub", "desc_color": 0 },
        "main_title": { "title": title },
        "sub_title_text": body,
        "card_action": {
            "type": 1,
            "url": "https://work.weixin.qq.com"
        },
        "task_id": task_id
    })
}

fn routing_task_id(request_id: &str, suffix: &str) -> String {
    format!(
        "route_{}_{}_{}",
        request_id.replace('-', "_"),
        suffix,
        uuid::Uuid::new_v4().simple()
    )
}

fn truncate_card_text(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn text_chunks(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let chars = text.chars().collect::<Vec<_>>();
    chars
        .chunks(WECOM_TEXT_CHUNK_CHARS)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im::core::{i18n::ImText, thread::ThreadListEntry, thread_list::ThreadRoutingPage};

    #[test]
    fn builds_initial_thread_choice_card() {
        let card = thread_routing_choice_card("thread-route-42", ImText::zh_cn());
        assert_eq!(card["card_type"], "button_interaction");
        assert_eq!(
            card.pointer("/button_list/0/text")
                .and_then(|value| value.as_str()),
            Some("创建新会话")
        );
        assert_eq!(
            card.pointer("/button_list/0/key")
                .and_then(|value| value.as_str()),
            Some("thread-choice:thread-route-42:create_new")
        );
        assert_eq!(
            card.pointer("/button_list/1/key")
                .and_then(|value| value.as_str()),
            Some("thread-choice:thread-route-42:resume_history")
        );
    }

    #[test]
    fn builds_history_selection_card_with_navigation() {
        let entries = (0..8)
            .map(|index| ThreadListEntry {
                thread_id: format!("thread-{index}"),
                title: format!("Session {index}"),
                state: String::new(),
                cwd: Some(format!("C:/work/{index}")),
            })
            .collect::<Vec<_>>();
        let page = ThreadRoutingPage {
            request_id: "thread-route-7".to_string(),
            page: 2,
            page_cursors: vec![None, Some("cursor-2".to_string())],
            thread_ids_by_page: vec![Vec::new(), Vec::new()],
            entries,
            next_cursor: Some("cursor-3".to_string()),
            model_provider_filter: Some("openai".to_string()),
        };
        let card = thread_routing_list_card(&page, ImText::zh_cn());

        assert_eq!(card["card_type"], "button_interaction");
        assert_eq!(
            card["button_selection"]["option_list"]
                .as_array()
                .unwrap()
                .len(),
            8
        );
        assert_eq!(card["button_list"].as_array().unwrap().len(), 1);
        assert_eq!(
            card["action_menu"]["action_list"].as_array().unwrap().len(),
            3
        );
        assert_eq!(
            card.pointer("/button_list/0/key")
                .and_then(|value| value.as_str()),
            Some("thread-select:thread-route-7")
        );
        let nav_keys = card["action_menu"]["action_list"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|action| action["key"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            nav_keys,
            vec![
                "thread-page:thread-route-7:next",
                "thread-page:thread-route-7:prev",
                "thread-choice:thread-route-7:back"
            ]
        );
    }

    #[test]
    fn builds_thread_create_settings_card_like_a_form() {
        let defaults = ThreadCreateDefaults {
            remote_name: Some("desktop".to_string()),
            model: Some("gpt-5.6-sol".to_string()),
            permission: Some("full_access".to_string()),
            projects: vec!["D:/work/codexhub".to_string()],
            models: vec![crate::im::core::thread::ThreadModelChoice {
                label: "GPT 5.6 Sol".to_string(),
                value: "gpt-5.6-sol".to_string(),
            }],
            efforts: vec!["high".to_string(), "xhigh".to_string()],
            ..Default::default()
        };
        let card = thread_create_settings_card(
            "thread-route-21",
            &defaults,
            &ThreadCreateDraftState::default(),
            ImText::zh_cn(),
        )
        .expect("create settings card");

        assert_eq!(card["card_type"], "multiple_interaction");
        assert_eq!(card["select_list"].as_array().unwrap().len(), 3);
        assert_eq!(
            card.pointer("/select_list/0/question_key"),
            Some(&serde_json::json!("cwd_display"))
        );
        assert_eq!(
            card.pointer("/select_list/1/question_key"),
            Some(&serde_json::json!("model"))
        );
        assert_eq!(
            card.pointer("/select_list/2/question_key"),
            Some(&serde_json::json!("effort"))
        );
        assert_eq!(
            card.pointer("/select_list/0/option_list/0/text")
                .and_then(|value| value.as_str()),
            Some("等待输入自定义目录")
        );
        assert_eq!(
            card.pointer("/submit_button/key")
                .and_then(|value| value.as_str()),
            Some("thread-create-custom-cwd:thread-route-21:full_access")
        );
        assert!(card.get("action_menu").is_none());

        let configured_card = thread_create_settings_card(
            "thread-route-21",
            &defaults,
            &ThreadCreateDraftState {
                cwd_custom: Some("D:/new/project".to_string()),
                ..Default::default()
            },
            ImText::zh_cn(),
        )
        .expect("configured create settings card");
        assert_eq!(
            configured_card
                .pointer("/submit_button/key")
                .and_then(|value| value.as_str()),
            Some("thread-create-submit:thread-route-21:full_access")
        );
        assert_eq!(
            configured_card
                .pointer("/select_list/0/option_list/0/text")
                .and_then(|value| value.as_str()),
            Some("D:/new/project")
        );
    }
}
