use wxdragon::prelude::*;
use wxdragon::widgets::dataview::DataViewCtrl;

use super::api::{
    ApiClient, CodexAppProviderStatus, CodexAppStatus, ConfigureRequest, DashboardSnapshot,
    DeleteProviderRequest, SetProviderWebSocketRequest,
};
use super::text::GuiText;
use super::{
    ConfigActionResult, ConfigActionResultStore, DEFAULT_PROVIDER_NAME, DashboardRefresh, UiHandles,
};
use super::{schedule_dashboard_refresh, set_actions_enabled};
use super::{show_error, show_info, show_local_codex_app_config_preview};

pub(super) fn configure_codex_app_and_verify(
    api: &ApiClient,
    request: &ConfigureRequest,
    selected_provider: &str,
    text: GuiText,
) -> Result<CodexAppStatus, String> {
    api.configure_codex_app(request)?;
    let status = api.codex_app_status()?;
    verify_selected_provider(text, &status, selected_provider)?;
    Ok(status)
}

pub(super) fn save_codex_provider_and_verify(
    api: &ApiClient,
    request: &ConfigureRequest,
    selected_provider: &str,
    text: GuiText,
) -> Result<CodexAppStatus, String> {
    api.configure_codex_app(request)?;
    let status = api.codex_app_status()?;
    verify_saved_provider(text, &status, selected_provider)?;
    Ok(status)
}

pub(super) fn set_provider_websocket_and_verify(
    api: &ApiClient,
    provider_name: &str,
    enabled: bool,
    text: GuiText,
) -> Result<CodexAppStatus, String> {
    let request = SetProviderWebSocketRequest {
        provider_name: provider_name.to_string(),
        enabled,
    };
    api.set_codex_provider_websocket(&request)?;
    let status = api.codex_app_status()?;
    verify_provider_websocket(text, &status, provider_name, enabled)?;
    Ok(status)
}

pub(super) fn delete_codex_provider_and_verify(
    api: &ApiClient,
    request: &DeleteProviderRequest,
    text: GuiText,
) -> Result<CodexAppStatus, String> {
    api.delete_codex_provider(request)?;
    let status = api.codex_app_status()?;
    verify_deleted_provider(text, &status, &request.provider_name)?;
    Ok(status)
}

fn verify_selected_provider(
    text: GuiText,
    status: &CodexAppStatus,
    selected_provider: &str,
) -> Result<(), String> {
    let selected_provider = selected_provider.trim();
    if selected_provider.is_empty() {
        return Ok(());
    }

    let active = status
        .provider
        .as_ref()
        .map(|provider| provider.name.as_str());
    if active == Some(selected_provider) {
        return Ok(());
    }

    Err(text.provider_verify_selected_failed(active.unwrap_or(text.unset()), selected_provider))
}

fn verify_saved_provider(
    text: GuiText,
    status: &CodexAppStatus,
    selected_provider: &str,
) -> Result<(), String> {
    let selected_provider = selected_provider.trim();
    if selected_provider.is_empty() {
        return Err(text.provider_name_empty().to_string());
    }

    if provider_rows(status)
        .iter()
        .any(|provider| provider.name == selected_provider)
    {
        return Ok(());
    }

    Err(text.provider_verify_saved_failed(selected_provider))
}

fn verify_provider_websocket(
    text: GuiText,
    status: &CodexAppStatus,
    provider_name: &str,
    expected: bool,
) -> Result<(), String> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return Err(text.provider_name_empty().to_string());
    }

    let actual = provider_rows(status)
        .iter()
        .find(|provider| provider.name == provider_name)
        .map(|provider| provider.supports_websockets);
    if actual == Some(expected) {
        return Ok(());
    }

    let actual = actual
        .map(|value| value.to_string())
        .unwrap_or_else(|| text.not_found().to_string());
    Err(text.provider_verify_websocket_failed(provider_name, &actual, expected))
}

