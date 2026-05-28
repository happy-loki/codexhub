use std::{
    cell::RefCell,
    env,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    rc::Rc,
    thread,
    time::Duration,
};

use qrcode::{Color, QrCode};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wxdragon::{prelude::*, timer::Timer};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";

fn main() {
    if let Err(err) = wxdragon::main(|_| build_ui()) {
        eprintln!("failed to start Codex Remote GUI: {err:?}");
    }
}

fn build_ui() {
    let api = ApiClient::new(default_base_url());

    let frame = Frame::builder()
        .with_title("Codex Remote")
        .with_size(Size::new(980, 700))
        .build();
    frame.set_background_color(Colour::rgb(246, 247, 250));
    let status_bar = StatusBar::builder(&frame)
        .with_fields_count(3)
        .with_status_widths(vec![-2, -1, -1])
        .add_initial_text(0, "本地服务启动中")
        .add_initial_text(1, "日志写入本地文件")
        .add_initial_text(2, "自动刷新 2.5s")
        .build();

    let root = Panel::builder(&frame).build();
    root.set_background_color(Colour::rgb(246, 247, 250));

    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let header_panel = Panel::builder(&root)
        .with_style(PanelStyle::BorderNone)
        .build();
    header_panel.set_background_color(Colour::rgb(246, 247, 250));
    let header = BoxSizer::builder(Orientation::Horizontal).build();
    let header_copy = BoxSizer::builder(Orientation::Vertical).build();
    let title = StaticText::builder(&header_panel)
        .with_label("Codex Remote")
        .build();
    title.set_foreground_color(Colour::rgb(24, 28, 35));
    header_copy.add(&title, 0, SizerFlag::Bottom, 4);
    let subtitle = StaticText::builder(&header_panel)
        .with_label("本地 remote-control backend + 飞书桥接")
        .build();
    subtitle.set_foreground_color(Colour::rgb(91, 100, 114));
    header_copy.add(&subtitle, 0, SizerFlag::Expand, 0);
    header.add_sizer(&header_copy, 1, SizerFlag::Expand, 0);

    let endpoint = StaticText::builder(&header_panel)
        .with_label(&format!("服务地址 {}", api.base_url))
        .build();
    endpoint.set_foreground_color(Colour::rgb(103, 111, 124));
    header.add(&endpoint, 0, SizerFlag::AlignCenterVertical, 0);
    header_panel.set_sizer(header, true);
    root_sizer.add(
        &header_panel,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let status_section =
        StaticBoxSizerBuilder::new_with_label(Orientation::Vertical, &root, "状态概览").build();
    let status_row = BoxSizer::builder(Orientation::Horizontal).build();
    let service_status = status_panel(&root, "本地服务", StatusIconKind::Service);
    let feishu_status = status_panel(&root, "飞书", StatusIconKind::Feishu);
    let codex_status = status_panel(&root, "Codex App", StatusIconKind::Codex);
    status_row.add(
        &service_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_row.add(
        &feishu_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_row.add(
        &codex_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_section.add_sizer(
        &status_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        8,
    );
    root_sizer.add_sizer(
        &status_section,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );

    let notebook = Notebook::builder(&root).build();

    let codex_page = Panel::builder(&notebook)
        .with_style(PanelStyle::TabTraversal)
        .build();
    codex_page.set_background_color(Colour::rgb(250, 251, 253));
    let codex_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let codex_status_box =
        StaticBoxSizerBuilder::new_with_label(Orientation::Vertical, &codex_page, "Codex App 状态")
            .build();
    let codex_config_state = StaticText::builder(&codex_page)
        .with_label("正在读取 ~/.codex 配置状态")
        .build();
    codex_config_state.set_foreground_color(Colour::rgb(75, 84, 98));
    codex_config_state.wrap(760);
    codex_status_box.add(
        &codex_config_state,
        0,
        SizerFlag::Expand | SizerFlag::All,
        12,
    );
    codex_sizer.add_sizer(
        &codex_status_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );

    let config_box = StaticBoxSizerBuilder::new_with_label(
        Orientation::Vertical,
        &codex_page,
        "写入 Codex App 配置",
    )
    .build();
    let config_header = BoxSizer::builder(Orientation::Horizontal).build();
    let config_hint = StaticText::builder(&codex_page)
        .with_label("填写第三方模型服务的 Base URL 和 API Key，然后写入 Codex App 配置。")
        .build();
    config_hint.set_foreground_color(Colour::rgb(103, 111, 124));
    config_header.add(&config_hint, 1, SizerFlag::AlignCenterVertical, 0);
    let uninstall_button = Button::builder(&codex_page).with_label("卸载注入").build();
    uninstall_button.set_tooltip("移除 Codex App 中由 Codex Remote 写入的连接配置");
    let configure_button = Button::builder(&codex_page).with_label("写入配置").build();
    configure_button.set_tooltip("写入 Codex App 使用 Codex Remote 所需的本地配置");
    config_header.add(&uninstall_button, 0, SizerFlag::Right, 8);
    config_header.add(&configure_button, 0, SizerFlag::Right, 0);
    config_box.add_sizer(&config_header, 0, SizerFlag::Expand | SizerFlag::All, 12);

    let form = FlexGridSizer::builder(0, 2)
        .with_gap(Size::new(10, 12))
        .build();
    form.add_growable_col(1, 1);
    let provider_name = text_field_row(&codex_page, &form, "Provider 名称", "codex");
    let provider_base_url = text_field_row(&codex_page, &form, "Base URL", "");
    let provider_key = text_field_row(&codex_page, &form, "API Key", "");
    config_box.add_sizer(
        &form,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        12,
    );
    codex_sizer.add_sizer(
        &config_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );
    codex_sizer.add_stretch_spacer(1);
    codex_page.set_sizer(codex_sizer, true);

    let feishu_page = Panel::builder(&notebook)
        .with_style(PanelStyle::TabTraversal)
        .build();
    feishu_page.set_background_color(Colour::rgb(250, 251, 253));
    let feishu_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let feishu_box =
        StaticBoxSizerBuilder::new_with_label(Orientation::Vertical, &feishu_page, "飞书机器人")
            .build();
    let feishu_state = StaticText::builder(&feishu_page)
        .with_label("检测中")
        .build();
    feishu_state.set_foreground_color(Colour::rgb(73, 83, 96));
    feishu_box.add(
        &feishu_state,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let feishu_detail = StaticText::builder(&feishu_page)
        .with_label("正在读取飞书接入状态")
        .build();
    feishu_detail.set_foreground_color(Colour::rgb(82, 91, 105));
    feishu_detail.wrap(760);
    feishu_box.add(
        &feishu_detail,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let divider = StaticLine::builder(&feishu_page).build();
    feishu_box.add(
        &divider,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );

    let feishu_meta = StaticText::builder(&feishu_page).with_label("").build();
    feishu_meta.set_foreground_color(Colour::rgb(103, 111, 124));
    feishu_meta.wrap(760);
    feishu_box.add(
        &feishu_meta,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let feishu_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    feishu_buttons.add_stretch_spacer(1);
    let stop_bridge_button = Button::builder(&feishu_page).with_label("断开接入").build();
    stop_bridge_button.set_tooltip("停止飞书桥接，不删除已保存的机器人配置");
    let change_bot_button = Button::builder(&feishu_page)
        .with_label("更换机器人")
        .build();
    change_bot_button.set_tooltip("重新进入飞书扫码接入流程");
    feishu_buttons.add(&stop_bridge_button, 0, SizerFlag::Right, 8);
    feishu_buttons.add(&change_bot_button, 0, SizerFlag::Right, 0);
    feishu_box.add_sizer(&feishu_buttons, 0, SizerFlag::Expand | SizerFlag::All, 12);
    feishu_sizer.add_sizer(
        &feishu_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );
    feishu_sizer.add_stretch_spacer(1);
    feishu_page.set_sizer(feishu_sizer, true);

    let system_page = Panel::builder(&notebook)
        .with_style(PanelStyle::TabTraversal)
        .build();
    system_page.set_background_color(Colour::rgb(250, 251, 253));
    let system_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let system_box =
        StaticBoxSizerBuilder::new_with_label(Orientation::Vertical, &system_page, "本地服务")
            .build();
    let service_text = StaticText::builder(&system_page)
        .with_label("Codex Remote 会在 GUI 打开时接管并重启本地 backend，避免旧版本服务残留；GUI 退出时会关闭本地 backend，并清除本次写入的 Codex App 环境变量。不会安装开机启动项，也不会修改系统常驻服务。")
        .build();
    service_text.set_foreground_color(Colour::rgb(82, 91, 105));
    service_text.wrap(760);
    system_box.add(&service_text, 0, SizerFlag::Expand | SizerFlag::All, 12);
    let refresh_button = Button::builder(&system_page).with_label("检测状态").build();
    refresh_button.set_tooltip("立即刷新本地服务、飞书和 Codex App 连接状态");
    let start_daemon_button = Button::builder(&system_page)
        .with_label("启动本地服务")
        .build();
    start_daemon_button.set_tooltip("本次会话启动 codex-remote daemon，不安装开机启动项");
    let system_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    system_buttons.add_stretch_spacer(1);
    system_buttons.add(&start_daemon_button, 0, SizerFlag::Right, 8);
    system_buttons.add(&refresh_button, 0, SizerFlag::Right, 0);
    system_box.add_sizer(&system_buttons, 0, SizerFlag::Expand | SizerFlag::All, 12);
    system_sizer.add_sizer(
        &system_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );
    system_sizer.add_stretch_spacer(1);
    system_page.set_sizer(system_sizer, true);

    notebook.add_page(&codex_page, "Codex App", true, None);
    notebook.add_page(&feishu_page, "飞书", false, None);
    notebook.add_page(&system_page, "本地服务", false, None);

    root_sizer.add(
        &notebook,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        8,
    );

    root.set_sizer(root_sizer, true);
    let frame_sizer = BoxSizer::builder(Orientation::Vertical).build();
    frame_sizer.add(&root, 1, SizerFlag::Expand, 0);
    frame.set_sizer(frame_sizer, true);

    let handles = UiHandles {
        status_bar,
        service_status,
        feishu_status,
        codex_status,
        feishu_state,
        feishu_detail,
        feishu_meta,
        codex_config_state,
        change_bot_button,
        stop_bridge_button,
        configure_button,
        refresh_button,
        start_daemon_button,
        uninstall_button,
        provider_name,
        provider_base_url,
        provider_key,
    };

    let daemon_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    match restart_daemon_for_gui(&api) {
        Ok(child) => {
            replace_managed_daemon(&daemon_child, child);
        }
        Err(err) => {
            status_bar.set_status_text(&format!("本地服务启动失败：{err}"), 0);
        }
    }
    refresh_dashboard(&api, &handles);
    repair_gui_environment_if_needed(&api, &handles);
    refresh_dashboard(&api, &handles);

    {
        let api = api.clone();
        let handles = handles;
        refresh_button.on_click(move |_| refresh_dashboard(&api, &handles));
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        let daemon_child = daemon_child.clone();
        start_daemon_button.on_click(move |_| match restart_daemon_for_gui(&api) {
            Ok(child) => {
                replace_managed_daemon(&daemon_child, child);
                repair_gui_environment_if_needed(&api, &handles);
                show_info(&frame, "本地服务已运行。");
                refresh_dashboard(&api, &handles);
            }
            Err(err) => show_error(&frame, &err),
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        configure_button.on_click(move |_| {
            let request = ConfigureRequest {
                provider_name: Some(provider_name.get_value()),
                provider_base_url: Some(provider_base_url.get_value()),
                provider_key: Some(provider_key.get_value()),
                model: None,
            };
            match api.configure_codex_app(&request) {
                Ok(_) => {
                    show_info(
                        &frame,
                        "配置已写入。请重新打开 Codex App，或在 Codex App 中重新进入远程控制。",
                    );
                    refresh_dashboard(&api, &handles);
                }
                Err(err) => show_error(&frame, &err),
            }
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        uninstall_button.on_click(move |_| match api.uninstall_codex_app() {
            Ok(_) => {
                show_info(&frame, "Codex App 注入配置已卸载。");
                refresh_dashboard(&api, &handles);
            }
            Err(err) => show_error(&frame, &err),
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        stop_bridge_button.on_click(move |_| match api.stop_bridge() {
            Ok(_) => {
                show_info(&frame, "飞书接入已断开。");
                refresh_dashboard(&api, &handles);
            }
            Err(err) => show_error(&frame, &err),
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        change_bot_button.on_click(move |_| {
            show_onboard_dialog(&frame, api.clone());
            refresh_dashboard(&api, &handles);
        });
    }

    let timer_store: Rc<RefCell<Option<Timer<Frame>>>> = Rc::new(RefCell::new(None));
    let timer = Timer::new(&frame);
    {
        let api = api.clone();
        let handles = handles;
        timer.on_tick(move |_| refresh_dashboard(&api, &handles));
    }
    timer.start(2500, false);
    timer_store.borrow_mut().replace(timer);
    std::mem::forget(timer_store);

    {
        let api = api.clone();
        let daemon_child = daemon_child.clone();
        frame.on_close(move |_| stop_daemon_on_exit(&api, &daemon_child));
    }

    frame.centre();
    frame.show(true);
}

fn default_base_url() -> String {
    std::env::var("CODEX_REMOTE_GUI_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn restart_daemon_for_gui(api: &ApiClient) -> Result<Child, String> {
    stop_existing_daemon(api);
    let mut child = spawn_daemon()?;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(250));
        if api.is_online() {
            return Ok(child);
        }
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(format!("本地服务启动后退出：{status}"));
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Err("本地服务已启动，但 10 秒内没有响应。请检查 logs/codex-remote-chain.log。".to_string())
}

fn stop_existing_daemon(api: &ApiClient) {
    if api.is_online() {
        let _ = api.shutdown();
        wait_for_daemon_offline(api, 15);
    }
    stop_daemon_by_port(api);
    wait_for_daemon_offline(api, 15);
}

fn stop_managed_daemon(daemon_child: &Rc<RefCell<Option<Child>>>) {
    if let Some(mut child) = daemon_child.borrow_mut().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn stop_daemon_on_exit(api: &ApiClient, daemon_child: &Rc<RefCell<Option<Child>>>) {
    clear_codex_app_gui_environment(&api.url("/backend-api"));
    let _ = api.shutdown();
    wait_for_daemon_offline(api, 10);
    stop_managed_daemon(daemon_child);
    if api.is_online() {
        stop_daemon_by_port(api);
        wait_for_daemon_offline(api, 10);
    }
}

fn clear_codex_app_gui_environment(backend_url: &str) {
    #[cfg(target_os = "macos")]
    {
        let login_issuer = oauth_issuer_url(backend_url);
        unset_launchctl_env_if_matches("CODEX_API_BASE_URL", backend_url);
        unset_launchctl_env_if_matches("CODEX_APP_SERVER_LOGIN_ISSUER", &login_issuer);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = backend_url;
    }
}

fn oauth_issuer_url(backend_url: &str) -> String {
    backend_url
        .trim_end_matches('/')
        .strip_suffix("/backend-api")
        .unwrap_or_else(|| backend_url.trim_end_matches('/'))
        .to_string()
}

#[cfg(target_os = "macos")]
fn unset_launchctl_env_if_matches(name: &str, expected: &str) {
    let Ok(output) = Command::new("launchctl").arg("getenv").arg(name).output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value == expected {
        let _ = Command::new("launchctl").arg("unsetenv").arg(name).output();
    }
}

fn wait_for_daemon_offline(api: &ApiClient, attempts: usize) {
    for _ in 0..attempts {
        thread::sleep(Duration::from_millis(100));
        if !api.is_online() {
            break;
        }
    }
}

fn replace_managed_daemon(daemon_child: &Rc<RefCell<Option<Child>>>, child: Child) {
    stop_managed_daemon(daemon_child);
    daemon_child.borrow_mut().replace(child);
}

#[cfg(unix)]
fn stop_daemon_by_port(api: &ApiClient) {
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
            && command.contains("codex-remote")
        {
            if let Some(pid) = pid.take() {
                let _ = Command::new("kill").arg(pid).status();
            }
        } else if line.starts_with('c') {
            pid = None;
        }
    }
}

#[cfg(not(unix))]
fn stop_daemon_by_port(_api: &ApiClient) {}

fn spawn_daemon() -> Result<Child, String> {
    let mut command = daemon_command()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("无法启动本地服务：{err}"))
}

fn daemon_command() -> Result<Command, String> {
    if let Some(exe) = sibling_daemon_exe() {
        let mut command = Command::new(exe);
        append_daemon_args(&mut command);
        return Ok(command);
    }

    if let Some(exe) = bundled_daemon_exe() {
        let mut command = Command::new(exe);
        append_daemon_args(&mut command);
        return Ok(command);
    }

    if env::var_os("CODEX_REMOTE_USE_REPO_CONFIG").is_some()
        && let Some(repo_root) = repo_root_from_target_exe().or_else(|| repo_root_from_cwd())
    {
        let mut command = Command::new("cargo");
        command.current_dir(repo_root).arg("run").arg("--");
        append_daemon_args(&mut command);
        return Ok(command);
    }

    let mut command = Command::new("codex-remote");
    append_daemon_args(&mut command);
    Ok(command)
}

fn append_daemon_args(command: &mut Command) {
    if let Some(config_path) = daemon_config_path() {
        command.arg("--config").arg(config_path);
    }
    command.arg("daemon");
}

fn sibling_daemon_exe() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "codex-remote.exe"
    } else {
        "codex-remote"
    };
    let path = std::env::current_exe().ok()?.with_file_name(exe_name);
    path.exists().then_some(path)
}

fn bundled_daemon_exe() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "codex-remote.exe"
    } else {
        "codex-remote"
    };
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    let contents_dir = macos_dir.parent()?;
    if macos_dir.file_name().and_then(|value| value.to_str()) != Some("MacOS") {
        return None;
    }
    let path = contents_dir.join("Resources").join(exe_name);
    path.exists().then_some(path)
}

fn daemon_config_path() -> Option<PathBuf> {
    if env::var_os("CODEX_REMOTE_USE_REPO_CONFIG").is_some() {
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

fn app_support_config_path() -> PathBuf {
    let base = env::var_os("CODEX_REMOTE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("Library/Application Support/Codex Remote"))
        })
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("config.toml")
}

fn repo_root_from_cwd() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    has_manifest(&cwd).then_some(cwd)
}

fn repo_root_from_target_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile_dir = exe.parent()?;
    let target_dir = profile_dir.parent()?;
    if target_dir.file_name().and_then(|value| value.to_str()) != Some("target") {
        return None;
    }
    let repo_root = target_dir.parent()?.to_path_buf();
    has_manifest(&repo_root).then_some(repo_root)
}

fn has_manifest(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
}

#[derive(Clone)]
struct ApiClient {
    base_url: String,
    http: Client,
}

impl ApiClient {
    fn new(base_url: String) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("build HTTP client");
        Self { base_url, http }
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let text = self.request_text(self.http.get(self.url(path)))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn is_online(&self) -> bool {
        self.get::<serde_json::Value>("/api/status").is_ok()
    }

    fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let text = self.request_text(self.http.post(self.url(path)))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let text = self.request_text(self.http.post(self.url(path)).json(body))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn request_text(&self, request: reqwest::blocking::RequestBuilder) -> Result<String, String> {
        let response = request
            .send()
            .map_err(|err| format!("无法连接本地服务 {}：{err}", self.base_url))?;
        let status = response.status();
        let text = response.text().map_err(|err| err.to_string())?;
        if status.is_success() {
            Ok(text)
        } else {
            Err(format!("HTTP {status}: {text}"))
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn local_port(&self) -> Option<u16> {
        let url = reqwest::Url::parse(&self.base_url).ok()?;
        let host = url.host_str()?;
        matches!(host, "127.0.0.1" | "localhost" | "::1").then_some(url.port_or_known_default()?)
    }

    fn dashboard(&self) -> DashboardSnapshot {
        let status = match self.get::<ServerStatus>("/api/status") {
            Ok(status) => status,
            Err(err) => {
                return DashboardSnapshot {
                    service_online: false,
                    error: Some(err),
                    ..DashboardSnapshot::default()
                };
            }
        };

        DashboardSnapshot {
            service_online: true,
            error: None,
            config: self.get::<AppConfig>("/api/config").ok(),
            backend: self
                .get::<RemoteControlBackendStatus>("/api/remote-control/backend-status")
                .ok(),
            remote: self
                .get::<RemoteControlStatus>("/api/remote-control/status")
                .ok(),
            codex_app: self.get::<CodexAppStatus>("/api/codex-app/status").ok(),
            status: Some(status),
        }
    }

    fn configure_codex_app(&self, request: &ConfigureRequest) -> Result<serde_json::Value, String> {
        self.post_json("/api/codex-app/configure", request)
    }

    fn uninstall_codex_app(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/codex-app/uninstall")
    }

    fn repair_codex_app_gui_environment(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/codex-app/repair-gui-environment")
    }

    fn stop_bridge(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/bridge/stop")
    }

    fn shutdown(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/shutdown")
    }

    fn start_feishu_onboard(&self) -> Result<FeishuOnboardStart, String> {
        self.post_empty("/api/feishu/onboard/start")
    }

    fn poll_feishu_onboard(&self, device_code: &str) -> Result<FeishuOnboardPoll, String> {
        self.post_json(
            "/api/feishu/onboard/poll",
            &serde_json::json!({ "deviceCode": device_code }),
        )
    }
}

#[derive(Clone, Copy)]
struct StatusPanel {
    panel: Panel,
    marker: StaticText,
    state: StaticText,
    detail: StaticText,
}

#[derive(Clone, Copy)]
enum StatusIconKind {
    Service,
    Feishu,
    Codex,
}

#[derive(Clone, Copy)]
struct UiHandles {
    status_bar: StatusBar,
    service_status: StatusPanel,
    feishu_status: StatusPanel,
    codex_status: StatusPanel,
    feishu_state: StaticText,
    feishu_detail: StaticText,
    feishu_meta: StaticText,
    codex_config_state: StaticText,
    change_bot_button: Button,
    stop_bridge_button: Button,
    configure_button: Button,
    refresh_button: Button,
    start_daemon_button: Button,
    uninstall_button: Button,
    provider_name: TextCtrl,
    provider_base_url: TextCtrl,
    provider_key: TextCtrl,
}

#[derive(Default)]
struct DashboardSnapshot {
    service_online: bool,
    status: Option<ServerStatus>,
    config: Option<AppConfig>,
    backend: Option<RemoteControlBackendStatus>,
    remote: Option<RemoteControlStatus>,
    codex_app: Option<CodexAppStatus>,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerStatus {
    bind: String,
    feishu_ws: FeishuWsState,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuWsState {
    connecting: bool,
    connected: bool,
    last_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppConfig {
    feishu: FeishuConfig,
    bridge: BridgeConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuConfig {
    app_id: String,
    allowed_open_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeConfig {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlBackendStatus {
    enabled: bool,
    feishu_configured: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlStatus {
    connected: bool,
    initialized: bool,
    server_name: Option<String>,
    current_thread_id: Option<String>,
    last_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppStatus {
    codex_home: String,
    configured: bool,
    config_ok: bool,
    auth_ok: bool,
    gui_api_base: GuiApiBaseStatus,
    provider: Option<CodexAppProviderStatus>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppProviderStatus {
    name: String,
    base_url: Option<String>,
    key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuiApiBaseStatus {
    #[serde(default)]
    configured: bool,
    #[serde(default)]
    login_issuer_configured: bool,
    value: Option<String>,
    #[serde(default)]
    login_issuer_value: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureRequest {
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardStart {
    verification_uri_complete: String,
    device_code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardPoll {
    done: bool,
    error: Option<serde_json::Value>,
}

fn status_panel(parent: &Panel, title: &str, icon_kind: StatusIconKind) -> StatusPanel {
    let panel = Panel::builder(parent)
        .with_style(PanelStyle::BorderStatic)
        .build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    panel.set_min_size(Size::new(280, 96));

    let row = BoxSizer::builder(Orientation::Horizontal).build();
    let icon = StaticBitmap::builder(&panel)
        .with_bitmap(Some(status_icon_bitmap(icon_kind, 34)))
        .with_scale_mode(Some(ScaleMode::AspectFit))
        .with_size(Size::new(34, 34))
        .build();
    icon.set_min_size(Size::new(34, 34));
    row.add_spacer(18);
    row.add(
        &icon,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        16,
    );

    let text_col = BoxSizer::builder(Orientation::Vertical).build();
    text_col.add_stretch_spacer(1);
    let title_row = BoxSizer::builder(Orientation::Horizontal).build();
    let marker = StaticText::builder(&panel).with_label("●").build();
    marker.set_foreground_color(Colour::rgb(116, 124, 136));
    title_row.add(&marker, 0, SizerFlag::Right, 5);
    let title_label = StaticText::builder(&panel).with_label(title).build();
    title_label.set_foreground_color(Colour::rgb(91, 100, 114));
    title_row.add(&title_label, 0, SizerFlag::Bottom, 0);
    text_col.add_sizer(&title_row, 0, SizerFlag::Bottom, 4);

    let state = StaticText::builder(&panel).with_label("检测中").build();
    state.set_foreground_color(Colour::rgb(34, 39, 47));
    text_col.add(&state, 0, SizerFlag::Bottom, 4);

    let detail = StaticText::builder(&panel).with_label("").build();
    detail.set_foreground_color(Colour::rgb(103, 111, 124));
    detail.wrap(250);
    text_col.add(&detail, 0, SizerFlag::Expand, 0);
    text_col.add_stretch_spacer(1);

    row.add_sizer(&text_col, 1, SizerFlag::Expand, 0);
    row.add_spacer(18);
    panel.set_sizer(row, true);
    StatusPanel {
        panel,
        marker,
        state,
        detail,
    }
}

fn status_icon_bitmap(kind: StatusIconKind, size: usize) -> Bitmap {
    match kind {
        StatusIconKind::Feishu => return brand_bitmap("feishu-logo.png"),
        StatusIconKind::Codex => return brand_bitmap("codex-app-logo.png"),
        StatusIconKind::Service => {}
    }

    let mut canvas = IconCanvas::new(size, [0, 0, 0, 0]);
    draw_service_icon(&mut canvas);
    Bitmap::from_rgba(&canvas.rgba, size as u32, size as u32).expect("status icon bitmap")
}

fn brand_bitmap(file_name: &str) -> Bitmap {
    let path = brand_asset_path(file_name);
    let image = image::open(&path)
        .unwrap_or_else(|err| panic!("failed to load brand image {}: {err}", path.display()))
        .into_rgba8();
    let (width, height) = image.dimensions();
    Bitmap::from_rgba(image.as_raw(), width, height)
        .unwrap_or_else(|| panic!("failed to create bitmap from {}", path.display()))
}

fn brand_asset_path(file_name: &str) -> PathBuf {
    if let Some(path) = bundled_brand_asset_path(file_name) {
        return path;
    }

    repo_root_from_target_exe()
        .or_else(repo_root_from_cwd)
        .map(|repo| repo.join("packaging/brand").join(file_name))
        .filter(|path| path.exists())
        .unwrap_or_else(|| panic!("brand asset not found: {file_name}"))
}

fn bundled_brand_asset_path(file_name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    let contents_dir = macos_dir.parent()?;
    if macos_dir.file_name().and_then(|value| value.to_str()) != Some("MacOS") {
        return None;
    }
    let path = contents_dir.join("Resources/brand").join(file_name);
    path.exists().then_some(path)
}

struct IconCanvas {
    size: usize,
    rgba: Vec<u8>,
}

impl IconCanvas {
    fn new(size: usize, background: [u8; 4]) -> Self {
        let mut rgba = vec![0; size * size * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&background);
        }
        Self { size, rgba }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
        let min_x = (cx - radius).floor().max(0.0) as usize;
        let max_x = (cx + radius).ceil().min((self.size - 1) as f32) as usize;
        let min_y = (cy - radius).floor().max(0.0) as usize;
        let max_y = (cy + radius).ceil().min((self.size - 1) as f32) as usize;
        let radius_sq = radius * radius;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                if dx * dx + dy * dy <= radius_sq {
                    self.set_pixel(x, y, color);
                }
            }
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
        for yy in y..(y + height).min(self.size) {
            for xx in x..(x + width).min(self.size) {
                self.set_pixel(xx, yy, color);
            }
        }
    }

    fn fill_round_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        radius: usize,
        color: [u8; 4],
    ) {
        let x2 = x + width - 1;
        let y2 = y + height - 1;
        let radius = radius as f32;
        for yy in y..=y2.min(self.size - 1) {
            for xx in x..=x2.min(self.size - 1) {
                let cx = if xx < x + radius as usize {
                    x as f32 + radius
                } else if xx > x2.saturating_sub(radius as usize) {
                    x2 as f32 - radius
                } else {
                    xx as f32
                };
                let cy = if yy < y + radius as usize {
                    y as f32 + radius
                } else if yy > y2.saturating_sub(radius as usize) {
                    y2 as f32 - radius
                } else {
                    yy as f32
                };
                let dx = xx as f32 - cx;
                let dy = yy as f32 - cy;
                if dx * dx + dy * dy <= radius * radius {
                    self.set_pixel(xx, yy, color);
                }
            }
        }
    }

    fn set_pixel(&mut self, x: usize, y: usize, color: [u8; 4]) {
        let offset = (y * self.size + x) * 4;
        self.rgba[offset..offset + 4].copy_from_slice(&color);
    }
}

fn draw_service_icon(canvas: &mut IconCanvas) {
    canvas.fill_circle(17.0, 17.0, 17.0, [229, 247, 239, 255]);
    canvas.fill_round_rect(9, 9, 16, 16, 3, [29, 142, 103, 255]);
    canvas.fill_round_rect(12, 12, 10, 3, 1, [246, 255, 251, 255]);
    canvas.fill_round_rect(12, 17, 10, 3, 1, [246, 255, 251, 255]);
    canvas.fill_rect(12, 22, 3, 2, [246, 255, 251, 255]);
}

fn text_field_row(parent: &Panel, sizer: &FlexGridSizer, label: &str, value: &str) -> TextCtrl {
    let label_widget = StaticText::builder(parent).with_label(label).build();
    label_widget.set_foreground_color(Colour::rgb(78, 86, 98));
    sizer.add(
        &label_widget,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        0,
    );

    let input = TextCtrl::builder(parent)
        .with_value(value)
        .with_style(TextCtrlStyle::Default)
        .build();
    input.set_min_size(Size::new(420, 30));
    sizer.add(&input, 1, SizerFlag::Expand, 0);
    input
}

fn refresh_dashboard(api: &ApiClient, handles: &UiHandles) {
    let snapshot = api.dashboard();
    update_dashboard(handles, &snapshot);
}

fn repair_gui_environment_if_needed(api: &ApiClient, handles: &UiHandles) {
    let snapshot = api.dashboard();
    let Some(status) = snapshot.codex_app.as_ref() else {
        return;
    };
    let needs_repair = status.config_ok
        && status.auth_ok
        && (!status.gui_api_base.configured || !status.gui_api_base.login_issuer_configured);
    if !needs_repair {
        return;
    }

    match api.repair_codex_app_gui_environment() {
        Ok(_) => {
            handles
                .status_bar
                .set_status_text("Codex App：已补写本地连接入口，重启 Codex App 生效", 2);
        }
        Err(err) => {
            handles
                .status_bar
                .set_status_text(&format!("Codex App：补写本地连接入口失败：{err}"), 2);
        }
    }
}

fn update_dashboard(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    if !snapshot.service_online {
        set_status_panel(
            &handles.service_status,
            "未运行",
            "点击“启动本地服务”后再连接 Codex App。",
            StateTone::Error,
        );
        set_status_panel(
            &handles.feishu_status,
            "不可用",
            "本地服务未运行",
            StateTone::Muted,
        );
        set_status_panel(
            &handles.codex_status,
            "不可用",
            "本地服务未运行",
            StateTone::Muted,
        );
        handles.feishu_state.set_label("本地服务未运行");
        handles
            .feishu_detail
            .set_label("请先启动 codex-remote 后端。");
        handles.feishu_meta.set_label("");
        let config_state = snapshot
            .error
            .as_deref()
            .map(|err| format!("无法读取 Codex App 配置状态。\n本地服务连接错误: {err}"))
            .unwrap_or_else(|| "无法读取 Codex App 配置状态。".to_string());
        handles.codex_config_state.set_label(&config_state);
        handles.status_bar.set_status_text("本地服务：离线", 0);
        handles.status_bar.set_status_text("飞书：不可用", 1);
        handles.status_bar.set_status_text("Codex App：不可用", 2);
        set_actions_enabled(handles, false);
        handles.start_daemon_button.enable(true);
        return;
    }

    set_actions_enabled(handles, true);
    handles.start_daemon_button.enable(false);

    if let Some(status) = &snapshot.status {
        set_status_panel(
            &handles.service_status,
            "运行中",
            &format!("监听 {}", status.bind),
            StateTone::Ok,
        );
        handles
            .status_bar
            .set_status_text(&format!("本地服务：{}", status.bind), 0);
    }

    let feishu_configured = snapshot
        .backend
        .as_ref()
        .map(|backend| backend.feishu_configured)
        .or_else(|| {
            snapshot
                .config
                .as_ref()
                .map(|config| !config.feishu.app_id.is_empty())
        })
        .unwrap_or(false);
    let bridge_enabled = snapshot
        .backend
        .as_ref()
        .map(|backend| backend.enabled)
        .or_else(|| snapshot.config.as_ref().map(|config| config.bridge.enabled))
        .unwrap_or(false);

    let feishu_ws = snapshot.status.as_ref().map(|status| &status.feishu_ws);
    let (feishu_state, feishu_detail, feishu_tone) = if !feishu_configured {
        (
            "未接入",
            "扫码接入飞书机器人后才会启动飞书桥接。",
            StateTone::Warn,
        )
    } else if !bridge_enabled {
        (
            "已断开",
            "机器人已保存，点击“更换机器人”可重新接入。",
            StateTone::Muted,
        )
    } else if feishu_ws.is_some_and(|ws| ws.connected) {
        ("已接入", "飞书桥接运行中。", StateTone::Ok)
    } else if feishu_ws.is_some_and(|ws| ws.connecting) {
        ("连接中", "正在连接飞书。", StateTone::Warn)
    } else {
        (
            "等待连接",
            "机器人已保存，等待飞书桥接启动。",
            StateTone::Warn,
        )
    };

    set_status_panel(
        &handles.feishu_status,
        feishu_state,
        feishu_detail,
        feishu_tone,
    );
    handles
        .status_bar
        .set_status_text(&format!("飞书：{feishu_state}"), 1);
    handles.feishu_state.set_label(feishu_state);
    handles
        .feishu_state
        .set_foreground_color(feishu_tone.colour());
    handles.feishu_detail.set_label(feishu_detail);
    handles.feishu_detail.wrap(300);
    handles
        .stop_bridge_button
        .enable(feishu_configured && bridge_enabled);

    let feishu_meta = match (
        &snapshot.config,
        feishu_ws.and_then(|ws| ws.last_error.as_deref()),
    ) {
        (Some(config), Some(err)) if !err.is_empty() => format!(
            "App ID: {}\n允许用户: {}\n最近错误: {err}",
            short_id(&config.feishu.app_id),
            config.feishu.allowed_open_ids.len()
        ),
        (Some(config), _) if !config.feishu.app_id.is_empty() => format!(
            "App ID: {}\n允许用户: {}",
            short_id(&config.feishu.app_id),
            config.feishu.allowed_open_ids.len()
        ),
        _ => "未保存飞书机器人凭据。".to_string(),
    };
    handles.feishu_meta.set_label(&feishu_meta);
    handles.feishu_meta.wrap(300);

    let codex_connected = snapshot
        .remote
        .as_ref()
        .map(|remote| remote.connected && remote.initialized)
        .unwrap_or(false);
    let codex_configured = snapshot
        .codex_app
        .as_ref()
        .map(|status| status.configured)
        .unwrap_or(false);

    if codex_connected {
        let detail = snapshot
            .remote
            .as_ref()
            .map(codex_remote_detail)
            .unwrap_or_else(|| "Codex App remote-control 已连接。".to_string());
        set_status_panel(&handles.codex_status, "已连接", &detail, StateTone::Ok);
        handles.status_bar.set_status_text("Codex App：已连接", 2);
    } else if codex_configured {
        set_status_panel(
            &handles.codex_status,
            "等待连接",
            "配置已注入，打开 Codex App 的远程控制后会连接到本机服务。",
            StateTone::Warn,
        );
        handles.status_bar.set_status_text("Codex App：等待连接", 2);
    } else {
        set_status_panel(
            &handles.codex_status,
            "未注入",
            "填写 Base URL 和 API Key 后写入配置。",
            StateTone::Warn,
        );
        handles.status_bar.set_status_text("Codex App：未注入", 2);
    }

    handles
        .codex_config_state
        .set_label(&codex_app_detail(snapshot));
    handles.codex_config_state.wrap(500);
    fill_provider_form_if_empty(handles, snapshot);
}

fn fill_provider_form_if_empty(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    let Some(provider) = snapshot
        .codex_app
        .as_ref()
        .and_then(|status| status.provider.as_ref())
    else {
        return;
    };
    if handles.provider_name.get_value().trim().is_empty() {
        handles.provider_name.change_value(&provider.name);
    }
    if handles.provider_base_url.get_value().trim().is_empty()
        && let Some(base_url) = provider.base_url.as_deref()
    {
        handles.provider_base_url.change_value(base_url);
    }
    if handles.provider_key.get_value().trim().is_empty()
        && let Some(key) = provider.key.as_deref()
    {
        handles.provider_key.change_value(key);
    }
}

fn set_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.change_bot_button.enable(enabled);
    handles.configure_button.enable(enabled);
    handles.refresh_button.enable(true);
    handles.uninstall_button.enable(enabled);
    handles.stop_bridge_button.enable(enabled);
}

#[derive(Clone, Copy)]
enum StateTone {
    Ok,
    Warn,
    Error,
    Muted,
}

impl StateTone {
    fn colour(self) -> Colour {
        match self {
            StateTone::Ok => Colour::rgb(28, 127, 89),
            StateTone::Warn => Colour::rgb(169, 104, 24),
            StateTone::Error => Colour::rgb(185, 55, 55),
            StateTone::Muted => Colour::rgb(102, 110, 122),
        }
    }
}

fn set_status_panel(panel: &StatusPanel, state: &str, detail: &str, tone: StateTone) {
    panel.marker.set_foreground_color(tone.colour());
    panel.state.set_label(state);
    panel.state.set_foreground_color(tone.colour());
    panel.detail.set_label(detail);
    panel.detail.wrap(220);
}

fn codex_remote_detail(remote: &RemoteControlStatus) -> String {
    if let Some(thread_id) = &remote.current_thread_id {
        return format!("当前 thread: {}", short_id(thread_id));
    }
    if let Some(server_name) = &remote.server_name {
        return format!("设备: {server_name}");
    }
    if let Some(err) = &remote.last_error {
        return format!("最近错误: {err}");
    }
    "Codex App remote-control 已连接。".to_string()
}

fn codex_app_detail(snapshot: &DashboardSnapshot) -> String {
    let Some(status) = &snapshot.codex_app else {
        return "无法读取 ~/.codex 配置状态。".to_string();
    };
    if status.configured {
        let mut detail = format!("已注入到 {}", status.codex_home);
        if let Some(value) = &status.gui_api_base.value {
            detail.push_str(&format!("\nGUI backend: {value}"));
        }
        return detail;
    }

    let mut parts = Vec::new();
    if !status.config_ok {
        parts.push("Codex App 还没有写入本地连接配置。".to_string());
    }
    if !status.auth_ok {
        parts.push("本地认证信息还没有准备好，请填写 API Key 后写入配置。".to_string());
    }
    if !status.gui_api_base.configured {
        parts.push("Codex App GUI 还没有指向本地服务，请点击写入配置。".to_string());
    }
    if !status.gui_api_base.login_issuer_configured {
        parts.push("本地授权入口还没有写入，请点击写入配置。".to_string());
    }
    if let Some(err) = &status.gui_api_base.error {
        parts.push(format!("检查环境变量时遇到问题: {err}"));
    }
    if parts.is_empty() {
        "尚未注入 Codex App 配置。".to_string()
    } else {
        parts.join("\n")
    }
}

fn qr_bitmap(value: &str) -> Option<(Bitmap, i32)> {
    let code = QrCode::new(value.as_bytes()).ok()?;
    let quiet_zone = 4usize;
    let cells = code.width() + quiet_zone * 2;
    let module_size = (420usize / cells).clamp(3, 8);
    let image_size = cells * module_size;
    let mut rgba = vec![255u8; image_size * image_size * 4];

    for y in 0..image_size {
        for x in 0..image_size {
            let cell_x = x / module_size;
            let cell_y = y / module_size;
            let dark = cell_x >= quiet_zone
                && cell_y >= quiet_zone
                && cell_x < quiet_zone + code.width()
                && cell_y < quiet_zone + code.width()
                && code[(cell_x - quiet_zone, cell_y - quiet_zone)] == Color::Dark;

            let offset = (y * image_size + x) * 4;
            let value = if dark { 0 } else { 255 };
            rgba[offset] = value;
            rgba[offset + 1] = value;
            rgba[offset + 2] = value;
            rgba[offset + 3] = 255;
        }
    }

    Bitmap::from_rgba(&rgba, image_size as u32, image_size as u32)
        .map(|bitmap| (bitmap, image_size as i32))
}

fn show_onboard_dialog(parent: &Frame, api: ApiClient) {
    let start = match api.start_feishu_onboard() {
        Ok(start) => start,
        Err(err) => {
            show_error(parent, &err);
            return;
        }
    };

    let dialog = Dialog::builder(parent, "更换飞书机器人")
        .with_size(500, 560)
        .build();
    dialog.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&dialog)
        .with_label("请使用飞书扫码")
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(&title, 0, SizerFlag::All, 18);

    if let Some((bitmap, qr_size)) = qr_bitmap(&start.verification_uri_complete) {
        let qr = StaticBitmap::builder(&dialog)
            .with_bitmap(Some(bitmap))
            .with_scale_mode(Some(ScaleMode::None))
            .with_size(Size::new(qr_size, qr_size))
            .build();
        qr.set_min_size(Size::new(qr_size, qr_size));
        sizer.add(
            &qr,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            12,
        );
    } else {
        let qr_error = StaticText::builder(&dialog)
            .with_label("二维码生成失败，请使用浏览器打开链接。")
            .build();
        qr_error.set_foreground_color(Colour::rgb(185, 55, 55));
        sizer.add(
            &qr_error,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            80,
        );
    }

    let info = StaticText::builder(&dialog)
        .with_label("扫码完成后会自动关闭。")
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(480);
    sizer.add(
        &info,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&dialog).with_label("关闭").build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    dialog.set_sizer(sizer, true);

    let timer = Timer::new(&dialog);
    {
        let api = api.clone();
        let device_code = start.device_code.clone();
        let dialog = dialog;
        timer.on_tick(move |_| match api.poll_feishu_onboard(&device_code) {
            Ok(result) if result.done => {
                dialog.end_modal(ID_OK);
            }
            Ok(result) => {
                if is_feishu_onboard_pending(result.error.as_ref()) {
                    info.set_label("扫码完成后会自动关闭。");
                } else if result.error.is_some() {
                    info.set_label("接入失败，请关闭后重试。");
                }
            }
            Err(_) => {
                info.set_label("接入失败，请关闭后重试。");
            }
        });
    }
    timer.start(1500, false);

    {
        let dialog = dialog;
        close_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
    }

    dialog.show_modal();
    timer.stop();
    dialog.destroy();
}

fn is_feishu_onboard_pending(error: Option<&serde_json::Value>) -> bool {
    matches!(
        error.and_then(|value| value.as_str()),
        Some("authorization_pending" | "slow_down")
    )
}

fn show_info(parent: &dyn WxWidget, message: &str) {
    MessageDialog::builder(parent, message, "Codex Remote")
        .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
        .build()
        .show_modal();
}

fn show_error(parent: &dyn WxWidget, message: &str) {
    MessageDialog::builder(parent, message, "Codex Remote")
        .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
        .build()
        .show_modal();
}

fn short_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 18 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..18])
    }
}
