use std::{
    cell::RefCell,
    env,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use wxdragon::{prelude::*, timer::Timer};

#[cfg(target_os = "windows")]
use super::CREATE_NO_WINDOW;
use super::api::ApiClient;
use super::text::GuiText;
use super::{
    DashboardRefresh, FrameTimerStore, GUI_STARTUP_WATCHDOG_TIMEOUT, GuiTimers, UiHandles,
};
use super::{force_dashboard_refresh, schedule_dashboard_refresh, set_actions_enabled};
use super::{show_dashboard_starting, show_dashboard_startup_error};

pub(super) fn restart_daemon_for_gui(api: &ApiClient, text: GuiText) -> Result<Child, String> {
    stop_existing_daemon(api);
    let mut child = spawn_daemon(text)?;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(250));
        if api.is_online() {
            return Ok(child);
        }
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(text.daemon_exited(&status.to_string()));
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Err(text.daemon_start_timeout().to_string())
}

pub(super) fn stop_existing_daemon(api: &ApiClient) {
    if api.is_online() {
        let _ = api.shutdown();
        wait_for_daemon_offline(api, 5);
    }
    stop_daemon_by_port(api);
    wait_for_daemon_offline(api, 5);
}

pub(super) fn stop_managed_daemon(daemon_child: &Rc<RefCell<Option<Child>>>) {
    if let Some(mut child) = daemon_child.borrow_mut().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

struct StartupResult;

pub(super) fn start_daemon_for_gui_async(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    daemon_child: &Rc<RefCell<Option<Child>>>,
    dashboard_refresh: &DashboardRefresh,
    gui_timers: &GuiTimers,
) {
    if dashboard_refresh
        .daemon_starting
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    dashboard_refresh.generation.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut result) = dashboard_refresh.result.lock() {
        result.take();
    }
    show_dashboard_starting(handles);

    let result: Arc<Mutex<Option<Result<StartupResult, String>>>> = Arc::new(Mutex::new(None));
    {
        let api = api.clone();
        let closing = dashboard_refresh.closing.clone();
        let pending_startup_child = dashboard_refresh.pending_startup_child.clone();
        let result = result.clone();
        let text = handles.text;
        thread::spawn(move || {
            let startup = match restart_daemon_for_gui(&api, text) {
                Ok(mut child) => {
                    let mut pending_child = pending_startup_child.lock().ok();
                    if closing.load(Ordering::SeqCst) {
                        wait_or_kill_child(&mut child, Duration::from_millis(250));
                    } else if let Some(slot) = pending_child.as_mut() {
                        slot.replace(child);
                    } else {
                        wait_or_kill_child(&mut child, Duration::from_millis(250));
                    }
                    Ok(StartupResult)
                }
                Err(err) => Err(err),
            };
            if let Ok(mut slot) = result.lock() {
                slot.replace(startup);
            }
        });
    }

    let startup_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let startup_timer = Timer::new(frame);
    {
        let api = api.clone();
        let handles = handles.clone();
        let daemon_child = daemon_child.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let startup_timer_store = startup_timer_store.clone();
        let startup_started_at = Instant::now();
        let startup_timeout_reported = Rc::new(RefCell::new(false));
        startup_timer.on_tick(move |_| {
            let startup = result.lock().ok().and_then(|mut slot| slot.take());
            let Some(startup) = startup else {
                if !*startup_timeout_reported.borrow()
                    && startup_started_at.elapsed() >= GUI_STARTUP_WATCHDOG_TIMEOUT
                {
                    *startup_timeout_reported.borrow_mut() = true;
                    dashboard_refresh
                        .daemon_starting
                        .store(false, Ordering::SeqCst);
                    show_dashboard_startup_error(&handles, handles.text.daemon_watchdog_timeout());
                    force_dashboard_refresh(&api, &dashboard_refresh);
                }
                return;
            };

            if let Some(timer) = startup_timer_store.borrow().as_ref() {
                timer.stop();
            }

            dashboard_refresh.generation.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut result) = dashboard_refresh.result.lock() {
                result.take();
            }
            dashboard_refresh
                .daemon_starting
                .store(false, Ordering::SeqCst);
            let should_refresh = match startup {
                Ok(_) if dashboard_refresh.closing.load(Ordering::SeqCst) => {
                    stop_pending_startup_daemon(&dashboard_refresh);
                    false
                }
                Ok(_) => {
                    if let Some(child) = take_pending_startup_daemon(&dashboard_refresh) {
                        replace_managed_daemon(&daemon_child, child);
                    }
                    cleanup_codex_app_gui_environment_async(&api, &dashboard_refresh);
                    true
                }
                Err(err) => {
                    show_dashboard_startup_error(&handles, &err);
                    set_actions_enabled(&handles, false);
                    false
                }
            };
            if should_refresh {
                force_dashboard_refresh(&api, &dashboard_refresh);
            }
        });
    }
    startup_timer.start(100, false);
    startup_timer_store.borrow_mut().replace(startup_timer);
    gui_timers.track(&startup_timer_store);
}

