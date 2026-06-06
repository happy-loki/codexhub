use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};

use crate::{
    app_state::SharedState,
    codex_app_config,
    im_runtime::{PendingApproval, ThreadCreateDraftState},
    remote_control_backend,
};

static THREAD_ROUTING_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Default)]
pub(crate) struct ThreadCreateForm {
    pub(crate) cwd_choice: Option<String>,
    pub(crate) cwd_custom: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) permission: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadModelChoice {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadCreateDefaults {
    pub remote_name: Option<String>,
    pub cwd: Option<String>,
    pub model_provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Option<String>,
    pub projects: Vec<String>,
    pub models: Vec<ThreadModelChoice>,
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ThreadCreateOption {
    pub(crate) label: String,
    pub(crate) summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ThreadModelCatalogEntry {
    model: String,
    label: String,
    hidden: bool,
    is_default: bool,
    supported_efforts: Vec<String>,
    default_effort: Option<String>,
}

pub(crate) fn thread_create_form_from_draft(draft: &ThreadCreateDraftState) -> ThreadCreateForm {
    ThreadCreateForm {
        cwd_choice: draft.cwd_choice.clone(),
        cwd_custom: draft.cwd_custom.clone(),
        model: draft.model.clone(),
        effort: draft.effort.clone(),
        permission: draft.permission.clone(),
    }
}

pub(crate) async fn thread_start_options_from_form_for_client(
    state: &SharedState,
    client_key: &str,
    form: ThreadCreateForm,
) -> Result<remote_control_backend::ThreadStartOptions> {
    let cwd = normalize_thread_cwd(form.cwd_choice, form.cwd_custom)?;
    let model = normalize_optional_selection(form.model);
    let reasoning_effort = normalize_reasoning_effort(form.effort)?;
    validate_reasoning_effort_for_model_for_client(
        state,
        client_key,
        model.as_deref(),
        reasoning_effort.as_deref(),
    )
    .await?;
    let (permissions, approval_policy, approvals_reviewer) =
        permission_mode_to_thread_start(form.permission)?;

    Ok(thread_start_options_with_current_provider(
        remote_control_backend::ThreadStartOptions {
            cwd,
            model,
            reasoning_effort,
            permissions,
            approval_policy,
            approvals_reviewer,
            ..Default::default()
        },
    ))
}

pub(crate) fn thread_start_options_with_current_provider(
    mut options: remote_control_backend::ThreadStartOptions,
) -> remote_control_backend::ThreadStartOptions {
    options.model_provider = load_codex_app_model_provider();
    options
}

pub(crate) fn summarize_thread_start_options(
    options: &remote_control_backend::ThreadStartOptions,
) -> String {
    let mut lines = Vec::new();
    if let Some(cwd) = options.cwd.as_ref() {
        lines.push(format!("目录：`{cwd}`"));
    } else {
        lines.push("目录：使用 Codex App 默认值".to_string());
    }
    if let Some(provider) = options.model_provider.as_ref() {
        lines.push(format!("Provider：`{provider}`"));
    }
    if let Some(model) = options.model.as_ref() {
        lines.push(format!("模型：`{model}`"));
    }
    if let Some(effort) = options.reasoning_effort.as_ref() {
        lines.push(format!("推理强度：`{effort}`"));
    }
    lines.push(format!("权限：{}", thread_start_permission_label(options)));
    lines.join("\n")
}

pub(crate) async fn load_thread_create_defaults_for_client(
    state: &SharedState,
    client_key: &str,
) -> ThreadCreateDefaults {
    let local_doc = load_codex_app_config_doc();
    let remote_status = remote_control_backend::status_snapshot(state).await;
    let remote_config =
        remote_control_backend::config_read_for_client(state, client_key, None, false)
            .await
            .ok()
            .and_then(|value| value.get("config").cloned());
    let catalog = load_model_catalog_for_client(state, client_key)
        .await
        .unwrap_or_default();
    let catalog_default_model = catalog
        .iter()
        .find(|entry| entry.is_default)
        .or_else(|| catalog.iter().find(|entry| !entry.hidden))
        .map(|entry| entry.model.clone());
    let model = config_string(remote_config.as_ref(), "model")
        .or_else(|| local_config_string(local_doc.as_ref(), "model"))
        .or(catalog_default_model);
    let effort = config_string(remote_config.as_ref(), "model_reasoning_effort")
        .or_else(|| {
            model
                .as_deref()
                .and_then(|model| catalog.iter().find(|entry| entry.model == model))
                .and_then(|entry| entry.default_effort.clone())
        })
        .or_else(|| local_config_string(local_doc.as_ref(), "model_reasoning_effort"));

    ThreadCreateDefaults {
        remote_name: remote_status.server_name,
        cwd: None,
        model_provider: config_string(remote_config.as_ref(), "model_provider")
            .or_else(|| local_config_string(local_doc.as_ref(), "model_provider")),
        model: model.clone(),
        effort: effort.clone(),
        permission: infer_permission_label(remote_config.as_ref()),
        projects: codex_project_paths(local_doc.as_ref()),
        models: thread_model_choices(model.as_deref(), &catalog),
        efforts: thread_reasoning_effort_choices(model.as_deref(), &catalog, effort.as_deref()),
    }
}

pub(crate) fn load_codex_app_model_provider() -> Option<String> {
    load_codex_app_config_doc().and_then(|doc| {
        doc.get("model_provider")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn next_thread_routing_request_id() -> String {
    let value = THREAD_ROUTING_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("thread-route-{value}")
}

#[derive(Debug, Clone)]
pub(crate) struct ThreadListEntry {
    pub(crate) thread_id: String,
    pub(crate) title: String,
    pub(crate) state: String,
    pub(crate) cwd: Option<String>,
}

pub(crate) fn build_thread_entries(
    loaded_ids: &[String],
    history_threads: &[serde_json::Value],
    current_thread_id: Option<&str>,
) -> Vec<ThreadListEntry> {
    let loaded_set = loaded_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    history_threads
        .iter()
        .map(|thread| {
            let thread_id = thread
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let title = summarize_thread_title(thread);
            ThreadListEntry {
                thread_id,
                title,
                state: summarize_thread_route_state(thread, &loaded_set, current_thread_id),
                cwd: thread_cwd(thread),
            }
        })
        .collect::<Vec<_>>()
}

pub(crate) fn thread_cwd(thread: &serde_json::Value) -> Option<String> {
    thread
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

pub(crate) fn summarize_thread_title(thread: &serde_json::Value) -> String {
    thread
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            thread
                .get("preview")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| truncate_text(v, 80))
        })
        .unwrap_or_else(|| {
            let thread_id = thread
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("会话 {thread_id}")
        })
}

pub(crate) fn summarize_thread_cwd(thread: &serde_json::Value) -> String {
    thread_cwd(thread)
        .map(|cwd| format!("目录：`{cwd}`"))
        .unwrap_or_else(|| "目录未知".to_string())
}

pub(crate) fn summarize_thread_status(thread: &serde_json::Value) -> String {
    match thread
        .get("status")
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
    {
        "active" => "运行中".to_string(),
        "idle" => "空闲".to_string(),
        "notLoaded" => "未加载".to_string(),
        "systemError" => "系统错误".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn is_approval_reply(command: &str) -> bool {
    matches!(command, "/y" | "/yes" | "/n" | "/no")
        || command
            .strip_prefix('/')
            .and_then(|value| value.parse::<usize>().ok())
            .is_some()
}

pub(crate) fn approval_reply_hint(pending: &PendingApproval) -> String {
    let options = pending
        .decisions
        .iter()
        .enumerate()
        .map(|(index, _)| format!("/{}", index + 1))
        .collect::<Vec<_>>();
    if options.is_empty() {
        "`/y` 或 `/n`".to_string()
    } else {
        options.join("、")
    }
}

pub(crate) fn thread_create_help_text(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> String {
    let lines = vec![
        "创建新 Codex thread".to_string(),
        String::new(),
        "当前设置：".to_string(),
        format!("目录：{}", selected_cwd_text(defaults, draft)),
        format!(
            "Provider：{}",
            defaults
                .model_provider
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("使用 Codex App 当前 provider")
        ),
        format!("模型：{}", selected_model_text(defaults, draft)),
        format!("推理强度：{}", selected_effort_text(defaults, draft)),
        format!("权限：{}", selected_permission_text(defaults, draft)),
        String::new(),
        "请选择要修改的设置，确认后创建。".to_string(),
    ];
    lines.join("\n")
}

pub(crate) fn create_options_for_field(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
    field: &str,
) -> Result<(String, String, Vec<(String, ThreadCreateOption)>)> {
    match field {
        "cwd" => Ok(cwd_create_options(defaults, draft)),
        "model" => Ok(model_create_options(defaults, draft)),
        "effort" => Ok(effort_create_options(defaults, draft)),
        "perm" => Ok(permission_create_options(defaults, draft)),
        _ => Err(anyhow!("不支持的创建字段：{field}")),
    }
}

pub(crate) fn apply_thread_create_draft_value(
    draft: &mut ThreadCreateDraftState,
    field: &str,
    value: &str,
) -> Result<()> {
    let Some(field) = normalize_thread_create_field(field) else {
        return Err(anyhow!("不支持的创建字段：{field}"));
    };
    let value = value.trim();
    match field {
        "cwd" => {
            draft.cwd_custom = None;
            draft.cwd_choice = (!is_default_value(value)).then(|| value.to_string());
        }
        "model" => {
            draft.model = (!is_default_value(value)).then(|| value.to_string());
        }
        "effort" => {
            draft.effort = (!is_default_value(value)).then(|| value.to_string());
        }
        "perm" => {
            draft.permission = (!is_default_value(value)).then(|| value.to_string());
        }
        _ => return Err(anyhow!("不支持的创建字段：{field}")),
    }
    Ok(())
}

pub(crate) fn normalize_thread_create_field(field: &str) -> Option<&'static str> {
    match field.trim().to_ascii_lowercase().as_str() {
        "cwd" | "dir" | "path" => Some("cwd"),
        "model" => Some("model"),
        "effort" | "reasoning" | "reasoning_effort" => Some("effort"),
        "perm" | "permission" | "permissions" => Some("perm"),
        _ => None,
    }
}

fn cwd_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, ThreadCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用 Codex App 默认目录",
        None,
        draft.cwd_custom.is_none() && is_default_selection(draft.cwd_choice.as_deref()),
    );
    for project in defaults
        .projects
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        push_create_option(
            &mut options,
            project,
            &project_option_label(project),
            Some(project.to_string()),
            draft.cwd_custom.is_none() && draft.cwd_choice.as_deref() == Some(project),
        );
    }
    (
        "选择项目目录".to_string(),
        format!("当前：{}", selected_cwd_text(defaults, draft)),
        options,
    )
}

fn model_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, ThreadCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用当前模型",
        Some(
            defaults
                .model
                .as_deref()
                .map(|model| format!("当前默认：{model}"))
                .unwrap_or_else(|| "不覆盖模型，由 Codex App 决定".to_string()),
        ),
        is_default_selection(draft.model.as_deref()),
    );
    for model in defaults
        .models
        .iter()
        .filter(|model| !model.value.trim().is_empty())
    {
        push_create_option(
            &mut options,
            &model.value,
            &model.label,
            None,
            draft.model.as_deref() == Some(model.value.as_str()),
        );
    }
    (
        "选择模型".to_string(),
        format!("当前：{}", selected_model_text(defaults, draft)),
        options,
    )
}

fn effort_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, ThreadCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用模型默认推理强度",
        Some(
            defaults
                .effort
                .as_deref()
                .map(|effort| format!("当前默认：{}", reasoning_effort_label(effort)))
                .unwrap_or_else(|| "不覆盖推理强度，由模型决定".to_string()),
        ),
        is_default_selection(draft.effort.as_deref()),
    );
    if let Some(effort) = defaults
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_create_option(
            &mut options,
            effort,
            &reasoning_effort_label(effort),
            None,
            draft.effort.as_deref() == Some(effort),
        );
    }
    for effort in defaults
        .efforts
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        push_create_option(
            &mut options,
            effort,
            &reasoning_effort_label(effort),
            None,
            draft.effort.as_deref() == Some(effort),
        );
    }
    (
        "选择推理强度".to_string(),
        format!("当前：{}", selected_effort_text(defaults, draft)),
        options,
    )
}