fn verify_deleted_provider(
    text: GuiText,
    status: &CodexAppStatus,
    provider_name: &str,
) -> Result<(), String> {
    if provider_rows(status)
        .iter()
        .any(|provider| provider.name == provider_name)
    {
        return Err(text.provider_verify_deleted_failed(provider_name));
    }
    Ok(())
}

pub(super) fn apply_pending_config_action(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    refresh: &DashboardRefresh,
    result: &ConfigActionResultStore,
) -> bool {
    let result = result.lock().ok().and_then(|mut slot| slot.take());
    let Some(result) = result else {
        return false;
    };

    let provider_websocket_result = matches!(&result, ConfigActionResult::ProviderWebSocket { .. });
    if provider_websocket_result {
        handles.provider_list.enable(true);
    } else {
        handles.configure_button.set_label(handles.text.enable());
        handles.save_provider_button.set_label(handles.text.save());
        handles
            .delete_provider_button
            .set_label(handles.text.delete());
        set_actions_enabled(handles, true);
    }

    match result {
        ConfigActionResult::Save {
            provider_name,
            result: Ok(status),
        } => {
            apply_provider_action_status(handles, refresh, status, &provider_name);
            show_info(frame, handles.text.provider_saved_info());
            schedule_dashboard_refresh(api, refresh);
        }
        ConfigActionResult::Save {
            result: Err(err), ..
        } => {
            show_local_codex_app_config_preview(handles, api, refresh);
            show_error(frame, &err);
        }
        ConfigActionResult::Delete(Ok(status)) => {
            clear_provider_list_selection(&handles.provider_list);
            set_combo_value_if_changed(&handles.provider_name, "");
            change_text_value_if_changed(&handles.provider_base_url, "");
            change_text_value_if_changed(&handles.provider_key, "");
            let snapshot = DashboardSnapshot {
                service_online: true,
                codex_app: Some(status),
                ..DashboardSnapshot::default()
            };
            if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
                last_snapshot.replace(snapshot.clone());
            }
            fill_provider_form_if_empty(handles, &snapshot);
            show_info(frame, handles.text.provider_deleted_info());
            schedule_dashboard_refresh(api, refresh);
        }
        ConfigActionResult::Delete(Err(err)) => {
            show_local_codex_app_config_preview(handles, api, refresh);
            show_error(frame, &err);
        }
        ConfigActionResult::Configure {
            provider_name,
            result: Ok(status),
        } => {
            apply_provider_action_status(handles, refresh, status, &provider_name);
            show_info(frame, handles.text.provider_enabled_info());
            schedule_dashboard_refresh(api, refresh);
        }
        ConfigActionResult::Configure {
            result: Err(err), ..
        } => {
            show_local_codex_app_config_preview(handles, api, refresh);
            show_error(frame, &err);
        }
        ConfigActionResult::ProviderWebSocket {
            provider_name,
            result: Ok(status),
        } => {
            apply_provider_websocket_status(handles, refresh, status, &provider_name);
        }
        ConfigActionResult::ProviderWebSocket {
            provider_name,
            result: Err(err),
        } => {
            if let Ok(status) = api.codex_app_status() {
                apply_provider_websocket_status(handles, refresh, status, &provider_name);
            }
            show_error(frame, &err);
        }
    }
    true
}

fn apply_provider_action_status(
    handles: &UiHandles,
    refresh: &DashboardRefresh,
    status: CodexAppStatus,
    provider_name: &str,
) {
    let snapshot = DashboardSnapshot {
        service_online: true,
        codex_app: Some(status),
        ..DashboardSnapshot::default()
    };
    if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
        last_snapshot.replace(snapshot.clone());
    }

    if let Some(status) = snapshot.codex_app.as_ref() {
        handles
            .provider_catalog
            .set_label(&provider_catalog_label(handles.text, status));
        handles.provider_catalog.wrap(980);
        handles.provider_catalog.layout();
        refresh_provider_choices(&handles.provider_name, &status.providers);
        refresh_provider_list(handles, Some(status));
    }

    if let Some(provider) = find_provider(&snapshot, provider_name) {
        apply_provider_to_form(handles, &provider, true);
    } else {
        set_combo_value_if_changed(&handles.provider_name, provider_name);
    }
}

