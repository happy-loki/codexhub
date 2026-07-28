use anyhow::Result;

use crate::{
    app_state::SharedState,
    im::{core::outbound::ImOutboundSender, wechat::flow::handle_text_inbound},
    im_runtime::TurnOrigin,
    types::InboundMessage,
};

use super::{adapter::WecomAdapter, api::WecomApi};

pub async fn handle_inbound(
    state: SharedState,
    api: WecomApi,
    outbound_tx: ImOutboundSender,
    message: InboundMessage,
) -> Result<()> {
    state
        .push_event(
            "info",
            "wecom_message",
            format!(
                "chat={} sender={} text_len={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count()
            ),
        )
        .await;
    let adapter = WecomAdapter::new(api);
    handle_text_inbound(
        state,
        outbound_tx,
        message,
        &adapter,
        TurnOrigin::Wecom,
        "wecom",
    )
    .await
}
