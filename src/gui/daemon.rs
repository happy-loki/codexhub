use std::{
    cell::RefCell,
    collections::HashSet,
    env,
    fs::OpenOptions,
    io::Write,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener},
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
use crate::{
    config::AppConfig,
    daemon_process::{DAEMON_INSTANCE_ENV, read_active_daemon_metadata},
    types::now_ms,
};

const DAEMON_GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_millis(1_500);
const DAEMON_FORCE_STOP_TIMEOUT: Duration = Duration::from_millis(1_500);
const DAEMON_STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct SpawnedDaemon {
    child: Child,
    instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PortOwner {
    pid: u32,
    command: String,
}

pub(super) fn restart_daemon_for_gui(api: &ApiClient, text: GuiText) -> Result<Child, String> {
    stop_existing_daemon(api, text)?;
    let SpawnedDaemon {
        mut child,
        instance_id,
    } = spawn_daemon(text)?;
    let child_pid = child.id();
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(250));
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(text.daemon_exited(&status.to_string()));
        }
        if api
            .daemon_identity()
            .is_ok_and(|identity| identity.pid == child_pid && identity.instance_id == instance_id)
        {
            daemon_manager_log(format!(
                "event=start_ready pid={} instance_id={}",
                child_pid, instance_id
            ));
            return Ok(child);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Err(text.daemon_start_timeout().to_string())
}

pub(super) fn stop_existing_daemon(api: &ApiClient, text: GuiText) -> Result<(), String> {
    let port = api
        .local_port()
        .ok_or_else(|| text.daemon_port_unknown(&api.base_url))?;
    let mut target_pids = HashSet::new();
    if let Ok(identity) = api.daemon_identity() {
        if identity.pid != std::process::id() && process_is_codexhub(identity.pid) {
            target_pids.insert(identity.pid);
        }
        daemon_manager_log(format!(
            "event=stop_request pid={} instance_id={} port={}",
            identity.pid, identity.instance_id, port
        ));
        let _ = api.shutdown();
    }
    if let Some(config_path) = daemon_config_path()
        && let Some(metadata) = read_active_daemon_metadata(&config_path)
        && metadata.identity.pid != std::process::id()
        && process_is_codexhub(metadata.identity.pid)
    {
        target_pids.insert(metadata.identity.pid);
    }

    let initial_target_pids = target_pids.iter().copied().collect::<Vec<_>>();
    if wait_for_daemon_stopped(api, &initial_target_pids, DAEMON_GRACEFUL_STOP_TIMEOUT) {
        return Ok(());
    }

    let owners = port_owners(port);
    if let Some(owners) = owners.as_ref() {
        for owner in owners {
            if owner.pid != std::process::id() && command_is_codexhub(&owner.command) {
                target_pids.insert(owner.pid);
            }
        }
        if !owners.is_empty() && target_pids.is_empty() {
            let owner_summary = owners
                .iter()
                .map(|owner| format!("{}:{}", owner.pid, owner.command))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(text.daemon_port_conflict(port, &owner_summary));
        }
    }

    if target_pids.is_empty() {
        if port_is_available(api) {
            return Ok(());
        }
        return Err(text.daemon_port_conflict(port, "unknown process"));
    }

    let mut target_pids = target_pids.into_iter().collect::<Vec<_>>();
    target_pids.sort_unstable();
    daemon_manager_log(format!(
        "event=stop_force_begin port={} pids={:?}",
        port, target_pids
    ));
    terminate_daemon_processes(&target_pids, false);
    if wait_for_daemon_stopped(api, &target_pids, DAEMON_FORCE_STOP_TIMEOUT) {
        daemon_manager_log(format!(
            "event=stop_ready port={} pids={:?}",
            port, target_pids
        ));
        return Ok(());
    }

    terminate_daemon_processes(&target_pids, true);
    if wait_for_daemon_stopped(api, &target_pids, DAEMON_FORCE_STOP_TIMEOUT) {
        daemon_manager_log(format!(
            "event=stop_killed port={} pids={:?}",
            port, target_pids
        ));
        return Ok(());
    }

    daemon_manager_log(format!(
        "event=stop_failed port={} pids={:?}",
        port, target_pids
    ));
    Err(text.daemon_stop_failed(port, &format!("{target_pids:?}")))
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

    if api.daemon_identity().is_ok() {
        let _ = api.shutdown();
    }
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

pub(super) fn replace_managed_daemon(daemon_child: &Rc<RefCell<Option<Child>>>, child: Child) {
    stop_managed_daemon(daemon_child);
    daemon_child.borrow_mut().replace(child);
}

fn wait_for_daemon_stopped(api: &ApiClient, target_pids: &[u32], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let processes_stopped = target_pids.iter().all(|pid| !process_is_codexhub(*pid));
        if processes_stopped && port_is_available(api) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(DAEMON_STOP_POLL_INTERVAL);
    }
}