fn apply_provider_websocket_status(
    handles: &UiHandles,
    refresh: &DashboardRefresh,
    status: CodexAppStatus,
    provider_name: &str,
) {
    let snapshot = if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
        let mut snapshot = last_snapshot.take().unwrap_or_default();
        snapshot.service_online = true;
        snapshot.codex_app = Some(status);
        last_snapshot.replace(snapshot.clone());
        snapshot
    } else {
        DashboardSnapshot {
            service_online: true,
            codex_app: Some(status),
            ..DashboardSnapshot::default()
        }
    };

    if let Some(status) = snapshot.codex_app.as_ref() {
        handles
            .provider_catalog
            .set_label(&provider_catalog_label(handles.text, status));
        handles.provider_catalog.wrap(980);
        handles.provider_catalog.layout();
        refresh_provider_choices(&handles.provider_name, &status.providers);
        refresh_provider_list(handles, Some(status));
    }

    if let Some(provider) = find_provider(&snapshot, provider_name) {
        apply_provider_to_form(handles, &provider, true);
    }
}

pub(super) fn fill_provider_form_if_empty(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    let Some(status) = snapshot.codex_app.as_ref() else {
        handles
            .provider_catalog
            .set_label(handles.text.provider_catalog_after_service());
        handles.provider_catalog.wrap(980);
        handles.provider_catalog.layout();
        refresh_provider_list(handles, None);
        return;
    };
    handles
        .provider_catalog
        .set_label(&provider_catalog_label(handles.text, status));
    handles.provider_catalog.wrap(980);
    handles.provider_catalog.layout();
    refresh_provider_list(handles, Some(status));
    if !handles.provider_image_generation.has_focus() {
        handles
            .provider_image_generation
            .set_value(status.image_generation_enabled);
    }

    if provider_form_has_focus(handles) {
        return;
    }

    refresh_provider_choices(&handles.provider_name, &status.providers);

    let target = status
        .provider
        .as_ref()
        .or_else(|| status.providers.first());
    let current = handles.provider_name.get_value();
    let current = current.trim();
    let provider_values_empty = handles.provider_base_url.get_value().trim().is_empty()
        && handles.provider_key.get_value().trim().is_empty();

    if current.is_empty() {
        if let Some(provider) = target {
            apply_provider_to_form(handles, provider, true);
        } else {
            set_combo_value_if_changed(&handles.provider_name, DEFAULT_PROVIDER_NAME);
        }
    } else if current == DEFAULT_PROVIDER_NAME
        && provider_values_empty
        && let Some(provider) = target
        && provider.name != DEFAULT_PROVIDER_NAME
    {
        apply_provider_to_form(handles, provider, true);
    }

    let selected = handles.provider_name.get_value();
    if let Some(provider) = find_provider(snapshot, &selected) {
        apply_provider_to_form(handles, &provider, false);
    }
}

fn provider_form_has_focus(handles: &UiHandles) -> bool {
    handles.provider_name.has_focus()
        || handles.provider_base_url.has_focus()
        || handles.provider_key.has_focus()
        || handles.provider_image_generation.has_focus()
        || handles.provider_list.has_focus()
}

fn refresh_provider_choices(input: &ComboBox, providers: &[CodexAppProviderStatus]) {
    let names = provider_choice_names(providers);
    if combo_box_items(input) == names {
        return;
    }

    let current = input.get_value();
    let insertion_point = input.get_insertion_point();
    input.clear();
    for name in names {
        input.append(&name);
    }
    set_combo_value_if_changed(input, &current);
    input.set_insertion_point(insertion_point.min(current.chars().count() as i64));
}

