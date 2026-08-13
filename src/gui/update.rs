use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;
use wxdragon::{prelude::*, timer::Timer};

#[cfg(target_os = "windows")]
use super::daemon::hide_command_window;
use super::text::GuiText;
use super::{
    FrameTimerStore, GuiTimers, LEGACY_UPDATE_MANIFEST_URL, UPDATE_CHECK_TIMEOUT,
    UPDATE_MANIFEST_URL, UPDATE_RELEASE_PAGE_URL, apply_outbound_blocking_proxy,
};
use super::{confirm_open_update_release, show_error, show_info};

const DOWNLOAD_PROGRESS_RESOLUTION: i32 = 1000;

#[derive(Debug)]
struct LatestReleaseInfo {
    version: String,
    release_url: String,
    download: Option<UpdateDownload>,
    notes: Option<String>,
}

#[derive(Debug)]
enum UpdateCheckOutcome {
    Newer {
        current_version: String,
        latest_version: String,
        release_url: String,
        download: Option<UpdateDownload>,
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
    #[serde(default)]
    assets: BTreeMap<String, UpdateAsset>,
}

#[derive(Deserialize)]
struct UpdateAsset {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default, rename = "type")]
    asset_type: Option<String>,
}

#[derive(Clone, Debug)]
struct UpdateDownload {
    url: String,
    sha256: Option<String>,
    asset_type: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdatePlatform {
    Windows,
    Macos,
    Linux,
}

#[derive(Debug)]
struct DownloadedUpdate {
    path: PathBuf,
    url: String,
}

#[derive(Default)]
struct DownloadProgress {
    downloaded: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
    result: Mutex<Option<Result<DownloadedUpdate, String>>>,
}

impl DownloadProgress {
    fn snapshot(&self) -> (u64, u64) {
        (
            self.downloaded.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn take_result(&self) -> Option<Result<DownloadedUpdate, String>> {
        self.result.lock().ok().and_then(|mut slot| slot.take())
    }
}

pub(super) fn check_for_updates_async(
    frame: &Frame,
    gui_timers: &GuiTimers,
    text: GuiText,
    in_flight: &Arc<AtomicBool>,
    quitting: &Rc<AtomicBool>,
) {
    check_for_updates_async_impl(frame, gui_timers, text, in_flight, quitting, false);
}

pub(super) fn check_for_updates_silent_async(
    frame: &Frame,
    gui_timers: &GuiTimers,
    text: GuiText,
    in_flight: &Arc<AtomicBool>,
    quitting: &Rc<AtomicBool>,
) {
    check_for_updates_async_impl(frame, gui_timers, text, in_flight, quitting, true);
}

fn check_for_updates_async_impl(
    frame: &Frame,
    gui_timers: &GuiTimers,
    text: GuiText,
    in_flight: &Arc<AtomicBool>,
    quitting: &Rc<AtomicBool>,
    silent_if_not_newer: bool,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        if !silent_if_not_newer {
            show_info(frame, text.checking_updates_busy());
        }
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
        let gui_timers = gui_timers.clone();
        let in_flight = in_flight.clone();
        let quitting = quitting.clone();
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
            show_update_check_result(
                &frame,
                &gui_timers,
                text,
                update,
                &quitting,
                silent_if_not_newer,
            );
        });
    }
    update_timer.start(100, false);
    update_timer_store.borrow_mut().replace(update_timer);
    gui_timers.track(&update_timer_store);
}

fn check_for_updates(text: GuiText) -> Result<UpdateCheckOutcome, String> {
    let client = apply_outbound_blocking_proxy(
        Client::builder()
            .connect_timeout(UPDATE_CHECK_TIMEOUT)
            .timeout(UPDATE_CHECK_TIMEOUT),
    )?
    .build()
    .map_err(|err| text.update_client_failed(&err.to_string()))?;

    let release = fetch_update_manifest(text, &client, UPDATE_MANIFEST_URL).or_else(
        |platform_manifest_err| {
            fetch_update_manifest(text, &client, LEGACY_UPDATE_MANIFEST_URL).map_err(
                |legacy_manifest_err| {
                    text.update_sources_failed(&platform_manifest_err, &legacy_manifest_err)
                },
            )
        },
    )?;
    build_update_check_outcome(text, release)
}