fn port_is_available(api: &ApiClient) -> bool {
    let Some(port) = api.local_port() else {
        return false;
    };
    port_owners(port)
        .map(|owners| owners.is_empty())
        .unwrap_or_else(|| fallback_port_is_available(port))
}

fn fallback_port_is_available(port: u16) -> bool {
    let addresses = [
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
        SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
    ];
    let mut checked = false;
    for address in addresses {
        match TcpListener::bind(address) {
            Ok(listener) => {
                checked = true;
                drop(listener);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AddrNotAvailable => {}
            Err(_) => return false,
        }
    }
    checked
}

fn command_is_codexhub(command: &str) -> bool {
    let command = command.trim().replace('\\', "/");
    let executable = command.rsplit('/').next().unwrap_or_default();
    executable.eq_ignore_ascii_case("codexhub") || executable.eq_ignore_ascii_case("codexhub.exe")
}

#[cfg(unix)]
fn port_owners(port: u16) -> Option<Vec<PortOwner>> {
    let executable = if Path::new("/usr/sbin/lsof").exists() {
        "/usr/sbin/lsof"
    } else {
        "lsof"
    };
    let output = Command::new(executable)
        .arg("-nP")
        .arg("-iTCP")
        .arg(format!(":{port}"))
        .arg("-sTCP:LISTEN")
        .arg("-F")
        .arg("pc")
        .output()
        .ok()?;
    Some(parse_lsof_port_owners(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn parse_lsof_port_owners(output: &str) -> Vec<PortOwner> {
    let mut owners = Vec::new();
    let mut pid = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let Some(command) = line.strip_prefix('c')
            && let Some(pid) = pid.take()
        {
            owners.push(PortOwner {
                pid,
                command: command.to_string(),
            });
        }
    }
    owners
}

#[cfg(windows)]
fn port_owners(port: u16) -> Option<Vec<PortOwner>> {
    let mut command = Command::new("netstat");
    command.args(["-ano", "-p", "TCP"]);
    hide_command_window(&mut command);
    let output = command.output().ok()?;
    let pids = parse_netstat_listener_pids(&String::from_utf8_lossy(&output.stdout), port);
    Some(
        pids.into_iter()
            .map(|pid| PortOwner {
                pid,
                command: windows_process_name(pid).unwrap_or_default(),
            })
            .collect(),
    )
}

fn parse_netstat_listener_pids(output: &str, port: u16) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in output.lines() {
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
        let Ok(pid) = parts[4].parse::<u32>() else {
            continue;
        };
        if pid != std::process::id() && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(windows)]
pub(super) fn netstat_addr_has_port(addr: &str, port: u16) -> bool {
    addr.rsplit_once(':')
        .and_then(|(_, value)| value.parse::<u16>().ok())
        == Some(port)
}

#[cfg(windows)]
fn windows_process_name(pid: u32) -> Option<String> {
    let filter = format!("PID eq {pid}");
    let mut command = Command::new("tasklist");
    command.args(["/FI", &filter, "/FO", "CSV", "/NH"]);
    hide_command_window(&mut command);
    let output = command.output().ok()?;
    let output_text = String::from_utf8_lossy(&output.stdout);
    let line = output_text.lines().next()?.trim();
    let name = line
        .strip_prefix('"')?
        .split("\",\"")
        .next()?
        .trim()
        .to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(unix)]
fn process_is_codexhub(pid: u32) -> bool {
    let pid = pid.to_string();
    let output = Command::new("/bin/ps")
        .args(["-p", pid.as_str(), "-o", "comm="])
        .output();
    output
        .ok()
        .is_some_and(|output| command_is_codexhub(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(windows)]
fn process_is_codexhub(pid: u32) -> bool {
    windows_process_name(pid).is_some_and(|name| command_is_codexhub(&name))
}

#[cfg(all(not(unix), not(windows)))]
fn process_is_codexhub(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn terminate_daemon_processes(pids: &[u32], force: bool) {
    let signal = if force { "-KILL" } else { "-TERM" };
    for pid in pids {
        let pid = pid.to_string();
        let _ = Command::new("/bin/kill")
            .args([signal, pid.as_str()])
            .status();
    }
}

#[cfg(windows)]
fn terminate_daemon_processes(pids: &[u32], _force: bool) {
    for pid in pids {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/F", "/T"]);
        hide_command_window(&mut command);
        let _ = command.status();
    }
}

#[cfg(all(not(unix), not(windows)))]
fn terminate_daemon_processes(_pids: &[u32], _force: bool) {}

fn spawn_daemon(text: GuiText) -> Result<SpawnedDaemon, String> {
    let mut command = daemon_command(text)?;
    let instance_id = uuid::Uuid::new_v4().to_string();
    command.env(DAEMON_INSTANCE_ENV, &instance_id);
    hide_command_window(&mut command);
    command.stdin(Stdio::null());
    if let Some((stdout, stderr)) = daemon_output_stdio() {
        command.stdout(stdout).stderr(stderr);
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let child = command
        .spawn()
        .map_err(|err| text.daemon_spawn_failed(&err.to_string()))?;
    daemon_manager_log(format!(
        "event=spawn_child pid={} instance_id={}",
        child.id(),
        instance_id
    ));
    Ok(SpawnedDaemon { child, instance_id })
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

fn daemon_output_stdio() -> Option<(Stdio, Stdio)> {
    let path = daemon_startup_log_path();
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).ok()?;
    }
    let mut stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let _ = writeln!(
        stdout,
        "\n[ts_ms={}] [daemon] event=spawn_start exe={}",
        now_ms(),
        exe
    );
    Some((Stdio::from(stdout), Stdio::from(stderr)))
}

fn daemon_manager_log(message: impl AsRef<str>) {
    let path = daemon_startup_log_path();
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            file,
            "[ts_ms={}] [daemon_manager] {}",
            now_ms(),
            message.as_ref()
        );
    }
}

fn daemon_startup_log_path() -> PathBuf {
    let config_path = daemon_config_path().unwrap_or_else(app_support_config_path);
    daemon_startup_log_path_for_config_path(&config_path)
}

fn daemon_startup_log_path_for_config_path(config_path: &Path) -> PathBuf {
    let base = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let state_path = AppConfig::load_or_default(&config_path.to_path_buf())
        .ok()
        .map(|config| {
            if config.state_path.is_relative() {
                base.join(config.state_path)
            } else {
                config.state_path
            }
        })
        .unwrap_or_else(|| base.join("codexhub-state.json"));
    state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(base)
        .join("logs")
        .join("codexhub-daemon-startup.log")
}

#[cfg(target_os = "windows")]
pub(super) fn platform_app_support_config_path() -> PathBuf {
    let legacy = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/CodexHub/config.toml"));
    if let Some(path) = legacy.filter(|path| path.exists()) {
        return path;
    }
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("CodexHub").join("config.toml")
}

#[cfg(not(target_os = "windows"))]
pub(super) fn platform_app_support_config_path() -> PathBuf {
    let base = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/CodexHub"))
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
            // `C:\\Program Files\\CodexHub` ship a default `config.toml` next to
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
    let probe = dir.join(format!(".codexhub-write-probe-{nanos}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_startup_log_follows_relative_state_path() {
        let root = std::env::temp_dir().join(format!("codexhub-daemon-log-test-{}", now_ms()));
        let config_path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("create temp dir");
        std::fs::write(&config_path, "statePath = 'state/codexhub-state.json'\n")
            .expect("write config");

        assert_eq!(
            daemon_startup_log_path_for_config_path(&config_path),
            root.join("state")
                .join("logs")
                .join("codexhub-daemon-startup.log")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lsof_parser_keeps_pid_and_command_pairs() {
        assert_eq!(
            parse_lsof_port_owners("p49094\ncCodexHub\np57488\ncother\n"),
            vec![
                PortOwner {
                    pid: 49094,
                    command: "CodexHub".to_string(),
                },
                PortOwner {
                    pid: 57488,
                    command: "other".to_string(),
                },
            ]
        );
        assert!(command_is_codexhub("CodexHub"));
        assert!(command_is_codexhub("codexhub"));
        assert!(command_is_codexhub(
            "C:\\Program Files\\CodexHub\\codexhub.exe"
        ));
        assert!(!command_is_codexhub("codexhub-helper"));
        assert!(!command_is_codexhub("not-codexhub.exe"));
        assert!(!command_is_codexhub("other"));
    }

    #[test]
    fn netstat_parser_returns_unique_listening_pids_for_port() {
        let output = r#"
  TCP    127.0.0.1:3847       0.0.0.0:0       LISTENING       49094
  TCP    [::1]:3847           [::]:0          LISTENING       49094
  TCP    127.0.0.1:9999       0.0.0.0:0       LISTENING       57488
"#;
        assert_eq!(parse_netstat_listener_pids(output, 3847), vec![49094]);
    }
}