fn refresh_provider_list(handles: &UiHandles, status: Option<&CodexAppStatus>) {
    if handles.pending_provider_websocket.borrow().is_some() {
        return;
    }

    let rows = provider_list_rows(handles.text, status);
    let mut current_rows = handles.provider_rows.borrow_mut();
    if *current_rows == rows {
        return;
    }

    let previous_len = current_rows.len();
    let selected_row = handles.provider_list.get_selected_row();
    let new_len = rows.len();
    *current_rows = rows;
    drop(current_rows);

    if previous_len != new_len {
        handles.provider_model.borrow_mut().reset(new_len);
        if let Some(row) = selected_row.filter(|row| *row < new_len) {
            handles.provider_list.select_row(row);
        }
    } else {
        let model = handles.provider_model.borrow();
        for row in 0..new_len {
            model.row_changed(row);
        }
    }
}

fn provider_list_rows(text: GuiText, status: Option<&CodexAppStatus>) -> Vec<[String; 5]> {
    let Some(status) = status else {
        return vec![[
            text.provider_waiting_service().to_string(),
            text.provider_read_after_start().to_string(),
            String::new(),
            String::new(),
            "false".to_string(),
        ]];
    };

    let active_name = status
        .provider
        .as_ref()
        .map(|provider| provider.name.as_str());
    let providers = provider_rows(status);
    if providers.is_empty() {
        return vec![[
            DEFAULT_PROVIDER_NAME.to_string(),
            text.provider_create_on_write().to_string(),
            String::new(),
            text.not_configured().to_string(),
            "false".to_string(),
        ]];
    }

    providers
        .iter()
        .map(|provider| {
            [
                provider.name.clone(),
                provider
                    .base_url
                    .clone()
                    .unwrap_or_else(|| text.not_configured().to_string()),
                if Some(provider.name.as_str()) == active_name {
                    text.in_use().to_string()
                } else {
                    String::new()
                },
                masked_provider_key(text, provider.key.as_deref()),
                provider.supports_websockets.to_string(),
            ]
        })
        .collect()
}

fn provider_rows(status: &CodexAppStatus) -> Vec<CodexAppProviderStatus> {
    let mut providers = status.providers.clone();
    if let Some(active) = &status.provider
        && !providers
            .iter()
            .any(|provider| provider.name == active.name)
    {
        providers.insert(0, active.clone());
    }
    providers
}

fn provider_choice_names(providers: &[CodexAppProviderStatus]) -> Vec<String> {
    if providers.is_empty() {
        return vec![DEFAULT_PROVIDER_NAME.to_string()];
    }

    let mut names = Vec::<String>::new();
    for provider in providers {
        if !names.iter().any(|name| name == &provider.name) {
            names.push(provider.name.clone());
        }
    }
    names
}

fn masked_provider_key(text: GuiText, value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return text.not_configured().to_string();
    };
    format!("{} {}", text.key_configured(), masked_secret(value))
}

fn masked_provider_key_input(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(masked_secret)
        .unwrap_or_default()
}

fn masked_secret(value: &str) -> String {
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("****{suffix}")
}

fn provider_key_value_for_config(value: &str) -> Option<String> {
    let value = value.trim();
    if is_placeholder_config_value(value) || is_masked_provider_key(value) {
        None
    } else {
        Some(value.to_string())
    }
}

fn is_masked_provider_key(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("****")
        && value.chars().filter(|ch| *ch == '*').count() >= 4
        && value.chars().any(|ch| ch != '*')
}

fn combo_box_items(input: &ComboBox) -> Vec<String> {
    (0..input.get_count())
        .filter_map(|index| input.get_string(index))
        .collect()
}

fn provider_catalog_label(text: GuiText, status: &CodexAppStatus) -> String {
    if status.providers.is_empty() {
        if let Some(active) = status.provider.as_ref() {
            return text.current_provider(active.name.as_str());
        }
        return text.no_provider().to_string();
    }

    if let Some(active) = status.provider.as_ref() {
        text.current_provider(&active.name)
    } else {
        text.saved_providers(status.providers.len())
    }
}

