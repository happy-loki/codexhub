use super::common::build_markdown_card;
use super::markdown::normalize_card_markdown;

#[allow(dead_code)]
pub fn build_oauth_device_card(
    verification_uri: &str,
    verification_uri_complete: &str,
    user_code: &str,
    expires_in: u64,
    scope_lines: &[String],
) -> serde_json::Value {
    let expires_minutes = ((expires_in as f64) / 60.0).ceil() as u64;
    let permission_content = if scope_lines.is_empty() {
        "当前这一步需要补充用户授权。".to_string()
    } else {
        format!(
            "所需权限：\n{}",
            scope_lines
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let auth_url = if !verification_uri_complete.trim().is_empty() {
        verification_uri_complete.trim()
    } else {
        verification_uri.trim()
    };
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": "授权后，应用将能够以你的身份继续执行当前操作。"
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": permission_content
        }),
        serde_json::json!({
            "tag": "column_set",
            "flex_mode": "none",
            "horizontal_align": "right",
            "columns": [
                {
                    "tag": "column",
                    "width": "auto",
                    "elements": [
                        {
                            "tag": "button",
                            "text": {
                                "tag": "plain_text",
                                "content": "前往授权"
                            },
                            "type": "primary",
                            "size": "medium",
                            "multi_url": {
                                "url": auth_url,
                                "pc_url": auth_url,
                                "android_url": auth_url,
                                "ios_url": auth_url
                            }
                        }
                    ]
                }
            ]
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='grey'>授权链接将在 {} 分钟后失效，届时需要重新发起。</font>", expires_minutes.max(1))
        }),
    ];
    if !user_code.trim().is_empty() {
        elements.insert(
            1,
            serde_json::json!({
                "tag": "markdown",
                "content": format!("用户码：`{}`", user_code.trim())
            }),
        );
    }
    let mut card = build_markdown_card("", Some("请授权以继续当前操作"), Some("blue"));
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

#[allow(dead_code)]
pub fn build_oauth_device_explanation(operation_label: &str) -> String {
    format!(
        "这一步需要你先授权一下，我才能继续完成“{}”。\n\n我已经发了一张授权卡，点击里面的按钮完成授权后，回来继续发消息就行。",
        operation_label
    )
}

#[allow(dead_code)]
pub fn build_permission_required_explanation(operation_label: &str) -> String {
    format!(
        "当前这一步要完成“{}”，但飞书应用侧的权限还没开通。\n\n这个不是你操作错了，需要应用管理员先补权限，之后再重试。",
        operation_label
    )
}

#[allow(dead_code)]
pub fn build_permission_required_card(operation_label: &str) -> serde_json::Value {
    let content = format!(
        "当前飞书应用缺少完成“{}”所需的应用权限。\n\n请应用管理员先在飞书开放平台完成权限开通，随后再重试当前操作。",
        operation_label
    );
    let mut card = build_markdown_card(&content, Some("需要补充应用权限"), Some("orange"));
    card["body"]["elements"] = serde_json::json!([
        {
            "tag": "markdown",
            "content": normalize_card_markdown(&content)
        },
        {
            "tag": "hr"
        },
        {
            "tag": "markdown",
            "content": "_管理员完成开通后，重新发起当前操作即可。_"
        }
    ]);
    card
}