fn fetch_update_manifest(
    text: GuiText,
    client: &Client,
    url: &str,
) -> Result<LatestReleaseInfo, String> {
    let body = fetch_update_text(text, client, url)?;
    let manifest: UpdateManifest = serde_json::from_str(&body)
        .map_err(|err| text.update_manifest_parse_failed(&format!("{url}: {}", err)))?;
    let release_url = manifest
        .release_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(UPDATE_RELEASE_PAGE_URL)
        .to_string();
    let download = platform_download(&manifest);
    Ok(LatestReleaseInfo {
        version: manifest.version,
        release_url,
        download,
        notes: manifest.notes,
    })
}

fn fetch_update_text(text: GuiText, client: &Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .header("User-Agent", "codexhub")
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
            download: release.download,
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
    gui_timers: &GuiTimers,
    text: GuiText,
    result: Result<UpdateCheckOutcome, String>,
    quitting: &Rc<AtomicBool>,
    silent_if_not_newer: bool,
) {
    match result {
        Ok(UpdateCheckOutcome::Current {
            current_version,
            latest_version,
        }) => {
            if !silent_if_not_newer {
                show_info(
                    parent,
                    &text.already_latest_version(&current_version, &latest_version),
                );
            }
        }
        Ok(UpdateCheckOutcome::Newer {
            current_version,
            latest_version,
            release_url,
            download,
            notes,
        }) => {
            let notes = update_notes_for_dialog(text, notes.as_deref());
            let message = text.new_version_message(
                &current_version,
                &latest_version,
                &notes,
                download.is_some(),
            );
            if confirm_open_update_release(parent, text, &message) {
                if let Some(download) = download {
                    download_and_install_update_async(parent, gui_timers, text, download, quitting);
                } else if let Err(err) = open_url_in_browser(text, &release_url) {
                    show_error(parent, &err);
                }
            }
        }
        Err(err) => {
            if !silent_if_not_newer {
                show_error(parent, &text.update_failed(&err));
            }
        }
    }
}

fn update_notes_for_dialog(text: GuiText, notes: Option<&str>) -> String {
    let notes = notes.unwrap_or_default().trim();
    if notes.is_empty() {
        return text.release_notes_default().to_string();
    }
    text.release_notes(&compact_update_notes(notes))
}

fn compact_update_notes(notes: &str) -> String {
    const MAX_LINES: usize = 4;
    const MAX_CHARS: usize = 280;

    let compact = notes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    truncate_for_dialog(&compact, MAX_CHARS)
}

impl UpdateDownload {
    fn from_asset(asset: &UpdateAsset) -> Option<Self> {
        let url = asset.url.as_deref()?.trim();
        if url.is_empty() {
            return None;
        }
        Some(Self {
            url: url.to_string(),
            sha256: asset
                .sha256
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_ascii_lowercase()),
            asset_type: asset
                .asset_type
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
        })
    }
}

fn platform_download(manifest: &UpdateManifest) -> Option<UpdateDownload> {
    platform_download_for_platform(current_update_platform(), manifest)
}

fn platform_download_for_platform(
    platform: UpdatePlatform,
    manifest: &UpdateManifest,
) -> Option<UpdateDownload> {
    manifest
        .assets
        .get(platform_manifest_asset_key(platform))
        .or_else(|| {
            manifest
                .assets
                .get(platform_manifest_fallback_asset_key(platform))
        })
        .and_then(UpdateDownload::from_asset)
}

#[cfg(target_os = "windows")]
fn current_update_platform() -> UpdatePlatform {
    UpdatePlatform::Windows
}

#[cfg(target_os = "macos")]
fn current_update_platform() -> UpdatePlatform {
    UpdatePlatform::Macos
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn current_update_platform() -> UpdatePlatform {
    UpdatePlatform::Linux
}

fn platform_manifest_asset_key(platform: UpdatePlatform) -> &'static str {
    match platform {
        UpdatePlatform::Windows => "windows-x86_64",
        UpdatePlatform::Macos => "macos-universal",
        UpdatePlatform::Linux => "linux-x86_64",
    }
}

fn platform_manifest_fallback_asset_key(platform: UpdatePlatform) -> &'static str {
    match platform {
        UpdatePlatform::Windows => "windows-portable-x86_64",
        UpdatePlatform::Macos => "macos-sparkle-universal",
        UpdatePlatform::Linux => "linux-appimage-x86_64",
    }
}

