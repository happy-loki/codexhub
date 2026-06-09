use std::{
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::HeaderMap;
use base64::Engine;
use serde_json::Value;

use crate::chain_log;

use super::REMOTE_REQUEST_ID;

pub(in crate::remote_control_backend) fn format_rfc3339_utc(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    let seconds_of_day = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

pub(in crate::remote_control_backend) fn unix_now_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(in crate::remote_control_backend) fn log_remote_control_entry_headers(
    event: &str,
    headers: &HeaderMap,
) {
    let header_names = headers
        .keys()
        .map(|name| name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let x_codex_name_raw = header_str(headers, "x-codex-name").unwrap_or_default();
    let x_codex_name_decoded = if x_codex_name_raw.is_empty() {
        String::new()
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(&x_codex_name_raw)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    };
    chain_log::write_line(format!(
        "[remote_control] event={} header_names={} user_agent={} origin={} referer={} host={} x_codex_protocol_version={} x_codex_server_id={} x_codex_name_raw={} x_codex_name_decoded={} x_codex_installation_id={} chatgpt_account_id={} x_codex_subscribe_cursor={}",
        event,
        header_names,
        header_str(headers, "user-agent").unwrap_or_default(),
        header_str(headers, "origin").unwrap_or_default(),
        header_str(headers, "referer").unwrap_or_default(),
        header_str(headers, "host").unwrap_or_default(),
        header_str(headers, "x-codex-protocol-version").unwrap_or_default(),
        header_str(headers, "x-codex-server-id").unwrap_or_default(),
        x_codex_name_raw,
        x_codex_name_decoded,
        header_str(headers, "x-codex-installation-id").unwrap_or_default(),
        header_str(headers, "chatgpt-account-id").unwrap_or_default(),
        header_str(headers, "x-codex-subscribe-cursor").unwrap_or_default()
    ));
}

pub(in crate::remote_control_backend) fn json_object_keys(value: &Value) -> String {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>().join(","))
        .unwrap_or_default()
}

pub(in crate::remote_control_backend) fn header_str(
    headers: &HeaderMap,
    name: &str,
) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

pub(in crate::remote_control_backend) fn stable_id(prefix: &str, seed: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in seed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{prefix}_{hash:016x}")
}

pub(in crate::remote_control_backend) fn stable_base64url_32(prefix: &str, seed: &str) -> String {
    let mut bytes = [0u8; 32];
    for chunk in 0..4 {
        let id = stable_id(prefix, &format!("{seed}:{chunk}"));
        let hex = id.rsplit('_').next().unwrap_or_default();
        let value = u64::from_str_radix(hex, 16).unwrap_or_default();
        bytes[(chunk * 8)..((chunk + 1) * 8)].copy_from_slice(&value.to_be_bytes());
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(in crate::remote_control_backend) fn uuid_like() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = REMOTE_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("{now:032x}-{counter:016x}")
}
