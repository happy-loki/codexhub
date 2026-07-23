use anyhow::Result;
use async_trait::async_trait;

use crate::{
    app_state::SharedState,
    im::core::{i18n::ImText, thread::ThreadCreateDefaults, thread_list::ThreadRoutingPage},
    im_runtime::ThreadCreateDraftState,
    types::{InboundAction, InboundMessage},
};

#[async_trait]
pub(crate) trait TextChatAdapter: Send + Sync {
    async fn send_text(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        text: &str,
    ) -> Result<String>;

    async fn send_thread_routing_choice_card(
        &self,
        _state: &SharedState,
        _message: &InboundMessage,
        _request_id: &str,
        _text: ImText,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    async fn send_thread_routing_list_cards(
        &self,
        _state: &SharedState,
        _message: &InboundMessage,
        _page: &ThreadRoutingPage,
        _text: ImText,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    async fn send_thread_create_settings_card(
        &self,
        _state: &SharedState,
        _message: &InboundMessage,
        _request_id: &str,
        _defaults: &ThreadCreateDefaults,
        _draft: &ThreadCreateDraftState,
        _text: ImText,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    fn thread_routing_page_size(&self) -> u32 {
        8
    }

    async fn acknowledge_thread_routing_action(
        &self,
        _message: &InboundMessage,
        _action: &InboundAction,
        _text: ImText,
    ) -> Result<()> {
        Ok(())
    }
}