fn download_and_install_update_async(
    parent: &Frame,
    gui_timers: &GuiTimers,
    text: GuiText,
    download: UpdateDownload,
    quitting: &Rc<AtomicBool>,
) {
    let progress = Arc::new(DownloadProgress::default());
    {
        let progress = progress.clone();
        thread::spawn(move || {
            let update = download_update(text, &download, &progress).and_then(|update| {
                if progress.is_cancelled() {
                    return Err(text.update_download_cancelled().to_string());
                }
                launch_downloaded_update(text, &update, true)?;
                Ok(update)
            });
            if let Ok(mut slot) = progress.result.lock() {
                slot.replace(update);
            }
        });
    }

    let dialog = ProgressDialog::builder(
        parent,
        text.update_download_title(),
        text.update_download_preparing(),
        DOWNLOAD_PROGRESS_RESOLUTION,
    )
    .can_abort()
    .smooth()
    .build();

    let timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let timer = Timer::new(parent);
    {
        let parent = *parent;
        let quitting = quitting.clone();
        let timer_store = timer_store.clone();
        timer.on_tick(move |_| {
            // User pressed Cancel on the progress dialog: tell the worker to stop.
            if dialog.was_cancelled() {
                progress.cancel.store(true, Ordering::Relaxed);
            }

            if let Some(result) = progress.take_result() {
                if let Some(timer) = timer_store.borrow().as_ref() {
                    timer.stop();
                }
                dialog.update(DOWNLOAD_PROGRESS_RESOLUTION, None);
                match result {
                    Ok(_update) => {
                        show_info(&parent, text.update_installer_started());
                        if should_quit_after_update_launch() {
                            quitting.store(true, Ordering::SeqCst);
                            parent.close(true);
                        }
                    }
                    Err(err) => {
                        if progress.is_cancelled() {
                            show_info(&parent, text.update_download_cancelled());
                        } else {
                            show_error(&parent, &text.update_failed(&err));
                        }
                    }
                }
                return;
            }

            let (downloaded, total) = progress.snapshot();
            if total > 0 {
                let ratio = (downloaded as f64 / total as f64).clamp(0.0, 1.0);
                let value = (ratio * DOWNLOAD_PROGRESS_RESOLUTION as f64).round() as i32;
                let message = text.update_download_progress(
                    &format_download_bytes(downloaded),
                    &format_download_bytes(total),
                );
                dialog.update(value.min(DOWNLOAD_PROGRESS_RESOLUTION - 1), Some(&message));
            } else {
                // Server did not report Content-Length: keep the bar pulsing.
                let message =
                    text.update_download_progress_unknown(&format_download_bytes(downloaded));
                dialog.pulse(Some(&message));
            }
        });
    }
    timer.start(150, false);
    timer_store.borrow_mut().replace(timer);
    gui_timers.track(&timer_store);
}

fn download_update(
    text: GuiText,
    download: &UpdateDownload,
    progress: &DownloadProgress,
) -> Result<DownloadedUpdate, String> {
    let url = download.url.trim();
    if url.is_empty() {
        return Err(text.empty_download_url().to_string());
    }

    let client = apply_outbound_blocking_proxy(
        Client::builder()
            .connect_timeout(UPDATE_CHECK_TIMEOUT)
            .timeout(None),
    )?
    .build()
    .map_err(|err| text.update_client_failed(&err.to_string()))?;
    let mut response = client
        .get(url)
        .header("User-Agent", "codexhub")
        .send()
        .map_err(|err| text.update_download_failed(url, &err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(text.update_download_http_failed(url, &status.to_string()));
    }

    if let Some(total) = response.content_length() {
        progress.total.store(total, Ordering::Relaxed);
    }

    let target_path = update_download_path(url, download.asset_type.as_deref())?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            text.update_download_failed(&parent.display().to_string(), &err.to_string())
        })?;
    }

    let mut file = fs::File::create(&target_path).map_err(|err| {
        text.update_download_failed(&target_path.display().to_string(), &err.to_string())
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    loop {
        if progress.is_cancelled() {
            drop(file);
            let _ = fs::remove_file(&target_path);
            return Err(text.update_download_cancelled().to_string());
        }
        let read = response
            .read(&mut buffer)
            .map_err(|err| text.update_download_failed(url, &err.to_string()))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|err| {
            text.update_download_failed(&target_path.display().to_string(), &err.to_string())
        })?;
        hasher.update(&buffer[..read]);
        downloaded += read as u64;
        progress.downloaded.store(downloaded, Ordering::Relaxed);
    }

    let actual_sha256 = format!("{:x}", hasher.finalize());
    if let Some(expected_sha256) = &download.sha256
        && !actual_sha256.eq_ignore_ascii_case(expected_sha256)
    {
        return Err(text.update_checksum_mismatch(expected_sha256, &actual_sha256));
    }

    Ok(DownloadedUpdate {
        path: target_path,
        url: url.to_string(),
    })
}