pub(super) fn cleanup_codex_app_gui_environment_async(
    api: &ApiClient,
    dashboard_refresh: &DashboardRefresh,
) {
    let api = api.clone();
    let dashboard_refresh = dashboard_refresh.clone();
    thread::spawn(move || {
        let _ = api.repair_codex_app_gui_environment();
        schedule_dashboard_refresh(&api, &dashboard_refresh);
    });
}

pub(super) fn stop_daemon_on_exit(api: &ApiClient, daemon_child: &Rc<RefCell<Option<Child>>>) {
    let child = daemon_child.borrow_mut().take();

    let _ = api.shutdown();
    if let Some(mut child) = child {
        kill_child(&mut child);
    }
}

pub(super) fn stop_pending_startup_daemon(dashboard_refresh: &DashboardRefresh) {
    if let Some(mut child) = take_pending_startup_daemon(dashboard_refresh) {
        wait_or_kill_child(&mut child, Duration::from_millis(250));
    }
}

pub(super) fn take_pending_startup_daemon(dashboard_refresh: &DashboardRefresh) -> Option<Child> {
    dashboard_refresh
        .pending_startup_child
        .lock()
        .ok()
        .and_then(|mut child| child.take())
}

pub(super) fn wait_or_kill_child(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                let _ = child.wait();
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => return,
        }
    }

    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

pub(super) fn kill_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

pub(super) fn wait_for_daemon_offline(api: &ApiClient, attempts: usize) {
    for _ in 0..attempts {
        thread::sleep(Duration::from_millis(100));
        if !api.is_online() {
            break;
        }
    }
}

pub(super) fn replace_managed_daemon(daemon_child: &Rc<RefCell<Option<Child>>>, child: Child) {
    stop_managed_daemon(daemon_child);
    daemon_child.borrow_mut().replace(child);
}

#[cfg(unix)]
pub(super) fn stop_daemon_by_port(api: &ApiClient) {
    let Some(port) = api.local_port() else {
        return;
    };
    let Ok(output) = Command::new("lsof")
        .arg("-nP")
        .arg("-iTCP")
        .arg(format!(":{port}"))
        .arg("-sTCP:LISTEN")
        .arg("-F")
        .arg("pc")
        .output()
    else {
        return;
    };
    let mut pid: Option<String> = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = Some(value.to_string());
        } else if let Some(command) = line.strip_prefix('c')
            && is_codex_remote_gateway_command(command)
        {
            if let Some(pid) = pid.take() {
                let _ = Command::new("kill").arg(pid).status();
            }
        } else if line.starts_with('c') {
            pid = None;
        }
    }
}

#[cfg(windows)]
pub(super) fn stop_daemon_by_port(api: &ApiClient) {
    let Some(port) = api.local_port() else {
        return;
    };
    let mut command = Command::new("netstat");
    command.args(["-ano", "-p", "TCP"]);
    hide_command_window(&mut command);
    let Ok(output) = command.output() else {
        return;
    };

    let current_pid = std::process::id().to_string();
    let mut pids = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 5 {
            continue;
        }
        if !parts[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        if !netstat_addr_has_port(parts[1], port) {
            continue;
        }
        let pid = parts[4].to_string();
        if pid != current_pid && !pids.iter().any(|value| value == &pid) {
            pids.push(pid);
        }
    }

    for pid in pids {
        if windows_pid_is_codex_remote_gateway(&pid) {
            let mut command = Command::new("taskkill");
            command.args(["/PID", &pid, "/F", "/T"]);
            hide_command_window(&mut command);
            let _ = command.status();
        }
    }
}

#[cfg(windows)]
pub(super) fn netstat_addr_has_port(addr: &str, port: u16) -> bool {
    addr.rsplit_once(':')
        .and_then(|(_, value)| value.parse::<u16>().ok())
        == Some(port)
}

