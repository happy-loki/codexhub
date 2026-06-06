use std::{
    cell::RefCell,
    process::Command,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use reqwest::blocking::Client;
use serde::Deserialize;
use wxdragon::{prelude::*, timer::Timer};

use super::daemon::hide_command_window;
use super::text::GuiText;
use super::{
    FrameTimerStore, GuiTimers, UPDATE_CHECK_TIMEOUT, UPDATE_MANIFEST_URL, UPDATE_RELEASE_API_URL,
    UPDATE_RELEASE_PAGE_URL,
};
use super::{confirm_open_update_release, show_error, show_info};

#[derive(Debug)]
struct LatestReleaseInfo {
    version: String,
    release_url: String,
    notes: Option<String>,
}

#[derive(Debug)]
enum UpdateCheckOutcome {
    Newer {
        current_version: String,
        latest_version: String,
        release_url: String,
        notes: Option<String>,
    },
    Current {
        current_version: String,
        latest_version: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateManifest {
    version: String,
    #[serde(default, alias = "release_url", alias = "html_url")]
    release_url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
}

pub(super) fn check_for_updates_async(
    frame: &Frame,
    gui_timers: &GuiTimers,
    text: GuiText,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        show_info(frame, text.checking_updates_busy());
        return;
    }

    let result: Arc<Mutex<Option<Result<UpdateCheckOutcome, String>>>> = Arc::new(Mutex::new(None));
    {
        let result = result.clone();
        thread::spawn(move || {
            let update = check_for_updates(text);
            if let Ok(mut slot) = result.lock() {
                slot.replace(update);
            }
        });
    }

    let update_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let update_timer = Timer::new(frame);
    {
        let frame = *frame;
        let in_flight = in_flight.clone();
        let update_timer_store = update_timer_store.clone();
        update_timer.on_tick(move |_| {
            let update = result.lock().ok().and_then(|mut slot| slot.take());
            let Some(update) = update else {
                return;
            };

            if let Some(timer) = update_timer_store.borrow().as_ref() {
                timer.stop();
            }
            in_flight.store(false, Ordering::SeqCst);
            show_update_check_result(&frame, text, update);
        });
    }
    update_timer.start(100, false);
    update_timer_store.borrow_mut().replace(update_timer);
    gui_timers.track(&update_timer_store);
}

fn check_for_updates(text: GuiText) -> Result<UpdateCheckOutcome, String> {
    let client = Client::builder()
        .connect_timeout(UPDATE_CHECK_TIMEOUT)
        .timeout(UPDATE_CHECK_TIMEOUT)
        .build()
        .map_err(|err| text.update_client_failed(&err.to_string()))?;

    let release = fetch_update_manifest(text, &client).or_else(|manifest_err| {
        fetch_github_latest_release(text, &client)
            .map_err(|api_err| text.update_sources_failed(&api_err, &manifest_err))
    })?;
    build_update_check_outcome(text, release)
}

fn fetch_update_manifest(text: GuiText, client: &Client) -> Result<LatestReleaseInfo, String> {
    let body = fetch_update_text(text, client, UPDATE_MANIFEST_URL)?;
    let manifest: UpdateManifest = serde_json::from_str(&body)
        .map_err(|err| text.update_manifest_parse_failed(&err.to_string()))?;
    let release_url = manifest
        .release_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| UPDATE_RELEASE_PAGE_URL.to_string());
    Ok(LatestReleaseInfo {
        version: manifest.version,
        release_url,
        notes: manifest.notes,
    })
}

fn fetch_github_latest_release(
    text: GuiText,
    client: &Client,
) -> Result<LatestReleaseInfo, String> {
    let body = fetch_update_text(text, client, UPDATE_RELEASE_API_URL)?;
    let release: GitHubRelease = serde_json::from_str(&body)
        .map_err(|err| text.github_release_parse_failed(&err.to_string()))?;
    Ok(LatestReleaseInfo {
        version: release.tag_name,
        release_url: release.html_url,
        notes: release.body,
    })
}

fn fetch_update_text(text: GuiText, client: &Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .header("User-Agent", "codex-remote")
        .header("Accept", "application/json")
        .send()
        .map_err(|err| {
            let is_timeout = err.is_timeout();
            let err = err.to_string();
            if is_timeout {
                text.url_request_timeout(url, &err)
            } else {
                text.url_request_failed(url, &err)
            }
        })?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|err| text.url_request_failed(url, &err.to_string()))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(text.url_http_failed(url, &status.to_string(), &body))
    }
}