fn permission_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, ThreadCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用 Codex App 当前权限",
        Some(
            defaults
                .permission
                .as_deref()
                .map(|permission| format!("当前：{permission}"))
                .unwrap_or_else(|| "不覆盖权限配置".to_string()),
        ),
        is_default_selection(draft.permission.as_deref()),
    );
    for (value, label, summary) in [
        (
            "workspace_user",
            "默认权限",
            "适合常规项目，需要时由用户确认。",
        ),
        ("auto_review", "自动审查", "需要审批时优先交给自动审查。"),
        (
            "full_access",
            "完全访问权限",
            "不再请求确认，允许完整本机访问。",
        ),
    ] {
        push_create_option(
            &mut options,
            value,
            label,
            Some(summary.to_string()),
            draft.permission.as_deref() == Some(value),
        );
    }
    (
        "选择权限".to_string(),
        format!("当前：{}", selected_permission_text(defaults, draft)),
        options,
    )
}

fn push_create_option(
    options: &mut Vec<(String, ThreadCreateOption)>,
    value: &str,
    label: &str,
    summary: Option<String>,
    selected: bool,
) {
    let value = value.trim();
    if value.is_empty() || options.iter().any(|(existing, _)| existing == value) {
        return;
    }
    let label = if selected {
        format!("已选：{}", label.trim())
    } else {
        label.trim().to_string()
    };
    let summary = match (selected, summary) {
        (true, Some(summary)) if !summary.trim().is_empty() => Some(summary.trim().to_string()),
        (true, _) => Some("已选".to_string()),
        (false, Some(summary)) if !summary.trim().is_empty() => Some(summary.trim().to_string()),
        _ => None,
    };
    options.push((value.to_string(), ThreadCreateOption { label, summary }));
}

