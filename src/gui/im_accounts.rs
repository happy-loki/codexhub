use wxdragon::prelude::*;

use super::api::{ApiClient, DashboardSnapshot, ImAccountItem};
use super::text::GuiText;
use super::widgets::{ImStatusPanel, StateTone, set_im_channel_row};
use super::{DashboardRefresh, ImActionResult, UiHandles, revert_im_toggle};
use super::{cached_dashboard_snapshot, force_dashboard_refresh, schedule_dashboard_refresh};
use super::{show_error, show_info};

pub(super) fn apply_pending_im_action(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    refresh: &DashboardRefresh,
    result: ImActionResult,
) {
    match result {
        ImActionResult::TelegramConfigure(Ok(_)) => {
            handles
                .save_telegram_button
                .set_label(handles.text.add_telegram_bot());
            handles.save_telegram_button.enable(true);
            show_info(frame, handles.text.telegram_saved());
            schedule_dashboard_refresh(api, refresh);
        }
        ImActionResult::TelegramConfigure(Err(err)) => {
            handles
                .save_telegram_button
                .set_label(handles.text.add_telegram_bot());
            handles.save_telegram_button.enable(true);
            show_error(frame, &err);
            schedule_dashboard_refresh(api, refresh);
        }
        ImActionResult::AccountToggle {
            row,
            previous_enabled: _,
            result: Ok(_),
        } => {
            handles.im_account_model.borrow().row_value_changed(row, 4);
            schedule_dashboard_refresh(api, refresh);
        }
        ImActionResult::AccountToggle {
            row,
            previous_enabled,
            result: Err(err),
        } => {
            revert_im_toggle(handles, row, previous_enabled);
            show_error(frame, &err);
            schedule_dashboard_refresh(api, refresh);
        }
        ImActionResult::AccountDelete(Ok(_)) => {
            handles
                .delete_im_account_button
                .set_label(handles.text.delete_selected());
            handles.delete_im_account_button.enable(true);
            show_info(frame, handles.text.im_account_deleted());
            force_dashboard_refresh(api, refresh);
        }
        ImActionResult::AccountDelete(Err(err)) => {
            handles
                .delete_im_account_button
                .set_label(handles.text.delete_selected());
            handles.delete_im_account_button.enable(true);
            show_error(frame, &err);
            schedule_dashboard_refresh(api, refresh);
        }
    }
}

pub(super) struct SelectedImAccount {
    pub(super) platform: String,
    pub(super) account_id: String,
    pub(super) display_name: Option<String>,
}

pub(super) fn selected_im_account(
    handles: &UiHandles,
    refresh: &DashboardRefresh,
) -> Option<SelectedImAccount> {
    let selected = handles.im_account_list.get_selected_row()?;
    let row_data = handles.im_account_rows.borrow().get(selected).cloned()?;
    let platform = im_platform_key(&row_data[1])?;
    let account_id = row_data[3].clone();
    if account_id.trim().is_empty() {
        return None;
    }
    cached_dashboard_snapshot(refresh)
        .and_then(|snapshot| snapshot.im_accounts)
        .and_then(|accounts| {
            accounts
                .accounts
                .into_iter()
                .find(|account| account.platform == platform && account.account_id == account_id)
        })
        .map(|account| SelectedImAccount {
            platform: account.platform,
            account_id: account.account_id,
            display_name: account.display_name,
        })
}

fn im_platform_label(text: GuiText, platform: &str) -> &'static str {
    match platform {
        "feishu" => text.feishu_label(),
        "telegram" => "Telegram",
        "wechat" => text.wechat_label(),
        _ => "IM",
    }
}

pub(super) fn im_platform_key(label: &str) -> Option<String> {
    match label.trim() {
        "飞书" | "Feishu" | "feishu" => Some("feishu".to_string()),
        "Telegram" | "telegram" => Some("telegram".to_string()),
        "微信" | "WeChat" | "wechat" => Some("wechat".to_string()),
        _ => None,
    }
}

pub(super) fn refresh_im_account_list(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    if snapshot.service_online
        && snapshot.im_accounts.is_none()
        && !handles.im_account_rows.borrow().is_empty()
    {
        return;
    }

    let rows = im_account_list_rows(handles.text, snapshot);
    refresh_im_status_from_rows(handles.text, &handles.im_status, &rows);
    let mut current_rows = handles.im_account_rows.borrow_mut();
    if *current_rows == rows {
        return;
    }

    let previous_len = current_rows.len();
    let previous_rows = current_rows.clone();
    let selected_row = handles.im_account_list.get_selected_row();
    let new_len = rows.len();
    *current_rows = rows;
    drop(current_rows);

    if previous_len != new_len {
        handles.im_account_model.borrow_mut().reset(new_len);
        if let Some(row) = selected_row.filter(|row| *row < new_len) {
            handles.im_account_list.select_row(row);
        }
    } else {
        let model = handles.im_account_model.borrow();
        let current_rows = handles.im_account_rows.borrow();
        for row in 0..new_len {
            if previous_rows.get(row) != current_rows.get(row) {
                model.row_changed(row);
            }
        }
    }
}

