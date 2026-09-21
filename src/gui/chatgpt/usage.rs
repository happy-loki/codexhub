use super::{AccountUsage, GuiText, label, strip_nul};
use crate::ai_gateway::chatgpt_auth::{UsageLimits, UsageWindow};

fn timestamp(value: u64) -> String {
    // Bound untrusted timestamps before calling the shared calendar formatter.
    if value > 253_402_300_799 {
        return "?".into();
    }
    crate::codex_app_config::format_rfc3339_utc(value)
        .replace('T', " ")
        .replace('Z', " UTC")
}

fn window_name(text: GuiText, seconds: u64) -> String {
    match seconds {
        18_000 => label(text, "5 小时", "5 hours").into(),
        604_800 => label(text, "7 天", "7 days").into(),
        s if s > 0 && s % 86_400 == 0 => format!("{} {}", s / 86_400, label(text, "天", "days")),
        s if s > 0 && s % 3600 == 0 => format!("{} {}", s / 3600, label(text, "小时", "hours")),
        s => format!("{} {}", s, label(text, "秒", "seconds")),
    }
}

fn format_window(text: GuiText, window: &UsageWindow, fetched_at: u64) -> String {
    let name = window_name(text, window.limit_window_seconds);
    let reset = window
        .reset_at
        .and_then(|s| u64::try_from(s).ok())
        .or_else(|| {
            window
                .reset_after_seconds
                .map(|s| fetched_at.saturating_add(s))
        })
        .map(timestamp)
        .unwrap_or_else(|| label(text, "官方未提供", "Not provided").into());
    let used = window.used_percent;
    if !used.is_finite() || used < 0.0 {
        return format!("{name}: {}", label(text, "用量未知", "Usage unknown"));
    }
    format!(
        "{name}: {} {:.1}% ({} {:.1}%)\n  {}: {reset}",
        label(text, "剩余", "remaining"),
        (100.0 - used).clamp(0.0, 100.0),
        label(text, "已用", "used"),
        used,
        label(text, "重置时间", "Resets")
    )
}

fn format_limits(
    text: GuiText,
    limits: Option<&UsageLimits>,
    fetched_at: u64,
    ordinary: bool,
) -> Vec<String> {
    let mut windows: Vec<_> = limits
        .into_iter()
        .flat_map(|l| [&l.primary_window, &l.secondary_window])
        .flatten()
        .collect();
    windows.sort_by_key(|w| w.limit_window_seconds);
    let mut lines: Vec<_> = windows
        .iter()
        .map(|w| format_window(text, w, fetched_at))
        .collect();
    if ordinary {
        for seconds in [18_000, 604_800] {
            if !windows.iter().any(|w| w.limit_window_seconds == seconds) {
                lines.push(format!(
                    "{}: {}",
                    window_name(text, seconds),
                    label(text, "官方未提供", "Not provided")
                ));
            }
        }
    } else if windows.is_empty() {
        lines.push(label(text, "额度详情：官方未提供", "Quota details: not provided").into());
    }
    lines
}

pub(super) fn format_usage(text: GuiText, snapshot: &AccountUsage) -> String {
    let usage = &snapshot.usage;
    let state = match usage.rate_limit.as_ref().and_then(|l| l.allowed) {
        Some(true) => label(text, "可用", "Available"),
        Some(false) => label(text, "当前额度不可用", "Current quota unavailable"),
        None => label(text, "官方未提供", "Not provided"),
    };
    let mut lines = vec![
        format!(
            "{}: {}  |  {}: {state}",
            label(text, "套餐", "Plan"),
            strip_nul(&usage.plan_type),
            label(text, "状态", "Status")
        ),
        label(
            text,
            "套餐到期：官方未提供",
            "Plan expiry: not provided by OpenAI",
        )
        .into(),
    ];
    lines.extend(format_limits(
        text,
        usage.rate_limit.as_ref(),
        snapshot.fetched_at,
        true,
    ));
    if let Some(credits) = &usage.credits {
        let balance = if credits.unlimited {
            label(text, "不限量", "Unlimited").into()
        } else if let Some(balance) = &credits.balance {
            strip_nul(balance)
        } else if !credits.has_credits {
            label(text, "无", "None").into()
        } else {
            label(
                text,
                "有余额，官方未提供数量",
                "Available; amount not provided",
            )
            .into()
        };
        lines.push(format!(
            "{}: {balance}",
            label(text, "额外点数", "Additional credits")
        ));
    }
    for additional in usage.additional_rate_limits.iter().flatten() {
        lines.push(format!("\n{}", strip_nul(&additional.limit_name)));
        lines.extend(format_limits(
            text,
            additional.rate_limit.as_ref(),
            snapshot.fetched_at,
            false,
        ));
    }
    lines.push(format!(
        "\n{}: {}",
        label(text, "查询时间", "Checked"),
        timestamp(snapshot.fetched_at)
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::text::GuiLocale;
    use serde_json::json;

    #[test]
    fn displays_actual_periods_and_never_invents_plan_expiry_or_missing_quotas() {
        let snapshot: AccountUsage = serde_json::from_value(json!({
            "account":{"authId":"test","email":null,"plan":"plus","needsLogin":false,"canRefresh":true},
            "fetchedAt":0,
            "usage":{"plan_type":"plus","rate_limit":{
                "allowed":true,"primary_window":{"used_percent":25.5,"limit_window_seconds":604800,"reset_at":86400},
                "secondary_window":{"used_percent":60,"limit_window_seconds":18000,"reset_after_seconds":3600}
            }}
        })).unwrap();
        let output = format_usage(GuiText::new(GuiLocale::EnUs), &snapshot);
        assert!(output.contains("5 hours: remaining 40.0% (used 60.0%)"));
        assert!(output.contains("7 days: remaining 74.5% (used 25.5%)"));
        assert!(output.contains("1970-01-01 01:00:00 UTC"));
        assert!(output.contains("1970-01-02 00:00:00 UTC"));
        assert!(output.contains("Plan expiry: not provided"));
        let mut unknown = snapshot;
        unknown.usage.rate_limit = None;
        let output = format_usage(GuiText::new(GuiLocale::ZhCn), &unknown);
        assert!(output.contains("5 小时: 官方未提供"));
        assert!(output.contains("7 天: 官方未提供"));
        assert!(!output.contains("100.0%"));
    }
}