fn build_update_check_outcome(
    text: GuiText,
    release: LatestReleaseInfo,
) -> Result<UpdateCheckOutcome, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = release.version.trim().to_string();
    if latest_version.is_empty() {
        return Err(text.release_missing_version().to_string());
    }

    if is_version_newer(text, &latest_version, &current_version)? {
        Ok(UpdateCheckOutcome::Newer {
            current_version,
            latest_version,
            release_url: release.release_url,
            notes: release.notes,
        })
    } else {
        Ok(UpdateCheckOutcome::Current {
            current_version,
            latest_version,
        })
    }
}

fn show_update_check_result(
    parent: &Frame,
    text: GuiText,
    result: Result<UpdateCheckOutcome, String>,
) {
    match result {
        Ok(UpdateCheckOutcome::Current {
            current_version,
            latest_version,
        }) => {
            show_info(
                parent,
                &text.already_latest_version(&current_version, &latest_version),
            );
        }
        Ok(UpdateCheckOutcome::Newer {
            current_version,
            latest_version,
            release_url,
            notes,
        }) => {
            let notes = update_notes_for_dialog(text, notes.as_deref());
            let message = text.new_version_message(&current_version, &latest_version, &notes);
            if confirm_open_update_release(parent, text, &message) {
                if let Err(err) = open_url_in_browser(text, &release_url) {
                    show_error(parent, &err);
                }
            }
        }
        Err(err) => {
            show_error(parent, &text.update_failed(&err));
        }
    }
}

fn update_notes_for_dialog(text: GuiText, notes: Option<&str>) -> String {
    let notes = notes.unwrap_or_default().trim();
    if notes.is_empty() {
        return text.release_notes_default().to_string();
    }
    text.release_notes(&truncate_for_dialog(notes, 700))
}

fn truncate_for_dialog(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut result = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        result.push_str("\n...");
    }
    result
}

fn is_version_newer(text: GuiText, latest: &str, current: &str) -> Result<bool, String> {
    let latest = parse_version_segments(text, latest)?;
    let current = parse_version_segments(text, current)?;
    for index in 0..latest.len().max(current.len()) {
        let latest_segment = latest.get(index).copied().unwrap_or_default();
        let current_segment = current.get(index).copied().unwrap_or_default();
        if latest_segment != current_segment {
            return Ok(latest_segment > current_segment);
        }
    }
    Ok(false)
}

fn parse_version_segments(text: GuiText, version: &str) -> Result<Vec<u64>, String> {
    let normalized = version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let segments = normalized
        .split('.')
        .map(|segment| {
            segment
                .parse::<u64>()
                .map_err(|_| text.version_not_comparable(version))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if segments.is_empty() {
        Err(text.version_not_comparable(version))
    } else {
        Ok(segments)
    }
}

#[cfg(test)]
mod update_tests {
    use super::super::text::{GuiLocale, GuiText};
    use super::*;

    #[test]
    fn compares_release_versions() {
        let text = GuiText::new(GuiLocale::EnUs);
        assert!(is_version_newer(text, "v0.2.6", "0.2.5").unwrap());
        assert!(is_version_newer(text, "0.3.0", "0.2.99").unwrap());
        assert!(!is_version_newer(text, "v0.2.5", "0.2.5").unwrap());
        assert!(!is_version_newer(text, "v0.2.4", "0.2.5").unwrap());
        assert!(!is_version_newer(text, "v0.2.5-beta.1", "0.2.5").unwrap());
    }
}

fn open_url_in_browser(text: GuiText, url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err(text.empty_download_url().to_string());
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        hide_command_window(&mut command);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|err| text.open_browser_failed(&err.to_string(), url))
}