fn im_account_list_rows(text: GuiText, snapshot: &DashboardSnapshot) -> Vec<[String; 5]> {
    if !snapshot.service_online {
        return vec![[
            text.waiting_service().to_string(),
            "IM".to_string(),
            text.im_waiting_service_row().to_string(),
            String::new(),
            "false".to_string(),
        ]];
    }
    let Some(accounts) = snapshot.im_accounts.as_ref() else {
        return vec![[
            text.reading().to_string(),
            "IM".to_string(),
            text.reading_bot_list().to_string(),
            String::new(),
            "false".to_string(),
        ]];
    };
    if accounts.accounts.is_empty() {
        return vec![[
            text.not_connected().to_string(),
            "IM".to_string(),
            text.scan_or_token().to_string(),
            String::new(),
            "false".to_string(),
        ]];
    }
    accounts
        .accounts
        .iter()
        .map(|account| {
            [
                account
                    .display_name
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| account.account_id.clone()),
                im_platform_label(text, &account.platform).to_string(),
                im_account_state_label(text, account).to_string(),
                account.account_id.clone(),
                account.enabled.to_string(),
            ]
        })
        .collect()
}

fn im_account_state_label(text: GuiText, account: &ImAccountItem) -> &'static str {
    let has_error = account
        .last_error
        .as_deref()
        .is_some_and(|err| !err.trim().is_empty());
    let long_polling_ready =
        matches!(account.platform.as_str(), "telegram" | "wechat") && account.polling && !has_error;

    if !account.configured || !account.secret_set {
        text.not_configured()
    } else if !account.enabled {
        text.paused()
    } else if account.connected || long_polling_ready {
        text.im_connected()
    } else if has_error {
        text.error()
    } else if account.connecting || account.polling {
        text.connecting()
    } else {
        text.waiting_connection()
    }
}

fn refresh_im_status_from_rows(text: GuiText, status: &ImStatusPanel, rows: &[[String; 5]]) {
    let (state, detail, tone) = im_channel_summary_from_rows(text, rows, "feishu");
    set_im_channel_row(&status.feishu, state, &detail, tone);
    let (state, detail, tone) = im_channel_summary_from_rows(text, rows, "telegram");
    set_im_channel_row(&status.telegram, state, &detail, tone);
    let (state, detail, tone) = im_channel_summary_from_rows(text, rows, "wechat");
    set_im_channel_row(&status.wechat, state, &detail, tone);
}

fn im_channel_summary_from_rows<'a>(
    text: GuiText,
    rows: &'a [[String; 5]],
    platform: &str,
) -> (&'static str, String, StateTone) {
    let platform_rows = rows
        .iter()
        .filter(|row| im_platform_key(&row[1]).as_deref() == Some(platform))
        .collect::<Vec<_>>();
    if platform_rows.is_empty() {
        return (
            text.not_connected(),
            text.im_empty_detail(platform),
            StateTone::Warn,
        );
    }

    let enabled_rows = platform_rows
        .iter()
        .copied()
        .filter(|row| row[4] == "true")
        .collect::<Vec<_>>();
    if enabled_rows.is_empty() {
        return (
            text.paused(),
            im_channel_first_name(&platform_rows)
                .map(|name| text.name_saved(&name))
                .unwrap_or_else(|| text.bot_saved().to_string()),
            StateTone::Muted,
        );
    }

    for (state, tone) in [
        (text.im_connected(), StateTone::Ok),
        (text.connecting(), StateTone::Warn),
        (text.waiting_connection(), StateTone::Warn),
        (text.error(), StateTone::Error),
    ] {
        if let Some(row) = enabled_rows.iter().find(|row| row[2] == state) {
            return (state, im_channel_row_detail(text, platform, row), tone);
        }
    }

    (
        text.waiting_connection(),
        im_channel_first_name(&enabled_rows)
            .map(|name| text.bot_waiting(&name))
            .unwrap_or_else(|| text.waiting_bot_connection().to_string()),
        StateTone::Warn,
    )
}

fn im_channel_row_detail(text: GuiText, platform: &str, row: &[String; 5]) -> String {
    let name = row[0].trim();
    let fallback = text.bot_fallback(platform);
    let name = if name.is_empty() { fallback } else { name };
    match row[2].as_str() {
        state if state == text.im_connected() => name.to_string(),
        state if state == text.connecting() => text.bot_connecting(name),
        state if state == text.waiting_connection() => text.bot_waiting(name),
        state if state == text.error() => text.bot_error(name),
        _ => name.to_string(),
    }
}

fn im_channel_first_name(rows: &[&[String; 5]]) -> Option<String> {
    rows.iter()
        .map(|row| row[0].trim())
        .find(|name| !name.is_empty())
        .map(str::to_string)
}