fn is_default_selection(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(is_default_value)
}

fn is_default_value(value: &str) -> bool {
    matches!(value.trim(), "" | "__default__" | "default" | "默认")
}

fn selected_cwd_text(defaults: &ThreadCreateDefaults, draft: &ThreadCreateDraftState) -> String {
    if let Some(cwd) = draft
        .cwd_custom
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return cwd.to_string();
    }
    if let Some(cwd) = draft
        .cwd_choice
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty() && !is_default_value(v))
    {
        if cwd == "__custom__" {
            return "等待输入自定义目录".to_string();
        }
        return cwd.to_string();
    }
    defaults
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|cwd| format!("使用 Codex App 默认目录（{cwd}）"))
        .unwrap_or_else(|| "使用 Codex App 默认目录".to_string())
}

fn selected_model_text(defaults: &ThreadCreateDefaults, draft: &ThreadCreateDraftState) -> String {
    if let Some(model) = draft
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_default_value(value))
    {
        return model.to_string();
    }
    defaults
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|model| format!("使用当前模型（{model}）"))
        .unwrap_or_else(|| "使用当前模型".to_string())
}

fn selected_effort_text(defaults: &ThreadCreateDefaults, draft: &ThreadCreateDraftState) -> String {
    if let Some(effort) = draft
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_default_value(value))
    {
        return reasoning_effort_label(effort);
    }
    defaults
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|effort| format!("使用默认推理强度（{}）", reasoning_effort_label(effort)))
        .unwrap_or_else(|| "使用模型默认推理强度".to_string())
}