#[cfg(windows)]
pub(super) fn windows_pid_is_codex_remote_gateway(pid: &str) -> bool {
    let filter = format!("PID eq {pid}");
    let mut command = Command::new("tasklist");
    command.args(["/FI", &filter, "/FO", "CSV", "/NH"]);
    hide_command_window(&mut command);
    let Ok(output) = command.output() else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .split(',')
        .any(is_codex_remote_gateway_command)
}

fn is_codex_remote_gateway_command(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    command.contains("codex-remote-gateway") || command.contains("codexhub")
}

#[cfg(all(not(unix), not(windows)))]
pub(super) fn stop_daemon_by_port(_api: &ApiClient) {}

pub(super) fn spawn_daemon(text: GuiText) -> Result<Child, String> {
    let mut command = daemon_command(text)?;
    hide_command_window(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| text.daemon_spawn_failed(&err.to_string()))
}

pub(super) fn daemon_command(text: GuiText) -> Result<Command, String> {
    let exe =
        std::env::current_exe().map_err(|err| text.daemon_current_exe_failed(&err.to_string()))?;
    let mut command = Command::new(exe);
    append_daemon_args(&mut command);
    Ok(command)
}

pub(super) fn append_daemon_args(command: &mut Command) {
    if let Some(config_path) = daemon_config_path() {
        command.arg("--config").arg(config_path);
    }
    command.arg("daemon");
}

pub(super) fn daemon_config_path() -> Option<PathBuf> {
    if env::var_os("CODEXHUB_HOME").is_some() {
        return Some(app_support_config_path());
    }
    if let Some(path) = adjacent_config_from_current_exe() {
        return Some(path);
    }
    if env::var_os("CODEXHUB_USE_REPO_CONFIG").is_some() {
        return std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join("config.toml"))
            .filter(|path| path.exists())
            .or_else(|| {
                repo_root_from_target_exe()
                    .map(|repo| repo.join("config.toml"))
                    .filter(|path| path.exists())
            });
    }
    Some(app_support_config_path())
}

pub(super) fn app_support_config_path() -> PathBuf {
    if let Some(base) = env::var_os("CODEXHUB_HOME").map(PathBuf::from) {
        return base.join("config.toml");
    }
    platform_app_support_config_path()
}

#[cfg(target_os = "windows")]
pub(super) fn platform_app_support_config_path() -> PathBuf {
    let legacy = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/Codex Remote Gateway/config.toml"));
    if let Some(path) = legacy.filter(|path| path.exists()) {
        return path;
    }
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let preferred = base.join("Codex Remote Gateway").join("config.toml");
    if preferred.exists() {
        return preferred;
    }
    let legacy = base.join("CodexHub").join("config.toml");
    if legacy.exists() {
        return legacy;
    }
    preferred
}

#[cfg(not(target_os = "windows"))]
pub(super) fn platform_app_support_config_path() -> PathBuf {
    let base = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| {
            let preferred = home.join("Library/Application Support/Codex Remote Gateway");
            if preferred.exists() {
                preferred
            } else {
                let legacy = home.join("Library/Application Support/CodexHub");
                if legacy.exists() { legacy } else { preferred }
            }
        })
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("config.toml")
}

pub(super) fn repo_root_from_target_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile_dir = exe.parent()?;
    let target_dir = profile_dir.parent()?;
    if target_dir.file_name().and_then(|value| value.to_str()) != Some("target") {
        return None;
    }
    let repo_root = target_dir.parent()?.to_path_buf();
    has_manifest(&repo_root).then_some(repo_root)
}

pub(super) fn adjacent_config_from_current_exe() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("config.toml")))
        .filter(|path| path.exists())
        .filter(|path| {
            // Only use the exe-adjacent config when its directory is writable.
            // Installed builds under protected locations such as
            // `C:\\Program Files\\Codex Remote Gateway` ship a default `config.toml` next to
            // the exe, but the directory is read-only for normal-privilege
            // processes, so saving config there fails with HTTP 500. In that
            // case fall through to the per-user app-support path instead.
            path.parent()
                .map(config_directory_is_writable)
                .unwrap_or(false)
        })
}

/// Returns true when a config file can be created/replaced inside `dir`.
fn config_directory_is_writable(dir: &Path) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let probe = dir.join(format!(".codex-remote-gateway-write-probe-{nanos}"));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub(super) fn has_manifest(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
}

#[cfg(target_os = "windows")]
pub(super) fn hide_command_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub(super) fn hide_command_window(_command: &mut Command) {}