pub(super) fn find_provider(
    snapshot: &DashboardSnapshot,
    provider_name: &str,
) -> Option<CodexAppProviderStatus> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return None;
    }
    let status = snapshot.codex_app.as_ref()?;
    status
        .providers
        .iter()
        .find(|provider| provider.name == provider_name)
        .cloned()
        .or_else(|| {
            status
                .provider
                .as_ref()
                .filter(|provider| provider.name == provider_name)
                .cloned()
        })
}

pub(super) fn provider_from_list_row(
    snapshot: &DashboardSnapshot,
    row: usize,
) -> Option<CodexAppProviderStatus> {
    let status = snapshot.codex_app.as_ref()?;
    provider_rows(status).get(row).cloned()
}

pub(super) fn provider_config_request_from_ui(
    handles: &UiHandles,
    provider_name: &ComboBox,
    provider_base_url: &TextCtrl,
    provider_key: &TextCtrl,
    snapshot: Option<&DashboardSnapshot>,
    activate: bool,
) -> (String, ConfigureRequest) {
    let form_provider = clean_provider_text(&provider_name.get_value());
    let mut selected_provider = form_provider.clone();
    let mut selected_base_url = strip_nul(&provider_base_url.get_value());
    let mut selected_key = strip_nul(&provider_key.get_value());

    let selected_row = handles.provider_list.get_selected_row();
    if selected_provider.is_empty()
        && let Some(row) = selected_row
    {
        if let Some(provider) = snapshot.and_then(|snapshot| provider_from_list_row(snapshot, row))
        {
            selected_provider = provider.name;
            let row_base_url = provider.base_url.unwrap_or_default();

            if selected_provider != form_provider || selected_base_url.trim().is_empty() {
                selected_base_url = row_base_url;
            }

            let row_key = masked_provider_key_input(provider.key.as_deref());
            if selected_provider != form_provider || selected_key.trim().is_empty() {
                selected_key = row_key;
            }
        } else if let Some(row_data) = provider_model_row(handles, row) {
            let row_name = clean_provider_text(&row_data[0]);
            if is_real_provider_name(&row_name) {
                selected_provider = row_name;
                let row_base_url = list_base_url_cell_to_input(&row_data[1]);

                if selected_provider != form_provider || selected_base_url.trim().is_empty() {
                    selected_base_url = row_base_url;
                }

                let row_key = list_key_cell_to_input(&row_data[3]);
                if selected_provider != form_provider || selected_key.trim().is_empty() {
                    selected_key = row_key;
                }
            }
        }
    }

    let selected_base_url = config_text_value(&selected_base_url).unwrap_or_default();
    let provider_key = provider_key_value_for_config(&selected_key);
    let supports_websockets = provider_websocket_value_from_ui(handles, &selected_provider);
    let request = ConfigureRequest {
        provider_name: Some(selected_provider.clone()),
        provider_base_url: Some(selected_base_url),
        provider_key,
        model: None,
        activate,
        image_generation_enabled: Some(handles.provider_image_generation.get_value()),
        supports_websockets,
    };
    (selected_provider, request)
}

pub(super) fn provider_name_from_ui(
    handles: &UiHandles,
    provider_name: &ComboBox,
    snapshot: Option<&DashboardSnapshot>,
) -> String {
    let form_provider = clean_provider_text(&provider_name.get_value());
    if !form_provider.is_empty() {
        return form_provider;
    }

    let Some(selected_row) = handles.provider_list.get_selected_row() else {
        return String::new();
    };

    snapshot
        .and_then(|snapshot| provider_from_list_row(snapshot, selected_row))
        .map(|provider| provider.name)
        .unwrap_or_else(|| {
            provider_model_row(handles, selected_row)
                .map(|row| clean_provider_text(&row[0]))
                .unwrap_or_default()
        })
}