fn selected_permission_text(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> String {
    if let Some(permission) = draft
        .permission
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_default_value(value))
    {
        return permission_label(permission);
    }
    defaults
        .permission
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|permission| format!("使用 Codex App 当前权限（{permission}）"))
        .unwrap_or_else(|| "使用 Codex App 当前权限".to_string())
}

fn project_option_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn reasoning_effort_label(effort: &str) -> String {
    match effort.trim() {
        "none" => "无 (none)".to_string(),
        "minimal" => "极低 (minimal)".to_string(),
        "low" => "低 (low)".to_string(),
        "medium" => "中 (medium)".to_string(),
        "high" => "高 (high)".to_string(),
        "xhigh" => "超高 (xhigh)".to_string(),
        other => other.to_string(),
    }
}

fn permission_label(permission: &str) -> String {
    match permission.trim() {
        "workspace_user" | "default" | "default_permissions" | "auto" => "默认权限".to_string(),
        "auto_review" | "guardian-approvals" | "guardian_approvals" => "自动审查".to_string(),
        "full_access" | "full-access" => "完全访问权限".to_string(),
        other => other.to_string(),
    }
}

fn summarize_thread_route_state(
    thread: &serde_json::Value,
    loaded_set: &std::collections::HashSet<String>,
    current_thread_id: Option<&str>,
) -> String {
    let thread_id = thread
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if current_thread_id == Some(thread_id) {
        return "当前会话".to_string();
    }
    if loaded_set.contains(thread_id) {
        return "已加载，可接入".to_string();
    }
    "历史会话，可接入".to_string()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_optional_selection(value: Option<String>) -> Option<String> {
    normalize_optional_text(value).filter(|value| value != "__default__")
}

fn normalize_reasoning_effort(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = normalize_optional_selection(value) else {
        return Ok(None);
    };
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "default" | "默认" => Ok(None),
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" => Ok(Some(normalized)),
        _ => Err(anyhow!(
            "推理强度只支持 none / minimal / low / medium / high / xhigh"
        )),
    }
}

