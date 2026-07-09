use std::cell::RefCell;
use std::rc::Rc;

use wxdragon::prelude::*;
use wxdragon::widgets::dataview::CustomDataViewVirtualListModel;

use crate::ai_gateway::config::{
    AiGatewayConfig, ProviderConfig, ProviderType, provider_display_base_url,
};

use super::UiHandles;
use super::api::ApiClient;
use super::widgets::{ProviderLogoKind, provider_logo_bitmap};

pub(super) type AiGwProviderRows = Rc<RefCell<Vec<AiGwProviderRow>>>;
pub(super) type AiGwProviderModel = Rc<RefCell<CustomDataViewVirtualListModel>>;
pub(super) type PendingAiGwChannelToggle = Rc<RefCell<Option<AiGwChannelToggle>>>;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct AiGwProviderRow {
    pub(super) enabled: bool,
    pub(super) name: String,
    pub(super) provider_type: ProviderType,
    pub(super) compatibility: Option<String>,
    pub(super) base_url: String,
    pub(super) models_url: Option<String>,
    pub(super) weight: u32,
}

#[derive(Clone)]
pub(super) struct AiGwChannelToggle {
    pub(super) row: usize,
    pub(super) name: String,
    pub(super) enabled: bool,
    pub(super) previous_enabled: bool,
}

pub(super) enum AiGwActionResult {
    Save(Result<(), String>),
    Delete(Result<(), String>),
    ChannelToggle {
        row: usize,
        previous_enabled: bool,
        result: Result<(), String>,
    },
    FilterImageGeneration(Result<bool, String>),
    RequestLogging(Result<bool, String>),
    RequestLogDetails(Result<bool, String>),
}

pub(super) fn save_ai_gw_provider(
    api: &ApiClient,
    mut provider: ProviderConfig,
) -> Result<(), String> {
    provider.base_url = provider_display_base_url(&provider.base_url);
    let mut config = api.get_app_config()?;
    if let Some(existing) = config
        .ai_gateway
        .providers
        .iter_mut()
        .find(|p| p.name == provider.name)
    {
        *existing = provider;
    } else {
        config.ai_gateway.providers.push(provider);
    }
    api.save_app_config(&config)?;
    Ok(())
}

pub(super) fn delete_ai_gw_provider(api: &ApiClient, name: &str) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.providers.retain(|p| p.name != name);
    api.save_app_config(&config)?;
    Ok(())
}

pub(super) fn set_ai_gw_provider_enabled(
    api: &ApiClient,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    let Some(provider) = config
        .ai_gateway
        .providers
        .iter_mut()
        .find(|provider| provider.name == name)
    else {
        return Err(format!("AI Gateway channel not found: {name}"));
    };
    provider.enabled = enabled;
    api.save_app_config(&config)?;
    Ok(())
}

pub(super) fn set_filter_image_generation_tool(
    api: &ApiClient,
    enabled: bool,
) -> Result<bool, String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.filter_image_generation_tool = enabled;
    api.save_app_config(&config)?;
    Ok(enabled)
}

pub(super) fn refresh_ai_gw_filter_image_generation(handles: &UiHandles, enabled: bool) {
    if !handles.ai_gw_filter_image_generation.has_focus() {
        handles.ai_gw_filter_image_generation.set_value(enabled);
    }
}

pub(super) fn set_request_logging_enabled(api: &ApiClient, enabled: bool) -> Result<bool, String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.request_logging_enabled = enabled;
    if !enabled {
        config.ai_gateway.request_log_details_enabled = false;
    }
    api.save_app_config(&config)?;
    Ok(enabled)
}

pub(super) fn set_request_log_details_enabled(
    api: &ApiClient,
    enabled: bool,
) -> Result<bool, String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.request_log_details_enabled =
        enabled && config.ai_gateway.request_logging_enabled;
    api.save_app_config(&config)?;
    Ok(config.ai_gateway.request_log_details_enabled)
}

pub(super) fn refresh_ai_gw_enable_logging(
    handles: &UiHandles,
    enabled: bool,
    details_enabled: bool,
) {
    if !handles.ai_gw_enable_logging.has_focus() {
        handles.ai_gw_enable_logging.set_value(enabled);
    }
    handles.ai_gw_enable_log_details.enable(enabled);
    if !handles.ai_gw_enable_log_details.has_focus() {
        handles
            .ai_gw_enable_log_details
            .set_value(enabled && details_enabled);
    }
    // Show/hide the disabled hint based on logging state
    if enabled {
        handles.request_log_disabled_hint.show(false);
    } else {
        handles.request_log_disabled_hint.show(true);
    }
}