pub(super) fn is_real_provider_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "等待本地服务" && value != "Waiting for local service"
}

pub(super) fn apply_provider_row_to_form(handles: &UiHandles, row: usize) {
    let Some(row_data) = provider_model_row(handles, row) else {
        return;
    };
    let name = clean_provider_text(&row_data[0]);
    let base_url = list_base_url_cell_to_input(&row_data[1]);
    let key = list_key_cell_to_input(&row_data[3]);
    if is_real_provider_name(&name) {
        set_combo_value_if_changed(&handles.provider_name, &name);
    }
    change_text_value_if_changed(&handles.provider_base_url, &base_url);
    change_text_value_if_changed(&handles.provider_key, &key);
}

fn list_base_url_cell_to_input(value: &str) -> String {
    let value = strip_nul(value);
    let value = value.trim();
    if is_placeholder_config_value(value) {
        String::new()
    } else {
        value.to_string()
    }
}

fn list_key_cell_to_input(value: &str) -> String {
    let value = strip_nul(value);
    let value = value.trim();
    if is_placeholder_config_value(value) {
        return String::new();
    }
    value
        .strip_prefix("已配置 ")
        .or_else(|| value.strip_prefix("Configured "))
        .unwrap_or(value)
        .to_string()
}

pub(super) fn clean_provider_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

pub(super) fn strip_nul(value: &str) -> String {
    value.chars().filter(|ch| *ch != '\0').collect()
}

fn config_text_value(value: &str) -> Option<String> {
    let value = strip_nul(value).trim().to_string();
    (!is_placeholder_config_value(&value)).then_some(value)
}

fn is_placeholder_config_value(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value.contains("未配") || value == "Not configured"
}

pub(super) fn apply_provider_to_form(
    handles: &UiHandles,
    provider: &CodexAppProviderStatus,
    overwrite: bool,
) {
    if overwrite || handles.provider_name.get_value().trim().is_empty() {
        set_combo_value_if_changed(&handles.provider_name, &provider.name);
    }
    if overwrite || handles.provider_base_url.get_value().trim().is_empty() {
        let base_url = provider
            .base_url
            .as_deref()
            .and_then(config_text_value)
            .unwrap_or_default();
        change_text_value_if_changed(&handles.provider_base_url, &base_url);
    }
    if overwrite || handles.provider_key.get_value().trim().is_empty() {
        let key = provider
            .key
            .as_deref()
            .and_then(config_text_value)
            .map(|value| masked_secret(&value))
            .unwrap_or_default();
        change_text_value_if_changed(&handles.provider_key, &key);
    }
}

pub(super) fn set_combo_value_if_changed(input: &ComboBox, value: &str) {
    if input.get_value() == value {
        return;
    }
    input.set_value(value);
}

pub(super) fn change_text_value_if_changed(input: &TextCtrl, value: &str) {
    if input.get_value() == value {
        return;
    }
    input.change_value(value);
}

fn provider_model_row(handles: &UiHandles, row: usize) -> Option<[String; 5]> {
    handles.provider_rows.borrow().get(row).cloned()
}

fn provider_model_row_by_name(handles: &UiHandles, provider_name: &str) -> Option<[String; 5]> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return None;
    }
    handles
        .provider_rows
        .borrow()
        .iter()
        .find(|row| clean_provider_text(&row[0]) == provider_name)
        .cloned()
}

fn provider_websocket_value_from_ui(handles: &UiHandles, provider_name: &str) -> bool {
    handles
        .provider_list
        .get_selected_row()
        .and_then(|row| provider_model_row(handles, row))
        .filter(|row| {
            let row_name = clean_provider_text(&row[0]);
            provider_name.trim().is_empty() || row_name == provider_name.trim()
        })
        .or_else(|| provider_model_row_by_name(handles, provider_name))
        .map(|row| row[4] == "true")
        .unwrap_or(false)
}

pub(super) fn clear_provider_list_selection(list: &DataViewCtrl) {
    list.unselect_all();
}