fn permission_mode_to_thread_start(
    value: Option<String>,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let Some(value) = normalize_optional_selection(value) else {
        return Ok((None, None, None));
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "workspace_user" | "default" | "default_permissions" | "auto" => Ok((
            Some(":workspace".to_string()),
            Some("on-request".to_string()),
            Some("user".to_string()),
        )),
        "auto_review" | "guardian-approvals" | "guardian_approvals" => Ok((
            Some(":workspace".to_string()),
            Some("on-request".to_string()),
            Some("auto_review".to_string()),
        )),
        "full_access" | "full-access" => Ok((
            Some(":danger-full-access".to_string()),
            Some("never".to_string()),
            Some("user".to_string()),
        )),
        _ => Err(anyhow!("权限只支持 默认权限 / 自动审查 / 完全访问权限")),
    }
}

fn normalize_thread_cwd(choice: Option<String>, custom: Option<String>) -> Result<Option<String>> {
    let choice = normalize_optional_text(choice);
    let custom = normalize_optional_text(custom);
    if choice.as_deref() == Some("__custom__") && custom.is_none() {
        return Err(anyhow!("选择自定义目录时需要填写绝对路径"));
    }
    let Some(value) = custom.or_else(|| match choice.as_deref() {
        Some("__default__" | "__custom__") | None => None,
        Some(_) => choice,
    }) else {
        return Ok(None);
    };
    let expanded = expand_home_prefix(&value);
    let path = PathBuf::from(expanded);
    if !path.is_absolute() {
        return Err(anyhow!("项目目录必须是绝对路径，或留空不指定目录"));
    }
    if path.exists() && !path.is_dir() {
        return Err(anyhow!("项目目录不是文件夹：{}", path.display()));
    }
    if !path.exists() {
        std::fs::create_dir_all(&path)
            .with_context(|| format!("创建项目目录失败：{}", path.display()))?;
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("读取项目目录失败：{}", path.display()))?;
    Ok(Some(canonical.to_string_lossy().to_string()))
}