fn format_download_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= MIB {
        format!("{:.1} MB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.0} KB", bytes_f / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn update_download_path(url: &str, asset_type: Option<&str>) -> Result<PathBuf, String> {
    let filename = Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back().map(ToOwned::to_owned))
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| default_update_filename(asset_type).to_string());
    let safe_filename = filename
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => ch,
        })
        .collect::<String>();
    Ok(std::env::temp_dir()
        .join("CodexHubUpdates")
        .join(safe_filename))
}

fn default_update_filename(asset_type: Option<&str>) -> &'static str {
    match asset_type.unwrap_or_default().to_ascii_lowercase().as_str() {
        "msi" => "CodexHub-update.msi",
        "dmg" => "CodexHub-update.dmg",
        "app-zip" => "CodexHub-update.app.zip",
        "zip" => "CodexHub-update.zip",
        _ => default_platform_update_filename(),
    }
}

#[cfg(target_os = "windows")]
fn default_platform_update_filename() -> &'static str {
    "CodexHub-update.msi"
}

#[cfg(target_os = "macos")]
fn default_platform_update_filename() -> &'static str {
    "CodexHub-update.dmg"
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn default_platform_update_filename() -> &'static str {
    "CodexHub-update"
}

fn launch_downloaded_update(
    text: GuiText,
    update: &DownloadedUpdate,
    _wait_for_current_process_exit: bool,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let current_pid = std::process::id();
        let mut command = if _wait_for_current_process_exit {
            windows_deferred_msi_command(&update.path, current_pid)
        } else {
            windows_msi_command(&update.path)
        };
        hide_command_window(&mut command);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(&update.path);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(&update.path);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|err| text.update_installer_launch_failed(&update.url, &err.to_string()))
}

#[cfg(target_os = "windows")]
fn should_quit_after_update_launch() -> bool {
    true
}

#[cfg(not(target_os = "windows"))]
fn should_quit_after_update_launch() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn windows_msi_command(path: &std::path::Path) -> Command {
    let mut command = Command::new("msiexec.exe");
    command.arg("/i").arg(path);
    command
}

#[cfg(target_os = "windows")]
fn windows_deferred_msi_command(path: &std::path::Path, parent_pid: u32) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-Command")
        .arg(windows_deferred_msi_script(path, parent_pid));
    command
}

#[cfg(target_os = "windows")]
fn windows_deferred_msi_script(path: &std::path::Path, parent_pid: u32) -> String {
    let msi_path = powershell_single_quoted(path.display().to_string().as_str());
    format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $p=Get-Process -Id {parent_pid}; \
         if ($p) {{ $p.WaitForExit(); }}; \
         Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', {msi_path})"
    )
}