pub(super) fn refresh_ai_gw_provider_list(handles: &UiHandles, config: Option<&AiGatewayConfig>) {
    let rows = ai_gw_provider_list_rows(config);
    let mut current_rows = handles.ai_gw_provider_rows.borrow_mut();
    if *current_rows == rows {
        return;
    }

    let previous_len = current_rows.len();
    let previous_rows = current_rows.clone();
    let selected_row = handles.ai_gw_provider_list.get_selected_row();
    let new_len = rows.len();
    *current_rows = rows;
    drop(current_rows);

    if previous_len != new_len {
        handles.ai_gw_provider_model.borrow_mut().reset(new_len);
        if let Some(row) = selected_row.filter(|row| *row < new_len) {
            handles.ai_gw_provider_list.select_row(row);
        }
    } else {
        let model = handles.ai_gw_provider_model.borrow();
        let current_rows = handles.ai_gw_provider_rows.borrow();
        for row in 0..new_len {
            if previous_rows.get(row) != current_rows.get(row) {
                model.row_changed(row);
            }
        }
    }
}

fn ai_gw_provider_list_rows(config: Option<&AiGatewayConfig>) -> Vec<AiGwProviderRow> {
    let Some(config) = config else {
        return Vec::new();
    };
    config
        .providers
        .iter()
        .map(|provider| AiGwProviderRow {
            enabled: provider.enabled,
            name: provider.name.clone(),
            provider_type: provider.provider_type.clone(),
            compatibility: provider.compatibility.clone(),
            base_url: provider.base_url.clone(),
            models_url: provider.models_url.clone(),
            weight: provider.effective_weight(),
        })
        .collect()
}

pub(super) fn provider_logo_variant(row: &AiGwProviderRow) -> Variant {
    let bitmap = provider_logo_bitmap(provider_logo_kind(row), 18);
    (&bitmap).into()
}

fn provider_logo_kind(row: &AiGwProviderRow) -> ProviderLogoKind {
    match row.provider_type {
        ProviderType::OpenAiResponses => ProviderLogoKind::OpenAi,
        ProviderType::GrokResponses => ProviderLogoKind::Grok,
        ProviderType::ChatCompletions => ProviderLogoKind::DeepSeek,
        ProviderType::AnthropicMessages => match row.compatibility.as_deref() {
            Some("glm_anthropic" | "zhipu_anthropic") => ProviderLogoKind::Zhipu,
            _ => ProviderLogoKind::Anthropic,
        },
    }
}

pub(super) fn provider_protocol_display(
    provider_type: &ProviderType,
    compatibility: Option<&str>,
) -> String {
    match provider_type {
        ProviderType::OpenAiResponses => "OpenAI Responses".to_string(),
        ProviderType::GrokResponses => "Grok Responses".to_string(),
        ProviderType::ChatCompletions => "Chat Completions".to_string(),
        ProviderType::AnthropicMessages => match compatibility {
            Some("glm_anthropic" | "zhipu_anthropic") => "GLM Anthropic Messages".to_string(),
            _ => "Anthropic Messages".to_string(),
        },
    }
}

pub(super) fn apply_pending_ai_gw_action(
    handles: &UiHandles,
    frame: &Frame,
    result: AiGwActionResult,
) {
    match result {
        AiGwActionResult::Save(Ok(())) => {
            handles
                .ai_gw_delete_button
                .set_label(handles.text.ai_gw_delete_channel());
            set_ai_gw_actions_enabled(handles, true);
            super::show_info(frame, handles.text.ai_gw_saved());
        }
        AiGwActionResult::Save(Err(err)) => {
            handles
                .ai_gw_delete_button
                .set_label(handles.text.ai_gw_delete_channel());
            set_ai_gw_actions_enabled(handles, true);
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::Delete(Ok(())) => {
            handles
                .ai_gw_delete_button
                .set_label(handles.text.ai_gw_delete_channel());
            set_ai_gw_actions_enabled(handles, true);
            super::show_info(frame, handles.text.ai_gw_deleted());
        }
        AiGwActionResult::Delete(Err(err)) => {
            handles
                .ai_gw_delete_button
                .set_label(handles.text.ai_gw_delete_channel());
            set_ai_gw_actions_enabled(handles, true);
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::ChannelToggle {
            row,
            previous_enabled: _,
            result: Ok(()),
        } => {
            handles
                .ai_gw_provider_model
                .borrow()
                .row_value_changed(row, 0);
        }
        AiGwActionResult::ChannelToggle {
            row,
            previous_enabled,
            result: Err(err),
        } => {
            if let Some(row_data) = handles.ai_gw_provider_rows.borrow_mut().get_mut(row) {
                row_data.enabled = previous_enabled;
            }
            handles
                .ai_gw_provider_model
                .borrow()
                .row_value_changed(row, 0);
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::FilterImageGeneration(Ok(_enabled)) => {}
        AiGwActionResult::FilterImageGeneration(Err(err)) => {
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::RequestLogging(Ok(_enabled)) => {}
        AiGwActionResult::RequestLogging(Err(err)) => {
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::RequestLogDetails(Ok(_enabled)) => {}
        AiGwActionResult::RequestLogDetails(Err(err)) => {
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
    }
}

pub(super) fn set_ai_gw_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.ai_gw_delete_button.enable(enabled);
    handles.ai_gw_new_button.enable(enabled);
    handles.ai_gw_edit_button.enable(enabled);
    handles.ai_gw_provider_list.enable(enabled);
    handles.ai_gw_filter_image_generation.enable(enabled);
}