pub(crate) fn expand_home_prefix(value: &str) -> PathBuf {
    if value == "~" {
        if let Some(home) = user_home_dir() {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = user_home_dir() {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(value)
}

fn thread_start_permission_label(options: &remote_control_backend::ThreadStartOptions) -> String {
    match (
        options.permissions.as_deref(),
        options.approval_policy.as_deref(),
        options.approvals_reviewer.as_deref(),
    ) {
        (Some(":workspace"), Some("on-request"), Some("user")) => "默认权限".to_string(),
        (Some(":workspace"), Some("on-request"), Some("auto_review" | "guardian_subagent")) => {
            "自动审查".to_string()
        }
        (Some(":danger-full-access"), Some("never"), Some("user")) => "完全访问权限".to_string(),
        (None, None, None) => "使用 Codex App 默认值".to_string(),
        _ => "自定义".to_string(),
    }
}

async fn load_model_catalog_for_client(
    state: &SharedState,
    client_key: &str,
) -> Result<Vec<ThreadModelCatalogEntry>> {
    let response =
        remote_control_backend::model_list_for_client(state, client_key, true, Some(100)).await?;
    Ok(parse_model_catalog(&response))
}

fn parse_model_catalog(response: &serde_json::Value) -> Vec<ThreadModelCatalogEntry> {
    response
        .get("data")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let model = value
                .get("model")
                .or_else(|| value.get("id"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            let display_name = value
                .get("displayName")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&model);
            let label = if display_name == model {
                model.clone()
            } else {
                format!("{display_name} ({model})")
            };
            let supported_efforts = value
                .get("supportedReasoningEfforts")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| {
                    value
                        .get("reasoningEffort")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            Some(ThreadModelCatalogEntry {
                model,
                label,
                hidden: value
                    .get("hidden")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
                is_default: value
                    .get("isDefault")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
                supported_efforts: dedupe_strings(supported_efforts),
                default_effort: value
                    .get("defaultReasoningEffort")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            })
        })
        .collect()
}

fn thread_model_choices(
    current: Option<&str>,
    catalog: &[ThreadModelCatalogEntry],
) -> Vec<ThreadModelChoice> {
    let mut models = Vec::new();
    if let Some(current) = current.map(str::trim).filter(|value| !value.is_empty()) {
        let label = catalog
            .iter()
            .find(|entry| entry.model == current)
            .map(|entry| entry.label.clone())
            .unwrap_or_else(|| current.to_string());
        push_model_choice(&mut models, label, current.to_string());
    }
    for entry in catalog
        .iter()
        .filter(|entry| !entry.hidden || Some(entry.model.as_str()) == current)
    {
        push_model_choice(&mut models, entry.label.clone(), entry.model.clone());
    }
    for model in ["gpt-5.5", "gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex"] {
        push_model_choice(&mut models, model.to_string(), model.to_string());
    }
    models
}

fn push_model_choice(models: &mut Vec<ThreadModelChoice>, label: String, value: String) {
    if !value.trim().is_empty() && !models.iter().any(|existing| existing.value == value) {
        models.push(ThreadModelChoice { label, value });
    }
}

fn thread_reasoning_effort_choices(
    current_model: Option<&str>,
    catalog: &[ThreadModelCatalogEntry],
    current_effort: Option<&str>,
) -> Vec<String> {
    let mut efforts = Vec::new();
    if let Some(current_effort) = current_effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        efforts.push(current_effort.to_string());
    }
    if let Some(entry) =
        current_model.and_then(|model| catalog.iter().find(|entry| entry.model == model))
    {
        efforts.extend(entry.supported_efforts.iter().cloned());
    }
    for entry in catalog.iter().filter(|entry| !entry.hidden) {
        efforts.extend(entry.supported_efforts.iter().cloned());
    }
    if efforts.is_empty() {
        efforts.extend(["minimal", "low", "medium", "high", "xhigh"].map(str::to_string));
    }
    sort_reasoning_efforts(dedupe_strings(efforts))
}

async fn validate_reasoning_effort_for_model_for_client(
    state: &SharedState,
    client_key: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<()> {
    let (Some(model), Some(effort)) = (model, effort) else {
        return Ok(());
    };
    let Ok(catalog) = load_model_catalog_for_client(state, client_key).await else {
        return Ok(());
    };
    let Some(entry) = catalog.iter().find(|entry| entry.model == model) else {
        return Ok(());
    };
    if entry.supported_efforts.is_empty()
        || entry
            .supported_efforts
            .iter()
            .any(|supported| supported == effort)
    {
        return Ok(());
    }
    Err(anyhow!(
        "模型 `{model}` 不支持推理强度 `{effort}`，可选：{}",
        entry.supported_efforts.join(" / ")
    ))
}

fn sort_reasoning_efforts(mut efforts: Vec<String>) -> Vec<String> {
    efforts.sort_by_key(|effort| reasoning_effort_rank(effort));
    efforts
}

fn reasoning_effort_rank(effort: &str) -> usize {
    match effort {
        "none" => 0,
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" => 5,
        _ => 100,
    }
}

fn dedupe_strings(values: Vec<String>) -> Vec<String> {
    let mut output = Vec::new();
    for value in values {
        if !output.iter().any(|existing| existing == &value) {
            output.push(value);
        }
    }
    output
}

fn config_string(config: Option<&serde_json::Value>, key: &str) -> Option<String> {
    config
        .and_then(|config| config.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn local_config_string(doc: Option<&toml::Value>, key: &str) -> Option<String> {
    doc.and_then(|doc| doc.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn infer_permission_label(config: Option<&serde_json::Value>) -> Option<String> {
    let sandbox = config_string(config, "sandbox_mode");
    let approval = config_string(config, "approval_policy");
    let reviewer = config_string(config, "approvals_reviewer");
    match (sandbox.as_deref(), approval.as_deref(), reviewer.as_deref()) {
        (Some("danger-full-access"), Some("never") | None, _) => Some("完全访问权限".to_string()),
        (Some("workspace-write"), _, Some("auto_review" | "guardian_subagent")) => {
            Some("自动审查".to_string())
        }
        (Some("workspace-write") | None, Some("on-request") | None, _) => {
            Some("默认权限".to_string())
        }
        (Some("read-only"), _, _) => Some("只读".to_string()),
        _ => None,
    }
}

fn codex_project_paths(doc: Option<&toml::Value>) -> Vec<String> {
    let mut projects = doc
        .and_then(|doc| doc.get("projects"))
        .and_then(|value| value.as_table())
        .map(|table| {
            table
                .keys()
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Ok(cwd) = std::env::current_dir() {
        let cwd = cwd.to_string_lossy().to_string();
        if !cwd.trim().is_empty() {
            projects.push(cwd);
        }
    }
    projects.sort();
    projects.dedup();
    projects
}

fn load_codex_app_config_doc() -> Option<toml::Value> {
    for path in codex_config_candidate_paths() {
        if let Ok(raw) = std::fs::read_to_string(path)
            && let Ok(doc) = raw.parse::<toml::Value>()
        {
            return Some(doc);
        }
    }
    None
}

fn codex_config_candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_path_once(
        &mut paths,
        codex_app_config::default_codex_home().join("config.toml"),
    );
    for home in home_env_candidates() {
        push_path_once(
            &mut paths,
            Path::new(&home).join(".codex").join("config.toml"),
        );
    }
    paths
}

fn home_env_candidates() -> Vec<std::ffi::OsString> {
    let keys = if cfg!(windows) {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    };
    keys.into_iter()
        .filter_map(std::env::var_os)
        .collect::<Vec<_>>()
}

fn user_home_dir() -> Option<std::ffi::OsString> {
    home_env_candidates().into_iter().next()
}

fn push_path_once(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::build_thread_entries;

    #[test]
    fn thread_entries_preserve_history_order() {
        let history_threads = vec![
            json!({"id": "thread-z", "name": "Newest"}),
            json!({"id": "thread-a", "name": "Loaded"}),
            json!({"id": "thread-m", "name": "Current"}),
        ];
        let entries = build_thread_entries(
            &["thread-a".to_string()],
            &history_threads,
            Some("thread-m"),
        );

        let ids = entries
            .iter()
            .map(|entry| entry.thread_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["thread-z", "thread-a", "thread-m"]);
        assert!(entries[1].state.contains("已加载"));
        assert!(entries[2].state.contains("当前会话"));
    }
}