#[cfg(target_os = "windows")]
fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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

    #[test]
    fn in_app_update_notes_are_bounded_to_four_short_lines() {
        let notes = "# Developer changelog\n\n- first user change\n- second user change\n- third user change\n- fourth user change\n- internal verification details";

        let compact = compact_update_notes(notes);

        assert_eq!(compact.lines().count(), 4);
        assert!(compact.chars().count() <= 280);
        assert!(compact.contains("fourth user change"));
        assert!(!compact.contains("internal verification"));
    }

    #[test]
    fn release_workflows_embed_separate_user_update_notes() {
        let windows = include_str!("../../.github/workflows/release-windows.yml");
        let macos = include_str!("../../.github/workflows/release-macos.yml");
        let linux = include_str!("../../.github/workflows/release-linux.yml");

        assert!(windows.contains("Get-Content \"UPDATE_NOTES.md\""));
        assert!(windows.contains("notes = $updateNotes"));
        assert!(macos.contains("Path(\"UPDATE_NOTES.md\").read_text"));
        assert!(macos.contains("\"notes\": update_notes"));
        assert!(linux.contains("Path(\"UPDATE_NOTES.md\").read_text"));
        assert!(linux.contains("target/dist/latest-linux.json"));
        assert!(windows.contains("make_latest: false"));
        assert!(linux.contains("make_latest: false"));
        assert!(macos.contains("make_latest: ${{ contains(github.ref_name, '-')"));
        assert!(macos.contains("required=(latest-windows.json latest-linux.json)"));
        assert!(macos.contains("refusing to promote it to Latest"));
    }

    #[test]
    fn macos_update_manifest_url_is_arch_independent() {
        assert!(super::super::MACOS_UPDATE_MANIFEST_URL.ends_with("/latest-macos.json"));
        assert!(!super::super::MACOS_UPDATE_MANIFEST_URL.contains("intel"));
    }

    #[test]
    fn macos_update_policy_downloads_universal_dmg() {
        let mut assets = BTreeMap::new();
        assets.insert(
            "macos-universal".to_string(),
            UpdateAsset {
                url: Some("https://example.test/CodexHub.dmg".to_string()),
                sha256: None,
                asset_type: Some("dmg".to_string()),
            },
        );
        assets.insert(
            "macos-sparkle-universal".to_string(),
            UpdateAsset {
                url: Some("https://example.test/CodexHub.app.zip".to_string()),
                sha256: None,
                asset_type: Some("app-zip".to_string()),
            },
        );
        let manifest = UpdateManifest {
            version: "v9.9.9".to_string(),
            release_url: None,
            notes: None,
            assets,
        };
        assert_eq!(
            platform_download_for_platform(UpdatePlatform::Macos, &manifest)
                .expect("macOS manifest download")
                .url,
            "https://example.test/CodexHub.dmg"
        );
    }

    #[test]
    fn macos_release_workflow_publishes_intel_compat_manifest() {
        let workflow = include_str!("../../.github/workflows/release-macos.yml");

        assert!(workflow.contains("asset_key: macos-universal"));
        assert!(workflow.contains("sparkle_key: macos-sparkle-universal"));
        assert!(workflow.contains("intel_manifest_name: latest-macos-intel.json"));
        assert!(workflow.contains("\"macos-intel\": manifest[\"assets\"][asset_key]"));
        assert!(workflow.contains("target/dist/${{ matrix.intel_manifest_name }}"));
        let cleanup = workflow
            .find("rm -rf \\\n            target/aarch64-apple-darwin")
            .expect("universal build cleanup");
        let create_dmg = workflow.find("hdiutil create").expect("DMG creation");
        assert!(cleanup < create_dmg);
    }

    #[test]
    fn non_macos_update_policy_keeps_platform_downloads() {
        let mut assets = BTreeMap::new();
        assets.insert(
            "windows-x86_64".to_string(),
            UpdateAsset {
                url: Some("https://example.test/CodexHub.msi".to_string()),
                sha256: None,
                asset_type: Some("msi".to_string()),
            },
        );
        assets.insert(
            "linux-x86_64".to_string(),
            UpdateAsset {
                url: Some("https://example.test/CodexHub.tar.gz".to_string()),
                sha256: None,
                asset_type: Some("tar.gz".to_string()),
            },
        );
        let manifest = UpdateManifest {
            version: "v9.9.9".to_string(),
            release_url: None,
            notes: None,
            assets,
        };

        assert_eq!(
            platform_download_for_platform(UpdatePlatform::Windows, &manifest)
                .expect("windows download")
                .url,
            "https://example.test/CodexHub.msi"
        );
        assert_eq!(
            platform_download_for_platform(UpdatePlatform::Linux, &manifest)
                .expect("linux download")
                .url,
            "https://example.test/CodexHub.tar.gz"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_update_launcher_waits_for_current_process_before_msi() {
        let script = windows_deferred_msi_script(
            std::path::Path::new(r"C:\Temp\CodexHub Update's.msi"),
            4242,
        );

        let wait_index = script.find("WaitForExit").expect("wait for parent");
        let start_index = script.find("Start-Process").expect("start installer");

        assert!(wait_index < start_index);
        assert!(script.contains("Get-Process -Id 4242"));
        assert!(script.contains("'C:\\Temp\\CodexHub Update''s.msi'"));
        assert!(script.contains("'msiexec.exe'"));
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
