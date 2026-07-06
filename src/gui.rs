use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    process::Child,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use tokio::sync::mpsc as tokio_mpsc;
use wxdragon::widgets::dataview::{
    CustomDataViewVirtualListModel, DataViewAlign, DataViewColumnFlags, DataViewCtrl, Variant,
};
use wxdragon::{prelude::*, timer::Timer};

#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
    System::Threading::CreateMutexW,
};

use crate::ai_gateway::config::{
    DEFAULT_PROVIDER_TIMEOUT_SECS, ProviderConfig, ProviderType, provider_api_root,
    provider_display_base_url,
};
use crate::config::{AppConfig, LocalConnectionMode};

#[cfg(target_os = "windows")]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
#[cfg(not(target_os = "windows"))]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
const CODEX_APP_GUI_UNSUPPORTED: bool = !(cfg!(target_os = "macos") || cfg!(target_os = "windows"));
const PROJECT_HOME_URL: &str = "https://github.com/happy-loki/codexhub";
#[cfg(target_os = "windows")]
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/happy-loki/codexhub/releases/latest/download/latest-windows.json";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/happy-loki/codexhub/releases/latest/download/latest-macos-intel.json";
#[cfg(all(target_os = "macos", not(target_arch = "x86_64")))]
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/happy-loki/codexhub/releases/latest/download/latest-macos.json";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/happy-loki/codexhub/releases/latest/download/latest-linux.json";
const LEGACY_UPDATE_MANIFEST_URL: &str =
    "https://github.com/happy-loki/codexhub/releases/latest/download/latest.json";
const UPDATE_RELEASE_API_URL: &str =
    "https://api.github.com/repos/happy-loki/codexhub/releases/latest";
const UPDATE_RELEASE_PAGE_URL: &str = "https://github.com/happy-loki/codexhub/releases/latest";
const DASHBOARD_REFRESH_INTERVAL_MS: i32 = 10_000;
const REQUEST_LOG_REFRESH_INTERVAL_MS: i32 = 5_000;
const REQUEST_LOG_TAB_INDEX: i32 = 3;
/// Poll interval for the short-lived "fetch models" dialog timer. This timer
/// only runs while that modal dialog is open, so it does not affect idle CPU.
const DIALOG_RESULT_POLL_MS: i32 = 150;
const GUI_CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const GUI_STATUS_TIMEOUT: Duration = Duration::from_millis(650);
const GUI_ACTION_TIMEOUT: Duration = Duration::from_secs(2);
const GUI_CONFIG_TIMEOUT: Duration = Duration::from_secs(15);
const GUI_STARTUP_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);
const GUI_MODEL_LIST_FETCH_TIMEOUT_SECS: u64 = 30;
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const ID_MENU_CLOSE_WINDOW: i32 = 10_001;
const ID_MENU_MINIMIZE: i32 = 10_002;
const ID_MENU_CHECK_UPDATE: i32 = 10_003;
const ID_MENU_LANGUAGE_ZH_CN: i32 = 10_004;
const ID_MENU_LANGUAGE_EN_US: i32 = 10_005;
const ID_MENU_THEME_SYSTEM: i32 = 10_006;
const ID_MENU_THEME_LIGHT: i32 = 10_007;
const ID_MENU_THEME_DARK: i32 = 10_008;
const ID_SERVICE_CONNECTION_SWITCH: i32 = 10_009;
const ID_MENU_QUIT: i32 = 10_010;

type ImAccountRows = Rc<RefCell<Vec<[String; 5]>>>;
type ImAccountModel = Rc<RefCell<CustomDataViewVirtualListModel>>;
type PendingImToggle = Rc<RefCell<Option<ImAccountToggle>>>;
type ModelMappingRows = Rc<RefCell<Vec<ModelMappingRow>>>;
type ModelMappingModel = Rc<RefCell<CustomDataViewVirtualListModel>>;

type FrameTimerStore = Rc<RefCell<Option<Timer<Frame>>>>;
type RequestLogResultStore = Arc<Mutex<Option<Result<Vec<RequestLogItem>, String>>>>;
type RequestLogDetailResultStore =
    Arc<Mutex<Option<(i64, Result<self::api::RequestLogDetail, String>)>>>;
type RequestLogClearResultStore = Arc<Mutex<Option<Result<usize, String>>>>;
type FetchModelsResultStore = Arc<Mutex<Option<Result<(Vec<String>, String), String>>>>;

/// Messages sent from background threads to the GUI thread via idle events
enum GuiMessage {
    CodexAction(CodexActionResult),
    ImAction(ImActionResult),
    AiGwAction(AiGwActionResult),
    DashboardUpdate,
}

#[derive(Clone, PartialEq, Eq)]
struct ModelMappingRow {
    upstream_model: String,
    codex_models: Vec<String>,
}

#[derive(Clone)]
struct ImAccountToggle {
    row: usize,
    platform: String,
    account_id: String,
    enabled: bool,
    previous_enabled: bool,
}

mod ai_gateway;
mod api;
mod codex_tab;
mod daemon;
mod im_accounts;
mod onboarding;
mod provider;
mod request_log_detail;
mod request_logs;
mod session_history;
mod text;
mod theme;
mod tray;
mod update;
mod widgets;

use self::ai_gateway::{
    AiGwActionResult, AiGwChannelToggle, AiGwProviderModel, AiGwProviderRow, AiGwProviderRows,
    PendingAiGwChannelToggle, apply_pending_ai_gw_action, delete_ai_gw_provider,
    provider_logo_variant, provider_protocol_display, refresh_ai_gw_enable_logging,
    refresh_ai_gw_filter_image_generation, refresh_ai_gw_provider_list, save_ai_gw_provider,
    set_ai_gw_actions_enabled, set_ai_gw_provider_enabled, set_filter_image_generation_tool,
    set_request_logging_enabled,
};
use self::api::{
    ApiClient, ConfigureTelegramBotRequest, DashboardSnapshot, DeleteImAccountRequest,
    RemoteControlConnectionStatus, RemoteControlStatus, RequestLogItem, SetImAccountEnabledRequest,
};
use self::codex_tab::{CodexActionResult, CodexTab};
use self::daemon::{
    app_support_config_path, daemon_config_path, start_daemon_for_gui_async, stop_daemon_on_exit,
    stop_pending_startup_daemon,
};
use self::im_accounts::{
    apply_pending_im_action, im_platform_key, refresh_im_account_list, selected_im_account,
};
use self::onboarding::{
    prompt_telegram_bot_token, show_feishu_onboard_dialog, show_wechat_onboard_dialog,
};
use self::provider::strip_nul;
use self::request_logs::{
    RequestLogModel, RequestLogRows, refresh_request_log_list, request_log_cell,
};
use self::session_history::show_session_history_window;
use self::text::{GuiLocale, GuiText};
use self::theme::ThemeMode;
use self::widgets::{
    ImStatusPanel, LucideIconKind, ProviderLogoKind, StateTone, StatusIconKind, StatusPanel,
    app_icon_bitmap, apply_dataview_theme, apply_notebook_theme, card_section,
    centered_status_panel, dataview_table_style, im_status_panel, lucide_icon_bitmap,
    provider_logo_bitmap, set_disabled_status_panel, set_im_channel_row, set_status_panel,
    status_icon_bitmap, status_panel, table_cell_attr, text_field_row, topology_connector,
    topology_splitter,
};

#[derive(Clone)]
struct GuiTimers {
    stores: Rc<RefCell<Vec<FrameTimerStore>>>,
}

impl GuiTimers {
    fn new() -> Self {
        Self {
            stores: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn track(&self, store: &FrameTimerStore) {
        self.stores.borrow_mut().push(store.clone());
    }

    fn stop_all(&self) {
        let stores = self.stores.borrow().clone();
        for store in stores {
            if let Some(timer) = store.borrow().as_ref() {
                timer.stop();
            }
        }
        self.stores.borrow_mut().clear();
    }
}

#[cfg(target_os = "windows")]
struct GuiSingleInstanceGuard {
    handle: HANDLE,
    another_running: bool,
}

#[cfg(target_os = "windows")]
impl GuiSingleInstanceGuard {
    fn acquire() -> Option<Self> {
        let name: Vec<u16> = "Local\\CodexHubGuiSingleInstance"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
        if handle.is_null() {
            return None;
        }
        let another_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        Some(Self {
            handle,
            another_running,
        })
    }

    fn is_another_running(&self) -> bool {
        self.another_running
    }
}

#[cfg(target_os = "windows")]
impl Drop for GuiSingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

#[cfg(not(target_os = "windows"))]
struct GuiSingleInstanceGuard {
    checker: SingleInstanceChecker,
}

#[cfg(not(target_os = "windows"))]
impl GuiSingleInstanceGuard {
    fn acquire() -> Option<Self> {
        SingleInstanceChecker::new("com.codexhub.gui", None).map(|checker| Self { checker })
    }

    fn is_another_running(&self) -> bool {
        self.checker.is_another_running()
    }
}

pub fn run() {
    let Some(single_instance_guard) = GuiSingleInstanceGuard::acquire() else {
        eprintln!("failed to create CodexHub GUI single instance checker");
        return;
    };
    if single_instance_guard.is_another_running() {
        return;
    }

    if let Err(err) = wxdragon::main(|app| build_ui(app, single_instance_guard)) {
        eprintln!("failed to start CodexHub GUI: {err:?}");
    }
}

fn build_ui(app: App, single_instance_guard: GuiSingleInstanceGuard) {
    // Apply the saved appearance and install the matching design tokens *before*
    // any window is built: `set_appearance` returns `CannotChange` once a window
    // exists, so this ordering is required.
    let theme_mode = load_gui_theme();
    let _ = set_appearance(theme_mode.appearance());
    theme::init(theme_mode);
    let app_theme = theme::theme();

    let locale = load_gui_locale();
    let text = GuiText::new(locale);
    let api = ApiClient::new(default_base_url(), text);
    let gui_timers = GuiTimers::new();

    // Event-driven async message handling: background threads send results over
    // this channel and wake the idle loop, replacing the old polling timers.
    let (gui_tx, gui_rx) = tokio_mpsc::unbounded_channel::<GuiMessage>();

    // Fit the initial window to the current screen work area so first-time users
    // on small or scaled laptop displays never open into a window that is taller
    // than the screen (which pushes the primary action buttons off-screen and
    // forces scrolling to find them).
    let frame_size = initial_frame_size();
    let frame = Frame::builder()
        .with_title("CodexHub")
        // Keep the first launch within smaller laptop work areas. The tab pages
        // own their scrolling, so the frame itself should not exceed the screen.
        .with_size(frame_size)
        .build();
    // Never let the window shrink below a floor where the primary buttons stop
    // fitting, but never force it larger than the screen work area either.
    frame.set_min_size(min_frame_size(frame_size));
    app.set_top_window(&frame);
    frame.set_icon(&app_icon_bitmap(48));
    let update_check_in_flight = Arc::new(AtomicBool::new(false));
    let quitting = Rc::new(AtomicBool::new(false));
    install_system_menu(
        &frame,
        &gui_timers,
        text,
        update_check_in_flight.clone(),
        quitting.clone(),
    );
    let tray_controller = tray::install(
        &frame,
        &gui_timers,
        text,
        update_check_in_flight.clone(),
        quitting.clone(),
    );
    frame.set_background_color(app_theme.bg_app);
    let _status_bar = StatusBar::builder(&frame)
        .with_fields_count(1)
        .with_status_widths(vec![-1])
        .add_initial_text(0, &text.version())
        .build();

    let root = Panel::builder(&frame).build();
    root.set_background_color(app_theme.bg_app);
    // wxMSW omits wxFULL_REPAINT_ON_RESIZE by default, so a resize only repaints
    // the newly exposed strip and the rest of the window keeps stale pixels,
    // smearing labels and bitmaps into vertical "ghost" streaks. Enable full
    // repaint-on-resize for the frame, the root panel, and each notebook page.
    enable_full_repaint_on_resize(&frame);
    enable_full_repaint_on_resize(&root);

    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let (status_box, status_section) = card_section(&root, text.status_overview());
    let status_row = BoxSizer::builder(Orientation::Horizontal).build();
    let codex_status = status_panel(
        &status_box,
        text.codex_control_channel(),
        StatusIconKind::Codex,
        text,
    );
    let vscode_status = status_panel(
        &status_box,
        text.vscode_extension(),
        StatusIconKind::VsCodeCodex,
        text,
    );
    let cli_status = status_panel(
        &status_box,
        text.codex_cli(),
        StatusIconKind::CodexCli,
        text,
    );
    if CODEX_APP_GUI_UNSUPPORTED {
        set_disabled_status_panel(
            &codex_status,
            text.unavailable(),
            text.app_gui_unsupported(),
        );
    }
    let service_status = centered_status_panel(
        &status_box,
        text.local_service(),
        StatusIconKind::Service,
        text,
    );
    let service_settings_button = Button::builder(&service_status.panel)
        .with_label(text.local_connection_settings())
        .build();
    service_settings_button.set_tooltip(text.local_connection_settings_help());
    let service_settings_row = BoxSizer::builder(Orientation::Horizontal).build();
    service_settings_row.add_stretch_spacer(1);
    service_settings_row.add(
        &service_settings_button,
        0,
        SizerFlag::AlignCenterVertical,
        0,
    );
    service_status.extra.add_sizer(
        &service_settings_row,
        0,
        SizerFlag::Expand | SizerFlag::Top,
        2,
    );
    service_settings_button.hide();
    let im_status = im_status_panel(&status_box, text);
    let entry_connector = topology_connector(&status_box);
    let bridge_connector = topology_splitter(&status_box);
    let entry_column = BoxSizer::builder(Orientation::Vertical).build();
    entry_column.add(
        &codex_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::Bottom,
        4,
    );
    entry_column.add(
        &vscode_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::Bottom,
        4,
    );
    entry_column.add(&cli_status.panel, 1, SizerFlag::Expand, 0);
    status_row.add_sizer(&entry_column, 1, SizerFlag::Expand | SizerFlag::All, 6);
    status_row.add(
        &entry_connector,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Left | SizerFlag::Right,
        4,
    );
    status_row.add(
        &service_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        6,
    );
    status_row.add(
        &bridge_connector,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Left | SizerFlag::Right,
        2,
    );
    status_row.add(&im_status.panel, 1, SizerFlag::Expand | SizerFlag::All, 6);
    status_section.add_sizer(
        &status_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        6,
    );
    root_sizer.add(
        &status_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );

    let notebook = Notebook::builder(&root).build();
    apply_notebook_theme(&notebook);
    let tab_icons = create_main_tab_icons(&notebook);

    let codex_tab = codex_tab::create(&notebook, text);
    enable_full_repaint_on_resize(&codex_tab.page);

    // --- AI Gateway Tab ---
    let ai_gw_page = ScrolledWindow::builder(&notebook)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    ai_gw_page.set_background_color(theme::theme().bg_card_alt);
    enable_full_repaint_on_resize(&ai_gw_page);
    let ai_gw_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let (ai_gw_behavior_box, ai_gw_behavior_section) =
        card_section(&ai_gw_page, text.ai_gw_behavior());
    let ai_gw_filter_image_generation = CheckBox::builder(&ai_gw_behavior_box)
        .with_label(text.image_generation_feature())
        .with_value(false)
        .build();
    ai_gw_filter_image_generation.set_background_color(theme::theme().bg_card);
    ai_gw_filter_image_generation.set_foreground_color(theme::theme().ink_primary);
    ai_gw_filter_image_generation.set_tooltip(text.image_generation_feature_help());
    let ai_gw_filter_image_generation_note = StaticText::builder(&ai_gw_behavior_box)
        .with_label(text.image_generation_feature_note())
        .build();
    ai_gw_filter_image_generation_note.set_foreground_color(theme::theme().ink_muted);
    let ai_gw_behavior_row = BoxSizer::builder(Orientation::Horizontal).build();
    ai_gw_behavior_row.add(
        &ai_gw_filter_image_generation,
        0,
        SizerFlag::Right | SizerFlag::AlignCenterVertical,
        8,
    );
    ai_gw_behavior_row.add(
        &ai_gw_filter_image_generation_note,
        0,
        SizerFlag::AlignCenterVertical,
        0,
    );
    ai_gw_behavior_section.add_sizer(
        &ai_gw_behavior_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        10,
    );
    ai_gw_sizer.add(
        &ai_gw_behavior_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let (ai_gw_list_box, ai_gw_list_section) = card_section(&ai_gw_page, text.ai_gw_channel_list());

    let ai_gw_provider_rows: AiGwProviderRows = Rc::new(RefCell::new(Vec::new()));
    let pending_ai_gw_channel_toggle: PendingAiGwChannelToggle = Rc::new(RefCell::new(None));
    let pending_ai_gw_channel_toggle_for_model = pending_ai_gw_channel_toggle.clone();
    let ai_gw_provider_model: AiGwProviderModel =
        Rc::new(RefCell::new(CustomDataViewVirtualListModel::new(
            0,
            ai_gw_provider_rows.clone(),
            |rows: &AiGwProviderRows, row, col| -> Variant {
                let rows = rows.borrow();
                let Some(row_data) = rows.get(row) else {
                    return String::new().into();
                };
                match col {
                    0 => row_data.enabled.into(),
                    1 => row_data.name.clone().into(),
                    2 => provider_logo_variant(row_data),
                    3 => provider_protocol_display(
                        &row_data.provider_type,
                        row_data.compatibility.as_deref(),
                    )
                    .into(),
                    4 => row_data.base_url.clone().into(),
                    5 => row_data.weight.to_string().into(),
                    _ => String::new().into(),
                }
            },
            Some(
                move |rows: &AiGwProviderRows, row, col, value: &Variant| -> bool {
                    if col != 0 {
                        return false;
                    }
                    let Some(enabled) = value.get_bool() else {
                        return false;
                    };
                    let mut rows = std::cell::RefCell::borrow_mut(std::rc::Rc::as_ref(rows));
                    let Some(row_data): Option<&mut AiGwProviderRow> = rows.get_mut(row) else {
                        return false;
                    };
                    let name = row_data.name.clone();
                    if name.trim().is_empty() {
                        return false;
                    }
                    let previous_enabled = row_data.enabled;
                    if previous_enabled == enabled {
                        return true;
                    }
                    row_data.enabled = enabled;
                    pending_ai_gw_channel_toggle_for_model.borrow_mut().replace(
                        AiGwChannelToggle {
                            row,
                            name,
                            enabled,
                            previous_enabled,
                        },
                    );
                    wxdragon::wake_up_idle();
                    true
                },
            ),
            Some(|_: &AiGwProviderRows, row, _| table_cell_attr(row)),
            None::<fn(&AiGwProviderRows, usize, usize) -> bool>,
        )));
    let ai_gw_provider_list = DataViewCtrl::builder(&ai_gw_list_box)
        .with_style(dataview_table_style(false))
        .with_size(Size::new(-1, 330))
        .build();
    apply_dataview_theme(&ai_gw_provider_list);
    ai_gw_provider_list.append_toggle_column(
        text.enable(),
        0,
        70,
        DataViewAlign::Center,
        DataViewColumnFlags::Resizable,
    );
    ai_gw_provider_list.append_text_column(
        text.ai_gw_col_name(),
        1,
        160,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    ai_gw_provider_list.append_bitmap_column(
        text.ai_gw_provider_service(),
        2,
        72,
        DataViewAlign::Center,
        DataViewColumnFlags::Resizable,
    );
    ai_gw_provider_list.append_text_column(
        text.ai_gw_api_protocol(),
        3,
        190,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    ai_gw_provider_list.append_text_column(
        text.ai_gw_col_base_url(),
        4,
        360,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    ai_gw_provider_list.append_text_column(
        text.ai_gw_weight(),
        5,
        80,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    ai_gw_provider_list.associate_model(&*ai_gw_provider_model.borrow());
    let ai_gw_new_button = Button::builder(&ai_gw_list_box)
        .with_label(text.ai_gw_add_channel())
        .build();
    let ai_gw_edit_button = Button::builder(&ai_gw_list_box)
        .with_label(text.ai_gw_edit_channel())
        .build();
    let ai_gw_delete_button = Button::builder(&ai_gw_list_box)
        .with_label(text.ai_gw_delete_channel())
        .build();
    let ai_gw_list_actions = BoxSizer::builder(Orientation::Horizontal).build();
    ai_gw_list_actions.add_stretch_spacer(1);
    ai_gw_list_actions.add(&ai_gw_new_button, 0, SizerFlag::Right, 8);
    ai_gw_list_actions.add(&ai_gw_edit_button, 0, SizerFlag::Right, 8);
    ai_gw_list_actions.add(&ai_gw_delete_button, 0, SizerFlag::Right, 0);
    ai_gw_list_section.add_sizer(
        &ai_gw_list_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        10,
    );
    ai_gw_list_section.add(
        &ai_gw_provider_list,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );

    ai_gw_sizer.add(
        &ai_gw_list_box,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );
    ai_gw_page.set_sizer(ai_gw_sizer, true);
    ai_gw_page.set_scroll_rate(0, 10);
    ai_gw_page.layout();
    ai_gw_page.fit_inside();

    let feishu_page = ScrolledWindow::builder(&notebook)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    feishu_page.set_background_color(theme::theme().bg_card_alt);
    enable_full_repaint_on_resize(&feishu_page);
    let feishu_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let im_access_hint = StaticText::builder(&feishu_page)
        .with_label(text.im_access_hint())
        .build();
    im_access_hint.set_foreground_color(theme::theme().ink_secondary);
    im_access_hint.wrap(920);
    feishu_sizer.add(
        &im_access_hint,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let (im_accounts_static_box, im_accounts_box) = card_section(&feishu_page, text.bot_pool());
    let im_account_rows: ImAccountRows = Rc::new(RefCell::new(Vec::new()));
    let pending_im_toggle: PendingImToggle = Rc::new(RefCell::new(None));
    let pending_im_toggle_for_model = pending_im_toggle.clone();
    let im_account_model: ImAccountModel =
        Rc::new(RefCell::new(CustomDataViewVirtualListModel::new(
            0,
            im_account_rows.clone(),
            |rows: &ImAccountRows, row, col| -> Variant {
                if col == 4 {
                    return rows
                        .borrow()
                        .get(row)
                        .and_then(|row_data| row_data.get(4))
                        .map(|value| value == "true")
                        .unwrap_or(false)
                        .into();
                }
                rows.borrow()
                    .get(row)
                    .and_then(|row_data| row_data.get(col))
                    .cloned()
                    .unwrap_or_default()
                    .into()
            },
            Some(
                move |rows: &ImAccountRows, row, col, value: &Variant| -> bool {
                    if col != 4 {
                        return false;
                    }
                    let Some(enabled) = value.get_bool() else {
                        return false;
                    };
                    let mut rows = std::cell::RefCell::borrow_mut(std::rc::Rc::as_ref(rows));
                    let Some(row_data): Option<&mut [String; 5]> = rows.get_mut(row) else {
                        return false;
                    };
                    let Some(platform) = im_platform_key(&row_data[1]) else {
                        return false;
                    };
                    let account_id = row_data[3].clone();
                    if account_id.trim().is_empty() {
                        return false;
                    }
                    let previous_enabled = row_data[4] == "true";
                    if previous_enabled == enabled {
                        return true;
                    }
                    row_data[4] = enabled.to_string();
                    pending_im_toggle_for_model
                        .borrow_mut()
                        .replace(ImAccountToggle {
                            row,
                            platform,
                            account_id,
                            enabled,
                            previous_enabled,
                        });
                    wxdragon::wake_up_idle();
                    true
                },
            ),
            Some(|_: &ImAccountRows, row, _| table_cell_attr(row)),
            None::<fn(&ImAccountRows, usize, usize) -> bool>,
        )));
    let im_account_list = DataViewCtrl::builder(&im_accounts_static_box)
        .with_style(dataview_table_style(false))
        .with_size(Size::new(-1, 190))
        .build();
    apply_dataview_theme(&im_account_list);
    im_account_list.append_text_column(
        text.bot(),
        0,
        280,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    im_account_list.append_text_column(
        text.platform(),
        1,
        90,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    im_account_list.append_text_column(
        text.state(),
        2,
        120,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    im_account_list.append_text_column(
        text.account(),
        3,
        260,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    im_account_list.append_toggle_column(
        text.access(),
        4,
        70,
        DataViewAlign::Center,
        DataViewColumnFlags::Resizable,
    );
    im_account_list.associate_model(&*im_account_model.borrow());
    im_accounts_box.add(
        &im_account_list,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );
    let im_account_actions = BoxSizer::builder(Orientation::Horizontal).build();
    im_account_actions.add_stretch_spacer(1);
    let delete_im_account_button = Button::builder(&im_accounts_static_box)
        .with_label(text.delete_selected())
        .build();
    delete_im_account_button.set_tooltip(text.delete_im_account_help());
    im_account_actions.add(&delete_im_account_button, 0, SizerFlag::Right, 0);
    im_accounts_box.add_sizer(
        &im_account_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );
    let (add_im_static_box, add_im_box) = card_section(&feishu_page, text.add_bot());
    let add_im_actions = BoxSizer::builder(Orientation::Horizontal).build();
    let change_bot_button = Button::builder(&add_im_static_box)
        .with_label(text.add_feishu_bot())
        .build();
    change_bot_button.set_tooltip(text.add_feishu_bot_help());
    let save_telegram_button = Button::builder(&add_im_static_box)
        .with_label(text.add_telegram_bot())
        .build();
    save_telegram_button.set_tooltip(text.add_telegram_bot_help());
    let connect_wechat_button = Button::builder(&add_im_static_box)
        .with_label(text.add_wechat_bot())
        .build();
    connect_wechat_button.set_tooltip(text.add_wechat_bot_help());
    add_im_actions.add(&change_bot_button, 0, SizerFlag::Right, 10);
    add_im_actions.add(&save_telegram_button, 0, SizerFlag::Right, 10);
    add_im_actions.add(&connect_wechat_button, 0, SizerFlag::Right, 0);
    add_im_box.add_sizer(
        &add_im_actions,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );
    let wechat_context_warning = StaticText::builder(&add_im_static_box)
        .with_label(text.wechat_context_token_warning())
        .build();
    wechat_context_warning.set_foreground_color(theme::theme().error);
    wechat_context_warning.wrap(920);
    add_im_box.add(
        &wechat_context_warning,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        12,
    );
    feishu_sizer.add(&add_im_static_box, 0, SizerFlag::Expand | SizerFlag::All, 8);
    feishu_sizer.add(
        &im_accounts_static_box,
        0,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    feishu_page.set_sizer(feishu_sizer, true);
    feishu_page.set_scroll_rate(0, 10);
    feishu_page.layout();
    feishu_page.fit_inside();

    // --- Request Logs Tab ---
    let request_logs_page = Panel::builder(&notebook).build();
    request_logs_page.set_background_color(theme::theme().bg_card_alt);
    enable_full_repaint_on_resize(&request_logs_page);
    let request_logs_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let request_log_hint = StaticText::builder(&request_logs_page)
        .with_label(text.request_log_open_hint())
        .build();
    request_log_hint.set_foreground_color(theme::theme().warn);

    let request_log_disabled_hint = StaticText::builder(&request_logs_page)
        .with_label(text.request_logging_disabled_hint())
        .build();
    request_log_disabled_hint.set_foreground_color(theme::theme().ink_muted);
    request_log_disabled_hint.show(false);
    let ai_gw_enable_logging = CheckBox::builder(&request_logs_page)
        .with_label(text.enable_request_logging())
        .with_value(true)
        .build();
    ai_gw_enable_logging.set_background_color(theme::theme().bg_card_alt);
    ai_gw_enable_logging.set_foreground_color(theme::theme().ink_primary);
    ai_gw_enable_logging.set_tooltip(text.enable_request_logging_help());

    let request_log_clear_old_button = Button::builder(&request_logs_page)
        .with_label(text.request_log_clear_old())
        .build();
    let request_log_clear_all_button = Button::builder(&request_logs_page)
        .with_label(text.request_log_clear_all())
        .build();
    let request_log_toolbar = BoxSizer::builder(Orientation::Horizontal).build();
    request_log_toolbar.add(&request_log_hint, 0, SizerFlag::AlignCenterVertical, 0);
    request_log_toolbar.add(
        &request_log_disabled_hint,
        0,
        SizerFlag::AlignCenterVertical,
        0,
    );
    request_log_toolbar.add_stretch_spacer(1);
    request_log_toolbar.add(
        &ai_gw_enable_logging,
        0,
        SizerFlag::Right | SizerFlag::AlignCenterVertical,
        10,
    );
    request_log_toolbar.add(&request_log_clear_old_button, 0, SizerFlag::Right, 8);
    request_log_toolbar.add(&request_log_clear_all_button, 0, SizerFlag::Right, 0);
    request_logs_sizer.add_sizer(
        &request_log_toolbar,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let request_log_rows: RequestLogRows = Rc::new(RefCell::new(Vec::new()));
    let request_log_model: RequestLogModel =
        Rc::new(RefCell::new(CustomDataViewVirtualListModel::new(
            0,
            request_log_rows.clone(),
            request_log_cell,
            None::<fn(&RequestLogRows, usize, usize, &Variant) -> bool>,
            Some(|_: &RequestLogRows, row, _| table_cell_attr(row)),
            None::<fn(&RequestLogRows, usize, usize) -> bool>,
        )));
    let request_log_list = DataViewCtrl::builder(&request_logs_page)
        .with_style(dataview_table_style(false))
        .with_size(Size::new(-1, 520))
        .build();
    apply_dataview_theme(&request_log_list);
    request_log_list.append_text_column(
        text.request_log_col_id(),
        0,
        80,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_model(),
        1,
        170,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_stream(),
        2,
        100,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_channel(),
        3,
        160,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_status(),
        4,
        120,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_tokens(),
        5,
        260,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_request_size(),
        6,
        110,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_read_cache(),
        7,
        160,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_write_cache(),
        8,
        150,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_cost(),
        9,
        110,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_ttft(),
        10,
        95,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_latency(),
        11,
        110,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.append_text_column(
        text.request_log_col_created_at(),
        12,
        170,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    request_log_list.associate_model(&*request_log_model.borrow());
    request_logs_sizer.add(
        &request_log_list,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        10,
    );
    request_logs_page.set_sizer(request_logs_sizer, true);

    notebook.add_page(&codex_tab.page, text.codex_tab(), true, tab_icons[0]);
    notebook.add_page(&ai_gw_page, text.ai_gateway_tab(), false, tab_icons[1]);
    notebook.add_page(&feishu_page, text.chat_tab(), false, tab_icons[2]);
    notebook.add_page(
        &request_logs_page,
        text.request_logs_tab(),
        false,
        tab_icons[3],
    );

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
    {
        // Belt-and-suspenders for the resize ghosting: even with
        // wxFULL_REPAINT_ON_RESIZE set above, force the whole client area to
        // erase + repaint on every size event. wx coalesces these invalidations
        // into a single paint, so this clears the vertical streaks without
        // double-drawing or flicker.
        let root = root;
        frame.on_size(move |event| {
            root.refresh(true, None);
            event.skip(true);
        });
    }

    let handles = UiHandles {
        text,
        service_status,
        service_settings_button,
        im_status,
        codex_status,
        vscode_status,
        cli_status,
        im_account_list,
        im_account_rows,
        im_account_model,
        pending_im_toggle,
        delete_im_account_button,
        save_telegram_button,
        connect_wechat_button,
        change_bot_button,
        codex_tab,
        ai_gw_provider_list,
        ai_gw_provider_rows,
        ai_gw_provider_model,
        pending_ai_gw_channel_toggle,
        ai_gw_filter_image_generation,
        ai_gw_enable_logging,
        ai_gw_delete_button,
        ai_gw_new_button,
        ai_gw_edit_button,
        request_log_list,
        request_log_rows,
        request_log_model,
        request_log_disabled_hint,
    };

    let daemon_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    let dashboard_refresh = DashboardRefresh::new(gui_tx.clone());
    show_dashboard_starting(&handles);

    let codex_action_in_flight = Arc::new(AtomicBool::new(false));
    codex_tab::bind_actions(
        &api,
        &frame,
        &handles.codex_tab,
        &dashboard_refresh,
        &gui_tx,
        &codex_action_in_flight,
    );

    bind_service_connection_settings(&frame, &handles);

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        change_bot_button.on_click(move |_| {
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                return;
            }
            show_feishu_onboard_dialog(&frame, handles.text, api.clone());
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    let im_action_in_flight = Arc::new(AtomicBool::new(false));

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let gui_tx = gui_tx.clone();
        let im_action_in_flight = im_action_in_flight.clone();
        delete_im_account_button.on_click(move |_| {
            if im_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                im_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            let Some(account) = selected_im_account(&handles, &dashboard_refresh) else {
                im_action_in_flight.store(false, Ordering::SeqCst);
                show_error(&frame, handles.text.select_bot_first());
                return;
            };
            let name = account
                .display_name
                .clone()
                .unwrap_or_else(|| account.account_id.clone());
            if !confirm_delete_im_account(&frame, handles.text, &name) {
                im_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            handles
                .delete_im_account_button
                .set_label(handles.text.delete_in_progress());
            handles.delete_im_account_button.enable(false);
            let request = DeleteImAccountRequest {
                platform: account.platform,
                account_id: account.account_id,
            };
            let thread_api = api.clone();
            let gui_tx = gui_tx.clone();
            let im_action_in_flight = im_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api.delete_im_account(&request);
                im_action_in_flight.store(false, Ordering::SeqCst);
                let _ = gui_tx.send(GuiMessage::ImAction(ImActionResult::AccountDelete(outcome)));
                wxdragon::wake_up_idle();
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let gui_tx = gui_tx.clone();
        let im_action_in_flight = im_action_in_flight.clone();
        save_telegram_button.on_click(move |_| {
            if im_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                im_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }

            let Some(token) = prompt_telegram_bot_token(&frame, handles.text) else {
                im_action_in_flight.store(false, Ordering::SeqCst);
                return;
            };

            handles
                .save_telegram_button
                .set_label(handles.text.add_in_progress());
            handles.save_telegram_button.enable(false);
            frame.refresh(true, None);
            frame.update();

            let request = ConfigureTelegramBotRequest {
                bot_token: Some(token),
            };
            let thread_api = api.clone();
            let gui_tx = gui_tx.clone();
            let im_action_in_flight = im_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api.configure_telegram_bot(&request);
                im_action_in_flight.store(false, Ordering::SeqCst);
                let _ = gui_tx.send(GuiMessage::ImAction(ImActionResult::TelegramConfigure(
                    outcome,
                )));
                wxdragon::wake_up_idle();
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        connect_wechat_button.on_click(move |_| {
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                return;
            }
            show_wechat_onboard_dialog(&frame, handles.text, api.clone());
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    // --- AI Gateway event handlers ---
    let ai_gw_action_in_flight = Arc::new(AtomicBool::new(false));

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let gui_tx = gui_tx.clone();
        let input = handles.ai_gw_filter_image_generation;
        input.on_toggled(move |_| {
            let enabled = input.get_value();
            let worker_api = api.clone();
            let gui_tx = gui_tx.clone();
            thread::spawn(move || {
                let outcome = set_filter_image_generation_tool(&worker_api, enabled);
                let _ = gui_tx.send(GuiMessage::AiGwAction(
                    AiGwActionResult::FilterImageGeneration(outcome),
                ));
                wxdragon::wake_up_idle();
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let gui_tx = gui_tx.clone();
        let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
        ai_gw_new_button.on_click(move |_| {
            if ai_gw_action_in_flight.load(Ordering::SeqCst) {
                return;
            }
            if let Some(provider) = show_ai_gw_channel_dialog(&frame, handles.text, None) {
                start_ai_gw_provider_save(
                    &api,
                    &dashboard_refresh,
                    &handles,
                    &gui_tx,
                    &ai_gw_action_in_flight,
                    provider,
                );
            }
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let gui_tx = gui_tx.clone();
        let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
        ai_gw_edit_button.on_click(move |_| {
            if ai_gw_action_in_flight.load(Ordering::SeqCst) {
                return;
            }
            let Some(provider) = selected_ai_gw_provider(&handles, &dashboard_refresh) else {
                show_error(&frame, handles.text.ai_gw_select_channel());
                return;
            };
            if let Some(provider) = show_ai_gw_channel_dialog(&frame, handles.text, Some(&provider))
            {
                start_ai_gw_provider_save(
                    &api,
                    &dashboard_refresh,
                    &handles,
                    &gui_tx,
                    &ai_gw_action_in_flight,
                    provider,
                );
            }
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let gui_tx = gui_tx.clone();
        let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
        ai_gw_delete_button.on_click(move |_| {
            if ai_gw_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            let Some(provider) = selected_ai_gw_provider(&handles, &dashboard_refresh) else {
                ai_gw_action_in_flight.store(false, Ordering::SeqCst);
                show_error(&frame, handles.text.ai_gw_select_channel());
                return;
            };
            let name = provider.name;
            handles
                .ai_gw_delete_button
                .set_label(handles.text.ai_gw_deleting());
            set_ai_gw_actions_enabled(&handles, false);

            let worker_api = api.clone();
            let gui_tx = gui_tx.clone();
            let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = delete_ai_gw_provider(&worker_api, &name);
                let _ = gui_tx.send(GuiMessage::AiGwAction(AiGwActionResult::Delete(outcome)));
                wxdragon::wake_up_idle();
                ai_gw_action_in_flight.store(false, Ordering::SeqCst);
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let gui_tx = gui_tx.clone();
        let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
        ai_gw_provider_list.on_item_activated(move |_| {
            if ai_gw_action_in_flight.load(Ordering::SeqCst) {
                return;
            }
            let Some(provider) = selected_ai_gw_provider(&handles, &dashboard_refresh) else {
                show_error(&frame, handles.text.ai_gw_select_channel());
                return;
            };
            if let Some(provider) = show_ai_gw_channel_dialog(&frame, handles.text, Some(&provider))
            {
                start_ai_gw_provider_save(
                    &api,
                    &dashboard_refresh,
                    &handles,
                    &gui_tx,
                    &ai_gw_action_in_flight,
                    provider,
                );
            }
        });
    }

    // AI Gateway checkbox event handlers
    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let gui_tx = gui_tx.clone();
        ai_gw_filter_image_generation.on_toggled(move |event| {
            let enabled = event.is_checked();
            let worker_api = api.clone();
            let gui_tx = gui_tx.clone();
            thread::spawn(move || {
                let outcome = set_filter_image_generation_tool(&worker_api, enabled);
                let _ = gui_tx.send(GuiMessage::AiGwAction(
                    AiGwActionResult::FilterImageGeneration(outcome),
                ));
                wxdragon::wake_up_idle();
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }
    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let gui_tx = gui_tx.clone();
        ai_gw_enable_logging.on_toggled(move |event| {
            let enabled = event.is_checked();
            let worker_api = api.clone();
            let gui_tx = gui_tx.clone();
            thread::spawn(move || {
                let outcome = set_request_logging_enabled(&worker_api, enabled);
                let _ = gui_tx.send(GuiMessage::AiGwAction(AiGwActionResult::RequestLogging(
                    outcome,
                )));
                wxdragon::wake_up_idle();
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    let request_log_result: RequestLogResultStore = Arc::new(Mutex::new(None));
    let request_log_in_flight = Arc::new(AtomicBool::new(false));
    let request_log_detail_result: RequestLogDetailResultStore = Arc::new(Mutex::new(None));
    let request_log_detail_in_flight = Arc::new(AtomicBool::new(false));
    let request_log_clear_result: RequestLogClearResultStore = Arc::new(Mutex::new(None));
    let request_log_clear_in_flight = Arc::new(AtomicBool::new(false));
    let request_logs_active = Rc::new(Cell::new(notebook.selection() == REQUEST_LOG_TAB_INDEX));
    let request_log_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    {
        let api = api.clone();
        let request_log_result = request_log_result.clone();
        let request_log_in_flight = request_log_in_flight.clone();
        let request_logs_active = request_logs_active.clone();
        let request_log_timer_store = request_log_timer_store.clone();
        notebook.on_page_changed(move |event| {
            let active = event.get_selection().unwrap_or(-1) == REQUEST_LOG_TAB_INDEX;
            request_logs_active.set(active);
            if active {
                force_request_log_refresh(&api, &request_log_result, &request_log_in_flight);
                start_request_log_timer(&request_log_timer_store);
            } else {
                stop_request_log_timer(&request_log_timer_store);
            }
        });
    }
    {
        let api = api.clone();
        let handles = handles.clone();
        let request_log_detail_result = request_log_detail_result.clone();
        let request_log_detail_in_flight = request_log_detail_in_flight.clone();
        request_log_list.on_item_activated(move |event| {
            let Some(row) = event.get_row().map(|row| row as usize) else {
                return;
            };
            start_request_log_detail_load(
                &api,
                &handles,
                row,
                &request_log_detail_result,
                &request_log_detail_in_flight,
            );
        });
    }
    {
        let api = api.clone();
        let frame = frame;
        let text = handles.text;
        let request_log_clear_result = request_log_clear_result.clone();
        let request_log_clear_in_flight = request_log_clear_in_flight.clone();
        request_log_clear_old_button.on_click(move |_| {
            if request_log_clear_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !confirm_clear_old_request_logs(&frame, text) {
                request_log_clear_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            request_log_clear_old_button.enable(false);
            request_log_clear_all_button.enable(false);
            let thread_api = api.clone();
            let request_log_clear_result = request_log_clear_result.clone();
            let request_log_clear_in_flight = request_log_clear_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api
                    .ai_gateway_clear_old_request_logs()
                    .map(|response| response.deleted);
                if let Ok(mut slot) = request_log_clear_result.lock() {
                    slot.replace(outcome);
                }
                request_log_clear_in_flight.store(false, Ordering::SeqCst);
                wxdragon::wake_up_idle();
            });
        });
    }
    {
        let api = api.clone();
        let frame = frame;
        let text = handles.text;
        let request_log_clear_result = request_log_clear_result.clone();
        let request_log_clear_in_flight = request_log_clear_in_flight.clone();
        request_log_clear_all_button.on_click(move |_| {
            if request_log_clear_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !confirm_clear_all_request_logs(&frame, text) {
                request_log_clear_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            request_log_clear_old_button.enable(false);
            request_log_clear_all_button.enable(false);
            let thread_api = api.clone();
            let request_log_clear_result = request_log_clear_result.clone();
            let request_log_clear_in_flight = request_log_clear_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api
                    .ai_gateway_clear_all_request_logs()
                    .map(|response| response.deleted);
                if let Ok(mut slot) = request_log_clear_result.lock() {
                    slot.replace(outcome);
                }
                request_log_clear_in_flight.store(false, Ordering::SeqCst);
                wxdragon::wake_up_idle();
            });
        });
    }
    if request_logs_active.get() {
        force_request_log_refresh(&api, &request_log_result, &request_log_in_flight);
    }

    let request_log_timer = Timer::new(&frame);
    {
        let api = api.clone();
        let handles = handles.clone();
        let request_log_result = request_log_result.clone();
        let request_log_in_flight = request_log_in_flight.clone();
        let request_log_detail_result = request_log_detail_result.clone();
        let request_log_clear_result = request_log_clear_result.clone();
        let request_logs_active = request_logs_active.clone();
        request_log_timer.on_tick(move |_| {
            apply_pending_request_logs(&handles, &request_log_result);
            apply_pending_request_log_detail(&frame, &handles, &request_log_detail_result);
            if apply_pending_request_log_clear(
                &frame,
                handles.text,
                &request_log_clear_old_button,
                &request_log_clear_all_button,
                &request_log_clear_result,
            ) && request_logs_active.get()
            {
                force_request_log_refresh(&api, &request_log_result, &request_log_in_flight);
            }
            if request_logs_active.get() {
                schedule_request_log_refresh(&api, &request_log_result, &request_log_in_flight);
            }
        });
    }
    request_log_timer.start(REQUEST_LOG_REFRESH_INTERVAL_MS, false);
    request_log_timer_store
        .borrow_mut()
        .replace(request_log_timer);
    gui_timers.track(&request_log_timer_store);
    if request_logs_active.get() {
        start_request_log_timer(&request_log_timer_store);
    }

    let timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let timer = Timer::new(&frame);
    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        timer.on_tick(move |_| {
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }
    timer.start(DASHBOARD_REFRESH_INTERVAL_MS, false);
    timer_store.borrow_mut().replace(timer);
    gui_timers.track(&timer_store);

    // Event-driven message pump: replaces the old 100ms polling timers. Drains
    // the channel of results posted by background threads and processes any
    // pending data-view toggles. Idle events only fire after `wake_up_idle()`
    // (called by the senders) or other UI activity, and `request_more(true)` is
    // only used while messages remain, so the app sleeps at ~0% CPU when idle.
    {
        let api = api.clone();
        let handles = handles.clone();
        let frame = frame;
        let dashboard_refresh = dashboard_refresh.clone();
        let gui_tx = gui_tx.clone();
        let im_action_in_flight = im_action_in_flight.clone();
        let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
        let mut gui_rx = gui_rx;
        let request_log_result = request_log_result.clone();
        let request_log_detail_result = request_log_detail_result.clone();
        let request_log_in_flight = request_log_in_flight.clone();
        let request_log_clear_result = request_log_clear_result.clone();
        let request_logs_active = request_logs_active.clone();
        let request_log_clear_old_button = request_log_clear_old_button;
        let request_log_clear_all_button = request_log_clear_all_button;
        frame.on_idle(move |event| {
            // Kick off any toggles queued from the data views.
            process_pending_im_toggle(
                &api,
                &handles,
                &frame,
                &dashboard_refresh,
                &gui_tx,
                &im_action_in_flight,
            );
            process_pending_ai_gw_toggle(
                &api,
                &handles,
                &frame,
                &dashboard_refresh,
                &gui_tx,
                &ai_gw_action_in_flight,
            );

            // Render request-log list/detail results as soon as their background
            // loads finish. These stores are filled by worker threads that call
            // `wake_up_idle()` on completion, so applying them here restores the
            // instant open behavior instead of waiting for the 1.5s fallback
            // timer. Both applies cheaply no-op when their slot is empty.
            apply_pending_request_logs(&handles, &request_log_result);
            apply_pending_request_log_detail(&frame, &handles, &request_log_detail_result);
            if apply_pending_request_log_clear(
                &frame,
                handles.text,
                &request_log_clear_old_button,
                &request_log_clear_all_button,
                &request_log_clear_result,
            ) && request_logs_active.get()
            {
                force_request_log_refresh(&api, &request_log_result, &request_log_in_flight);
            }

            // Drain a bounded batch of background results to keep the GUI snappy.
            let mut processed = 0;
            let mut needs_dashboard_refresh = false;
            while processed < 20 {
                match gui_rx.try_recv() {
                    Ok(message) => {
                        processed += 1;
                        match message {
                            GuiMessage::CodexAction(result) => {
                                codex_tab::apply_pending_action(
                                    &api,
                                    &handles.codex_tab,
                                    handles.text,
                                    &frame,
                                    &dashboard_refresh,
                                    result,
                                );
                            }
                            GuiMessage::ImAction(result) => {
                                apply_pending_im_action(
                                    &api,
                                    &handles,
                                    &frame,
                                    &dashboard_refresh,
                                    result,
                                );
                            }
                            GuiMessage::AiGwAction(result) => {
                                if matches!(result, AiGwActionResult::ChannelToggle { .. }) {
                                    needs_dashboard_refresh = true;
                                }
                                apply_pending_ai_gw_action(&handles, &frame, result);
                            }
                            GuiMessage::DashboardUpdate => {
                                apply_pending_dashboard(&handles, &dashboard_refresh, &api, &frame);
                            }
                        }
                    }
                    Err(tokio_mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio_mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
            if needs_dashboard_refresh {
                schedule_dashboard_refresh(&api, &dashboard_refresh);
            }

            // Only keep spinning idle events while we still had a full batch to
            // drain; otherwise let the loop sleep until the next wake-up.
            if let WindowEventData::Idle(idle) = event {
                idle.request_more(processed >= 20);
            }
        });
    }

    start_daemon_for_gui_async(
        &api,
        &handles,
        &frame,
        &daemon_child,
        &dashboard_refresh,
        &gui_timers,
    );

    {
        let api = api.clone();
        let daemon_child = daemon_child.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let gui_timers = gui_timers.clone();
        let frame = frame;
        let app = app;
        let tray_controller = Rc::new(tray_controller);
        let quitting = quitting.clone();
        frame.on_close(move |event| {
            let _single_instance_guard = &single_instance_guard;
            if !quitting.load(Ordering::SeqCst) {
                if let WindowEventData::General(raw_event) = &event
                    && raw_event.can_veto()
                {
                    raw_event.veto();
                }
                tray::hide_main_window(&frame, handles.text);
                return;
            }
            dashboard_refresh.closing.store(true, Ordering::SeqCst);
            gui_timers.stop_all();
            tray_controller.remove_icon();
            stop_pending_startup_daemon(&dashboard_refresh);
            stop_daemon_on_exit(&api, &daemon_child);
            frame.destroy();
            app.exit_main_loop();
        });
    }

    frame.centre();
    // On displays too small to show the preferred size, open maximized so the
    // whole first screen (status overview + active tab actions) is visible
    // without manual resizing.
    if let Some((work_w, work_h)) = screen_work_area_size()
        && (work_w < PREFERRED_FRAME_WIDTH || work_h < PREFERRED_FRAME_HEIGHT)
    {
        frame.maximize(true);
    }
    frame.show(true);
    update::check_for_updates_silent_async(
        &frame,
        &gui_timers,
        text,
        &update_check_in_flight,
        &quitting,
    );
}

/// Enable `wxFULL_REPAINT_ON_RESIZE` (0x00010000) on a window so its whole
/// client area is repainted on resize instead of only the newly exposed strip.
/// Without it, wxMSW leaves stale pixels behind that smear into vertical
/// "ghost" streaks while the window is being resized.
fn enable_full_repaint_on_resize<W: WxWidget>(window: &W) {
    const WXD_FULL_REPAINT_ON_RESIZE: i64 = 0x0001_0000;
    window.set_style_raw(window.get_style_raw() | WXD_FULL_REPAINT_ON_RESIZE);
}

/// Preferred first-launch window size on a roomy display.
const PREFERRED_FRAME_WIDTH: i32 = 1180;
const PREFERRED_FRAME_HEIGHT: i32 = 760;
/// Smallest window that still keeps the status overview and the primary tab
/// action buttons reachable without hunting through scrollbars.
const MIN_FRAME_WIDTH: i32 = 900;
const MIN_FRAME_HEIGHT: i32 = 560;

/// Compute the initial frame size, clamped to the current screen work area so
/// the window never opens taller or wider than the usable desktop. On small or
/// scaled laptops this keeps the bottom action row (e.g. "save models") inside
/// the first screen instead of below a scrollbar the user has to discover.
fn initial_frame_size() -> Size {
    let preferred = Size::new(PREFERRED_FRAME_WIDTH, PREFERRED_FRAME_HEIGHT);
    match screen_work_area_size() {
        Some((work_w, work_h)) => {
            // Leave a small margin so the title bar and window borders stay on
            // screen even when the reported work area is flush with the frame.
            let max_w = (work_w - 32).max(MIN_FRAME_WIDTH);
            let max_h = (work_h - 48).max(MIN_FRAME_HEIGHT);
            Size::new(preferred.width.min(max_w), preferred.height.min(max_h))
        }
        None => preferred,
    }
}

/// Never let `set_min_size` demand more than the screen can show; otherwise the
/// floor itself would force off-screen content on very small displays.
fn min_frame_size(frame_size: Size) -> Size {
    Size::new(
        MIN_FRAME_WIDTH.min(frame_size.width),
        MIN_FRAME_HEIGHT.min(frame_size.height),
    )
}

/// Return the usable desktop work area (excluding taskbar/dock) in pixels.
#[cfg(target_os = "windows")]
fn screen_work_area_size() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `SystemParametersInfoW` fills the RECT we own; we only read it
    // after checking the call succeeded.
    let ok =
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut rect as *mut RECT).cast(), 0) };
    if ok == 0 {
        return None;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(not(target_os = "windows"))]
fn screen_work_area_size() -> Option<(i32, i32)> {
    None
}

fn create_main_tab_icons(notebook: &Notebook) -> [Option<i32>; 4] {
    // Use 24x24 for better quality on high-DPI displays
    let size = 24;
    let image_list = ImageList::new(size, size, true, 4);
    let image_ids = [
        image_list.add_bitmap(&status_icon_bitmap(StatusIconKind::Codex, size as usize)),
        image_list.add_bitmap(&lucide_icon_bitmap(LucideIconKind::Router, size as usize)),
        image_list.add_bitmap(&lucide_icon_bitmap(
            LucideIconKind::MessagesSquare,
            size as usize,
        )),
        image_list.add_bitmap(&lucide_icon_bitmap(
            LucideIconKind::ScrollText,
            size as usize,
        )),
    ];
    let icons = image_ids.map(|id| (id >= 0).then_some(id));
    if icons.iter().any(Option::is_some) {
        notebook.set_image_list(image_list);
    }
    icons
}

fn default_base_url() -> String {
    std::env::var("CODEX_REMOTE_GUI_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| local_service_base_url(load_local_connection_mode()))
}

fn local_service_base_url(mode: LocalConnectionMode) -> String {
    match mode {
        LocalConnectionMode::Standard => DEFAULT_BASE_URL.to_string(),
        LocalConnectionMode::VpnCompatible => "http://localhost:3847".to_string(),
    }
}

fn load_local_connection_mode() -> LocalConnectionMode {
    daemon_config_path()
        .and_then(|path| AppConfig::load_or_default(&path).ok())
        .map(|config| config.local_connection_mode)
        .unwrap_or_default()
}

fn save_local_connection_mode(mode: LocalConnectionMode) -> Result<(), String> {
    let path = daemon_config_path().unwrap_or_else(app_support_config_path);
    let mut config = AppConfig::load_or_default(&path).map_err(|err| err.to_string())?;
    config.local_connection_mode = mode;
    config.save(&path).map_err(|err| err.to_string())
}

fn load_gui_locale() -> GuiLocale {
    daemon_config_path()
        .and_then(|path| AppConfig::load_or_default(&path).ok())
        .and_then(|config| config.language)
        .and_then(|language| GuiLocale::from_code(&language))
        .unwrap_or_default()
}

fn save_gui_locale(locale: GuiLocale) -> Result<(), String> {
    let path = daemon_config_path().unwrap_or_else(app_support_config_path);
    let mut config = AppConfig::load_or_default(&path).map_err(|err| err.to_string())?;
    config.language = Some(locale.code().to_string());
    config.save(&path).map_err(|err| err.to_string())
}

fn load_gui_theme() -> ThemeMode {
    daemon_config_path()
        .and_then(|path| AppConfig::load_or_default(&path).ok())
        .and_then(|config| config.theme)
        .and_then(|theme| ThemeMode::from_code(&theme))
        .unwrap_or_default()
}

fn save_gui_theme(mode: ThemeMode) -> Result<(), String> {
    let path = daemon_config_path().unwrap_or_else(app_support_config_path);
    let mut config = AppConfig::load_or_default(&path).map_err(|err| err.to_string())?;
    config.theme = Some(mode.code().to_string());
    config.save(&path).map_err(|err| err.to_string())
}

fn install_system_menu(
    frame: &Frame,
    gui_timers: &GuiTimers,
    text: GuiText,
    update_check_in_flight: Arc<AtomicBool>,
    quitting: Rc<AtomicBool>,
) {
    let file_menu = Menu::builder()
        .append_item(
            ID_MENU_CLOSE_WINDOW,
            text.close_window(),
            text.close_window_help(),
        )
        .append_item(ID_MENU_MINIMIZE, text.minimize(), text.minimize_help())
        .append_separator()
        .append_item(ID_MENU_QUIT, text.quit(), text.quit_help())
        .build();
    let language_menu = Menu::builder()
        .append_radio_item(
            ID_MENU_LANGUAGE_ZH_CN,
            text.language_zh_cn(),
            text.language_restart_message(),
        )
        .append_radio_item(
            ID_MENU_LANGUAGE_EN_US,
            text.language_en_us(),
            text.language_restart_message(),
        )
        .build();
    language_menu.check_item(ID_MENU_LANGUAGE_ZH_CN, text.locale == GuiLocale::ZhCn);
    language_menu.check_item(ID_MENU_LANGUAGE_EN_US, text.locale == GuiLocale::EnUs);
    let theme_mode = load_gui_theme();
    let theme_menu = Menu::builder()
        .append_radio_item(
            ID_MENU_THEME_SYSTEM,
            text.theme_system(),
            text.theme_restart_message(),
        )
        .append_radio_item(
            ID_MENU_THEME_LIGHT,
            text.theme_light(),
            text.theme_restart_message(),
        )
        .append_radio_item(
            ID_MENU_THEME_DARK,
            text.theme_dark(),
            text.theme_restart_message(),
        )
        .build();
    theme_menu.check_item(ID_MENU_THEME_SYSTEM, theme_mode == ThemeMode::System);
    theme_menu.check_item(ID_MENU_THEME_LIGHT, theme_mode == ThemeMode::Light);
    theme_menu.check_item(ID_MENU_THEME_DARK, theme_mode == ThemeMode::Dark);
    let help_menu = Menu::builder()
        .append_item(
            ID_MENU_CHECK_UPDATE,
            text.check_updates(),
            text.check_updates_help(),
        )
        .append_separator()
        .append_item(ID_ABOUT, text.about(), "About CodexHub")
        .build();
    let menu_bar = MenuBar::builder()
        .append(file_menu, text.file_menu())
        .append(language_menu, text.language_menu())
        .append(theme_menu, text.theme_menu())
        .append(help_menu, text.help_menu())
        .build();
    frame.set_menu_bar(menu_bar);

    let frame = *frame;
    let gui_timers = gui_timers.clone();
    frame.on_menu_selected(move |event| match event.get_id() {
        ID_MENU_CLOSE_WINDOW => tray::hide_main_window(&frame, text),
        ID_MENU_QUIT | ID_EXIT => tray::request_app_quit(&frame, &quitting),
        ID_MENU_MINIMIZE => frame.iconize(true),
        ID_MENU_CHECK_UPDATE => {
            update::check_for_updates_async(
                &frame,
                &gui_timers,
                text,
                &update_check_in_flight,
                &quitting,
            );
        }
        ID_MENU_LANGUAGE_ZH_CN => {
            handle_language_selected(&frame, text, GuiLocale::ZhCn);
        }
        ID_MENU_LANGUAGE_EN_US => {
            handle_language_selected(&frame, text, GuiLocale::EnUs);
        }
        ID_MENU_THEME_SYSTEM => handle_theme_selected(&frame, text, ThemeMode::System),
        ID_MENU_THEME_LIGHT => handle_theme_selected(&frame, text, ThemeMode::Light),
        ID_MENU_THEME_DARK => handle_theme_selected(&frame, text, ThemeMode::Dark),
        ID_ABOUT => show_about_dialog(&frame),
        _ => event.skip(true),
    });
}

fn ai_gw_service_option(
    parent: &Panel,
    parent_sizer: &BoxSizer,
    label: &str,
    logo: Option<ProviderLogoKind>,
    first_in_group: bool,
) -> RadioButton {
    let row_panel = Panel::builder(parent).build();
    row_panel.set_background_color(theme::theme().bg_card_alt);
    row_panel.set_min_size(Size::new(0, 46));

    let row = BoxSizer::builder(Orientation::Horizontal).build();
    row.add_spacer(12);
    match logo {
        Some(kind) => {
            let icon = StaticBitmap::builder(&row_panel)
                .with_bitmap(Some(provider_logo_bitmap(kind, 24)))
                .with_size(Size::new(24, 24))
                .build();
            icon.set_min_size(Size::new(24, 24));
            row.add(
                &icon,
                0,
                SizerFlag::AlignCenterVertical | SizerFlag::Right,
                10,
            );
        }
        None => {
            row.add_spacer(34);
        }
    }

    let builder = RadioButton::builder(&row_panel).with_label(label);
    let radio = if first_in_group {
        builder.first_in_group().build()
    } else {
        builder.build()
    };
    radio.set_background_color(theme::theme().bg_card_alt);
    radio.set_foreground_color(theme::theme().ink_primary);
    radio.set_tooltip(label);
    row.add(&radio, 1, SizerFlag::AlignCenterVertical, 0);
    row.add_spacer(12);
    row_panel.set_sizer(row, true);
    parent_sizer.add(&row_panel, 0, SizerFlag::Expand | SizerFlag::Bottom, 8);
    radio
}

fn selected_ai_gw_provider(
    handles: &UiHandles,
    dashboard_refresh: &DashboardRefresh,
) -> Option<ProviderConfig> {
    let index = handles.ai_gw_provider_list.get_selected_row()?;
    let name = handles
        .ai_gw_provider_rows
        .borrow()
        .get(index)
        .map(|row| row.name.clone())?;
    let snapshot = cached_dashboard_snapshot(dashboard_refresh)?;
    snapshot
        .ai_gateway
        .as_ref()
        .and_then(|config| {
            config
                .providers
                .iter()
                .find(|provider| provider.name == name)
        })
        .cloned()
}

fn start_ai_gw_provider_save(
    api: &ApiClient,
    dashboard_refresh: &DashboardRefresh,
    handles: &UiHandles,
    gui_tx: &tokio_mpsc::UnboundedSender<GuiMessage>,
    in_flight: &Arc<AtomicBool>,
    provider: ProviderConfig,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    set_ai_gw_actions_enabled(handles, false);

    let worker_api = api.clone();
    let gui_tx = gui_tx.clone();
    let in_flight = in_flight.clone();
    thread::spawn(move || {
        let outcome = save_ai_gw_provider(&worker_api, provider);
        in_flight.store(false, Ordering::SeqCst);
        let _ = gui_tx.send(GuiMessage::AiGwAction(AiGwActionResult::Save(outcome)));
        wxdragon::wake_up_idle();
    });
    schedule_dashboard_refresh(api, dashboard_refresh);
}

fn show_ai_gw_channel_dialog(
    parent: &Frame,
    text: GuiText,
    initial: Option<&ProviderConfig>,
) -> Option<ProviderConfig> {
    let dialog = Dialog::builder(parent, text.ai_gw_channel_editor())
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(1120, 760)
        .build();
    dialog.set_min_size(Size::new(920, 640));
    dialog.set_background_color(theme::theme().bg_card_alt);

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(theme::theme().bg_card_alt);
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let help = StaticText::builder(&panel)
        .with_label(text.ai_gw_channel_editor_help())
        .build();
    help.set_foreground_color(theme::theme().ink_muted);
    root.add(
        &help,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let workspace = BoxSizer::builder(Orientation::Horizontal).build();

    let service_panel = Panel::builder(&panel).build();
    service_panel.set_background_color(theme::theme().bg_card);
    service_panel.set_min_size(Size::new(300, 500));
    let service_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let service_title = StaticText::builder(&service_panel)
        .with_label(text.ai_gw_provider_service())
        .build();
    service_title.set_foreground_color(theme::theme().ink_primary);
    service_title.set_font(&theme::font(theme::TextRole::Title));
    service_sizer.add(
        &service_title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        14,
    );
    let radio_openai = ai_gw_service_option(
        &service_panel,
        &service_sizer,
        text.ai_gw_service_openai(),
        Some(ProviderLogoKind::OpenAi),
        true,
    );
    let radio_deepseek = ai_gw_service_option(
        &service_panel,
        &service_sizer,
        text.ai_gw_service_deepseek(),
        Some(ProviderLogoKind::DeepSeek),
        false,
    );
    let radio_anthropic = ai_gw_service_option(
        &service_panel,
        &service_sizer,
        text.ai_gw_service_anthropic(),
        Some(ProviderLogoKind::Anthropic),
        false,
    );
    let radio_glm = ai_gw_service_option(
        &service_panel,
        &service_sizer,
        text.ai_gw_service_glm(),
        Some(ProviderLogoKind::Zhipu),
        false,
    );
    service_sizer.add_stretch_spacer(1);
    service_panel.set_sizer(service_sizer, true);
    workspace.add(&service_panel, 0, SizerFlag::Expand | SizerFlag::Right, 14);

    let form_panel = Panel::builder(&panel).build();
    form_panel.set_background_color(theme::theme().bg_card);
    form_panel.set_min_size(Size::new(620, 500));
    let form_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let form_title = StaticText::builder(&form_panel)
        .with_label(text.ai_gw_channel_settings())
        .build();
    form_title.set_foreground_color(theme::theme().ink_primary);
    form_title.set_font(&theme::font(theme::TextRole::Title));
    form_sizer.add(
        &form_title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let grid = FlexGridSizer::builder(0, 2)
        .with_vgap(12)
        .with_hgap(14)
        .build();
    grid.add_growable_col(1, 1);

    let type_input = text_field_row(
        &form_panel,
        &grid,
        text.ai_gw_api_protocol(),
        text.provider_type_openai_responses(),
    );
    type_input.set_editable(false);
    let name_input = text_field_row(&form_panel, &grid, text.ai_gw_provider_name(), "");
    let base_url_input = text_field_row(&form_panel, &grid, text.ai_gw_col_base_url(), "");
    let models_url_input = text_field_row(&form_panel, &grid, text.ai_gw_models_url(), "");
    models_url_input.set_tooltip(text.ai_gw_models_url_help());
    let models_url_help_spacer = StaticText::builder(&form_panel).with_label("").build();
    grid.add(&models_url_help_spacer, 0, SizerFlag::Right, 0);
    let models_url_help = StaticText::builder(&form_panel)
        .with_label(text.ai_gw_models_url_help())
        .build();
    models_url_help.set_foreground_color(theme::theme().ink_muted);
    models_url_help.wrap(540);
    grid.add(&models_url_help, 0, SizerFlag::Expand, 0);
    let key_input = text_field_row(&form_panel, &grid, text.ai_gw_col_api_key(), "");
    let weight_input = text_field_row(&form_panel, &grid, text.ai_gw_weight(), "100");

    form_sizer.add_sizer(
        &grid,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let model_header = BoxSizer::builder(Orientation::Horizontal).build();
    let model_title = StaticText::builder(&form_panel)
        .with_label(text.ai_gw_models())
        .build();
    model_title.set_foreground_color(theme::theme().ink_secondary);
    let fetch_models_button = Button::builder(&form_panel)
        .with_label(text.ai_gw_fetch_models())
        .build();
    let add_model_button = Button::builder(&form_panel)
        .with_label(text.ai_gw_add_model())
        .build();
    let delete_model_button = Button::builder(&form_panel)
        .with_label(text.ai_gw_delete_model())
        .build();
    model_header.add(&model_title, 1, SizerFlag::AlignCenterVertical, 0);
    model_header.add(
        &fetch_models_button,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        8,
    );
    model_header.add(
        &add_model_button,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        8,
    );
    model_header.add(&delete_model_button, 0, SizerFlag::AlignCenterVertical, 0);
    form_sizer.add_sizer(
        &model_header,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let model_mapping_rows: ModelMappingRows = Rc::new(RefCell::new(Vec::new()));
    let model_mapping_model: ModelMappingModel =
        Rc::new(RefCell::new(CustomDataViewVirtualListModel::new(
            0,
            model_mapping_rows.clone(),
            model_mapping_cell,
            None::<fn(&ModelMappingRows, usize, usize, &Variant) -> bool>,
            Some(|_: &ModelMappingRows, row, _| table_cell_attr(row)),
            None::<fn(&ModelMappingRows, usize, usize) -> bool>,
        )));
    let models_list = DataViewCtrl::builder(&form_panel)
        .with_style(dataview_table_style(false))
        .with_size(Size::new(-1, 180))
        .build();
    apply_dataview_theme(&models_list);
    models_list.append_text_column(
        text.ai_gw_upstream_model(),
        0,
        310,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    models_list.append_text_column(
        text.ai_gw_codex_model_aliases(),
        1,
        360,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    models_list.associate_model(&*model_mapping_model.borrow());
    models_list.set_min_size(Size::new(420, 150));
    form_sizer.add(
        &models_list,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );
    form_panel.set_sizer(form_sizer, true);
    workspace.add(&form_panel, 1, SizerFlag::Expand, 0);

    root.add_sizer(
        &workspace,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let cancel_button = Button::builder(&panel)
        .with_id(ID_CANCEL)
        .with_label(text.cancel())
        .build();
    let save_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label(if initial.is_some() {
            text.ai_gw_save_channel()
        } else {
            text.ai_gw_create_channel()
        })
        .build();
    save_button.set_default();
    buttons.add_stretch_spacer(1);
    buttons.add(&cancel_button, 0, SizerFlag::Right, 8);
    buttons.add(&save_button, 0, SizerFlag::Right, 0);
    root.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        18,
    );

    panel.set_sizer(root, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);

    apply_ai_gw_dialog_template(
        text,
        initial,
        &radio_openai,
        &radio_deepseek,
        &radio_anthropic,
        &radio_glm,
        &type_input,
        &name_input,
        &base_url_input,
        &models_url_input,
        &models_list,
        &model_mapping_rows,
        &model_mapping_model,
        &weight_input,
    );
    let current_ai_gw_provider_template = Rc::new(RefCell::new(normalize_provider_models_url(
        initial
            .cloned()
            .unwrap_or_else(|| default_ai_gw_service_provider(ProviderType::OpenAiResponses)),
    )));
    if let Some(provider) = initial {
        key_input.change_value(&provider.api_key);
    }

    let service_template_applying = Rc::new(RefCell::new(false));
    if let Some(provider) = initial {
        bind_locked_ai_gw_service_selection(
            text,
            provider.provider_type.clone(),
            provider.compatibility.as_deref().map(ToOwned::to_owned),
            &radio_openai,
            &radio_deepseek,
            &radio_anthropic,
            &radio_glm,
            &type_input,
            service_template_applying.clone(),
        );
    }
    if initial.is_none() {
        let type_input = type_input;
        let name_input = name_input;
        let base_url_input = base_url_input;
        let models_url_input = models_url_input;
        let key_input = key_input;
        let models_list = models_list;
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        let weight_input = weight_input;
        let service_template_applying = service_template_applying.clone();
        let current_ai_gw_provider_template = current_ai_gw_provider_template.clone();
        radio_openai.on_selected(move |_| {
            if radio_openai.get_value() && !*service_template_applying.borrow() {
                let provider = default_ai_gw_service_provider(ProviderType::OpenAiResponses);
                apply_ai_gw_service_template(
                    text,
                    provider.clone(),
                    &radio_openai,
                    &radio_deepseek,
                    &radio_anthropic,
                    &radio_glm,
                    &type_input,
                    &name_input,
                    &base_url_input,
                    &models_url_input,
                    &key_input,
                    &models_list,
                    &model_mapping_rows,
                    &model_mapping_model,
                    &weight_input,
                    &service_template_applying,
                );
                *current_ai_gw_provider_template.borrow_mut() = provider;
            }
        });
    }
    if initial.is_none() {
        let type_input = type_input;
        let name_input = name_input;
        let base_url_input = base_url_input;
        let models_url_input = models_url_input;
        let key_input = key_input;
        let models_list = models_list;
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        let weight_input = weight_input;
        let service_template_applying = service_template_applying.clone();
        let current_ai_gw_provider_template = current_ai_gw_provider_template.clone();
        radio_deepseek.on_selected(move |_| {
            if radio_deepseek.get_value() && !*service_template_applying.borrow() {
                let provider = default_ai_gw_service_provider(ProviderType::ChatCompletions);
                apply_ai_gw_service_template(
                    text,
                    provider.clone(),
                    &radio_openai,
                    &radio_deepseek,
                    &radio_anthropic,
                    &radio_glm,
                    &type_input,
                    &name_input,
                    &base_url_input,
                    &models_url_input,
                    &key_input,
                    &models_list,
                    &model_mapping_rows,
                    &model_mapping_model,
                    &weight_input,
                    &service_template_applying,
                );
                *current_ai_gw_provider_template.borrow_mut() = provider;
            }
        });
    }
    if initial.is_none() {
        let type_input = type_input;
        let name_input = name_input;
        let base_url_input = base_url_input;
        let models_url_input = models_url_input;
        let key_input = key_input;
        let models_list = models_list;
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        let weight_input = weight_input;
        let service_template_applying = service_template_applying.clone();
        let current_ai_gw_provider_template = current_ai_gw_provider_template.clone();
        radio_anthropic.on_selected(move |_| {
            if radio_anthropic.get_value() && !*service_template_applying.borrow() {
                let provider = default_ai_gw_service_provider(ProviderType::AnthropicMessages);
                apply_ai_gw_service_template(
                    text,
                    provider.clone(),
                    &radio_openai,
                    &radio_deepseek,
                    &radio_anthropic,
                    &radio_glm,
                    &type_input,
                    &name_input,
                    &base_url_input,
                    &models_url_input,
                    &key_input,
                    &models_list,
                    &model_mapping_rows,
                    &model_mapping_model,
                    &weight_input,
                    &service_template_applying,
                );
                *current_ai_gw_provider_template.borrow_mut() = provider;
            }
        });
    }
    if initial.is_none() {
        let type_input = type_input;
        let name_input = name_input;
        let base_url_input = base_url_input;
        let models_url_input = models_url_input;
        let key_input = key_input;
        let models_list = models_list;
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        let weight_input = weight_input;
        let service_template_applying = service_template_applying.clone();
        let current_ai_gw_provider_template = current_ai_gw_provider_template.clone();
        radio_glm.on_selected(move |_| {
            if radio_glm.get_value() && !*service_template_applying.borrow() {
                let provider = default_ai_gw_glm_service_provider();
                apply_ai_gw_service_template(
                    text,
                    provider.clone(),
                    &radio_openai,
                    &radio_deepseek,
                    &radio_anthropic,
                    &radio_glm,
                    &type_input,
                    &name_input,
                    &base_url_input,
                    &models_url_input,
                    &key_input,
                    &models_list,
                    &model_mapping_rows,
                    &model_mapping_model,
                    &weight_input,
                    &service_template_applying,
                );
                *current_ai_gw_provider_template.borrow_mut() = provider;
            }
        });
    }
    {
        let models_list = models_list;
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        add_model_button.on_click(move |_| {
            let input = TextEntryDialog::builder(
                &dialog,
                text.ai_gw_model_id_prompt(),
                text.ai_gw_add_model(),
            )
            .with_style(
                TextEntryDialogStyle::Ok
                    | TextEntryDialogStyle::Cancel
                    | TextEntryDialogStyle::Centre,
            )
            .build();
            let result = input.show_modal();
            if result != ID_OK {
                return;
            }
            let Some(value) = input.get_value() else {
                return;
            };
            let models = parse_model_ids(&value);
            if models.is_empty() {
                show_error(&dialog, text.ai_gw_model_id_empty());
                return;
            }
            append_model_mapping_rows(
                &models_list,
                &model_mapping_rows,
                &model_mapping_model,
                models,
                None,
            );
        });
    }
    {
        let models_list = models_list;
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        delete_model_button.on_click(move |_| {
            let Some(index) = models_list.get_selected_row() else {
                show_error(&dialog, text.ai_gw_select_model());
                return;
            };
            delete_model_mapping_row(
                &models_list,
                &model_mapping_rows,
                &model_mapping_model,
                index,
            );
        });
    }
    {
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        models_list.on_item_activated(move |event| {
            let row = event
                .get_row()
                .and_then(|row| usize::try_from(row).ok())
                .or_else(|| models_list.get_selected_row());
            let Some(row) = row else {
                return;
            };
            edit_model_mapping_row(
                &dialog,
                text,
                row,
                &model_mapping_rows,
                &model_mapping_model,
            );
        });
    }
    let fetch_models_result: FetchModelsResultStore = Arc::new(Mutex::new(None));
    let fetch_models_in_flight = Arc::new(AtomicBool::new(false));
    let fetch_models_closed = Arc::new(AtomicBool::new(false));
    {
        let base_url_input = base_url_input;
        let models_url_input = models_url_input;
        let key_input = key_input;
        let current_ai_gw_provider_template = current_ai_gw_provider_template.clone();
        let fetch_models_result = fetch_models_result.clone();
        let fetch_models_in_flight = fetch_models_in_flight.clone();
        let fetch_models_closed = fetch_models_closed.clone();
        fetch_models_button.on_click(move |_| {
            if fetch_models_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            let base_url = strip_nul(&base_url_input.get_value()).trim().to_string();
            if base_url.is_empty() {
                fetch_models_in_flight.store(false, Ordering::SeqCst);
                show_error(&dialog, text.ai_gw_base_url_empty());
                return;
            }

            fetch_models_button.enable(false);
            fetch_models_button.set_label(text.ai_gw_fetching_models());
            let api_key = strip_nul(&key_input.get_value()).trim().to_string();
            let mut template = current_ai_gw_provider_template.borrow().clone();
            template.base_url = base_url.clone();
            let models_url_value = strip_nul(&models_url_input.get_value());
            let models_url = normalize_optional_url(Some(&models_url_value));
            let fallback_models_url = known_models_url_for_provider(&template);
            let fetch_models_result = fetch_models_result.clone();
            let fetch_models_in_flight = fetch_models_in_flight.clone();
            let fetch_models_closed = fetch_models_closed.clone();
            thread::spawn(move || {
                let outcome = fetch_remote_models(
                    &base_url,
                    models_url.as_deref(),
                    fallback_models_url.as_deref(),
                    &api_key,
                    GUI_MODEL_LIST_FETCH_TIMEOUT_SECS,
                );
                if fetch_models_closed.load(Ordering::SeqCst) {
                    fetch_models_in_flight.store(false, Ordering::SeqCst);
                    return;
                }
                if let Ok(mut slot) = fetch_models_result.lock() {
                    slot.replace(outcome);
                }
                fetch_models_in_flight.store(false, Ordering::SeqCst);
            });
        });
    }
    let fetch_models_timer = Timer::new(&dialog);
    {
        let fetch_models_result = fetch_models_result.clone();
        let fetch_models_in_flight = fetch_models_in_flight.clone();
        let model_mapping_rows = model_mapping_rows.clone();
        let model_mapping_model = model_mapping_model.clone();
        fetch_models_timer.on_tick(move |_| {
            let Some(outcome) = fetch_models_result
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
            else {
                return;
            };
            fetch_models_button.set_label(text.ai_gw_fetch_models());
            fetch_models_button.enable(true);
            fetch_models_in_flight.store(false, Ordering::SeqCst);
            match outcome {
                Ok((models, normalized_base_url)) => {
                    if models.is_empty() {
                        show_error(&dialog, text.ai_gw_models_empty());
                    } else {
                        base_url_input.change_value(&normalized_base_url);
                        let existing_aliases =
                            model_mapping_aliases_by_upstream(&model_mapping_rows);
                        replace_model_mapping_rows(
                            &models_list,
                            &model_mapping_rows,
                            &model_mapping_model,
                            &models,
                            &existing_aliases,
                        );
                        let count = model_mapping_rows.borrow().len();
                        show_info(&dialog, &text.ai_gw_models_fetched(count));
                    }
                }
                Err(err) => show_error(&dialog, &text.ai_gw_models_fetch_failed(&err)),
            }
        });
    }
    fetch_models_timer.start(DIALOG_RESULT_POLL_MS, false);
    {
        let dialog = dialog;
        let fetch_models_closed = fetch_models_closed.clone();
        cancel_button.on_click(move |_| {
            fetch_models_closed.store(true, Ordering::SeqCst);
            dialog.end_modal(ID_CANCEL);
        });
    }
    {
        let dialog = dialog;
        save_button.on_click(move |_| dialog.end_modal(ID_OK));
    }

    name_input.set_focus();
    dialog.center();
    let result = dialog.show_modal();

    let provider = if result == ID_OK {
        let name = strip_nul(&name_input.get_value()).trim().to_string();
        if name.is_empty() {
            show_error(parent, text.ai_gw_provider_name_empty());
            None
        } else {
            let provider_type = initial
                .map(|provider| provider.provider_type.clone())
                .unwrap_or_else(|| {
                    selected_ai_gw_dialog_provider_type(
                        &radio_deepseek,
                        &radio_anthropic,
                        &radio_glm,
                    )
                });
            let compatibility = initial
                .and_then(|provider| provider.compatibility.clone())
                .or_else(|| selected_ai_gw_dialog_compatibility(&radio_anthropic, &radio_glm));
            let (models, explicit_aliases) = model_mapping_rows_to_config(&model_mapping_rows);
            let model_aliases = build_model_aliases_for_save(&models, explicit_aliases);
            let mut template = current_ai_gw_provider_template.borrow().clone();
            template.name = name.clone();
            template.provider_type = provider_type.clone();
            template.compatibility = compatibility.clone();
            template.base_url = strip_nul(&base_url_input.get_value()).trim().to_string();
            let models_url_value = strip_nul(&models_url_input.get_value());
            let models_url = normalize_optional_url(Some(&models_url_value));
            let weight = strip_nul(&weight_input.get_value())
                .trim()
                .parse::<u32>()
                .unwrap_or(100)
                .max(1);
            Some(ProviderConfig {
                name,
                enabled: initial.map(|provider| provider.enabled).unwrap_or(true),
                provider_type,
                compatibility,
                base_url: template.base_url,
                models_url,
                api_key: strip_nul(&key_input.get_value()).trim().to_string(),
                model_aliases,
                models,
                prompt_cache_retention: initial
                    .and_then(|provider| provider.prompt_cache_retention.clone()),
                weight,
                timeout_secs: initial
                    .map(|provider| provider.timeout_secs)
                    .unwrap_or(DEFAULT_PROVIDER_TIMEOUT_SECS),
            })
        }
    } else {
        None
    };

    fetch_models_closed.store(true, Ordering::SeqCst);
    fetch_models_timer.stop();
    dialog.destroy();
    provider
}

fn apply_ai_gw_dialog_template(
    text: GuiText,
    provider: Option<&ProviderConfig>,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    radio_anthropic: &RadioButton,
    radio_glm: &RadioButton,
    type_input: &TextCtrl,
    name_input: &TextCtrl,
    base_url_input: &TextCtrl,
    models_url_input: &TextCtrl,
    models_list: &DataViewCtrl,
    model_mapping_rows: &ModelMappingRows,
    model_mapping_model: &ModelMappingModel,
    weight_input: &TextCtrl,
) {
    let provider = normalize_provider_models_url(
        provider
            .cloned()
            .unwrap_or_else(|| default_ai_gw_service_provider(ProviderType::OpenAiResponses)),
    );

    set_ai_gw_dialog_provider_type(
        text,
        provider.provider_type.clone(),
        provider.compatibility.as_deref(),
        radio_openai,
        radio_deepseek,
        radio_anthropic,
        radio_glm,
        type_input,
    );
    name_input.change_value(&provider.name);
    base_url_input.change_value(&provider.base_url);
    models_url_input.change_value(provider.models_url.as_deref().unwrap_or_default());
    replace_model_mapping_rows(
        models_list,
        model_mapping_rows,
        model_mapping_model,
        &provider.models,
        &provider.model_aliases,
    );
    weight_input.change_value(&provider.effective_weight().to_string());
}

fn apply_ai_gw_service_template(
    text: GuiText,
    provider: ProviderConfig,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    radio_anthropic: &RadioButton,
    radio_glm: &RadioButton,
    type_input: &TextCtrl,
    name_input: &TextCtrl,
    base_url_input: &TextCtrl,
    models_url_input: &TextCtrl,
    key_input: &TextCtrl,
    models_list: &DataViewCtrl,
    model_mapping_rows: &ModelMappingRows,
    model_mapping_model: &ModelMappingModel,
    weight_input: &TextCtrl,
    service_template_applying: &Rc<RefCell<bool>>,
) {
    *service_template_applying.borrow_mut() = true;
    apply_ai_gw_dialog_template(
        text,
        Some(&provider),
        radio_openai,
        radio_deepseek,
        radio_anthropic,
        radio_glm,
        type_input,
        name_input,
        base_url_input,
        models_url_input,
        models_list,
        model_mapping_rows,
        model_mapping_model,
        weight_input,
    );
    key_input.change_value("");
    *service_template_applying.borrow_mut() = false;
}

fn bind_locked_ai_gw_service_selection(
    text: GuiText,
    provider_type: ProviderType,
    compatibility: Option<String>,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    radio_anthropic: &RadioButton,
    radio_glm: &RadioButton,
    type_input: &TextCtrl,
    service_template_applying: Rc<RefCell<bool>>,
) {
    bind_locked_ai_gw_service_radio(
        text,
        provider_type.clone(),
        compatibility.clone(),
        radio_openai,
        radio_openai,
        radio_deepseek,
        radio_anthropic,
        radio_glm,
        type_input,
        service_template_applying.clone(),
    );
    bind_locked_ai_gw_service_radio(
        text,
        provider_type.clone(),
        compatibility.clone(),
        radio_deepseek,
        radio_openai,
        radio_deepseek,
        radio_anthropic,
        radio_glm,
        type_input,
        service_template_applying.clone(),
    );
    bind_locked_ai_gw_service_radio(
        text,
        provider_type.clone(),
        compatibility.clone(),
        radio_anthropic,
        radio_openai,
        radio_deepseek,
        radio_anthropic,
        radio_glm,
        type_input,
        service_template_applying.clone(),
    );
    bind_locked_ai_gw_service_radio(
        text,
        provider_type,
        compatibility,
        radio_glm,
        radio_openai,
        radio_deepseek,
        radio_anthropic,
        radio_glm,
        type_input,
        service_template_applying,
    );
}

fn bind_locked_ai_gw_service_radio(
    text: GuiText,
    provider_type: ProviderType,
    compatibility: Option<String>,
    radio: &RadioButton,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    radio_anthropic: &RadioButton,
    radio_glm: &RadioButton,
    type_input: &TextCtrl,
    service_template_applying: Rc<RefCell<bool>>,
) {
    let radio = *radio;
    let radio_openai = *radio_openai;
    let radio_deepseek = *radio_deepseek;
    let radio_anthropic = *radio_anthropic;
    let radio_glm = *radio_glm;
    let type_input = *type_input;
    radio.on_selected(move |_| {
        if *service_template_applying.borrow() {
            return;
        }
        *service_template_applying.borrow_mut() = true;
        set_ai_gw_dialog_provider_type(
            text,
            provider_type.clone(),
            compatibility.as_deref(),
            &radio_openai,
            &radio_deepseek,
            &radio_anthropic,
            &radio_glm,
            &type_input,
        );
        *service_template_applying.borrow_mut() = false;
    });
}

fn default_ai_gw_service_provider(provider_type: ProviderType) -> ProviderConfig {
    match provider_type {
        ProviderType::OpenAiResponses => ProviderConfig {
            name: "openai".to_string(),
            provider_type: ProviderType::OpenAiResponses,
            base_url: "https://api.openai.com/v1".to_string(),
            ..Default::default()
        },
        ProviderType::ChatCompletions => ProviderConfig {
            name: "deepseek".to_string(),
            provider_type: ProviderType::ChatCompletions,
            base_url: "https://api.deepseek.com/v1".to_string(),
            ..Default::default()
        },
        ProviderType::AnthropicMessages => ProviderConfig {
            name: "anthropic".to_string(),
            provider_type: ProviderType::AnthropicMessages,
            compatibility: Some("anthropic".to_string()),
            base_url: "https://api.anthropic.com/v1".to_string(),
            ..Default::default()
        },
    }
}

fn default_ai_gw_glm_service_provider() -> ProviderConfig {
    ProviderConfig {
        name: "glm".to_string(),
        provider_type: ProviderType::AnthropicMessages,
        compatibility: Some("glm_anthropic".to_string()),
        base_url: "https://open.bigmodel.cn/api/anthropic".to_string(),
        ..Default::default()
    }
}

fn normalize_provider_models_url(mut provider: ProviderConfig) -> ProviderConfig {
    provider.models_url = normalize_optional_url(provider.models_url.as_deref())
        .filter(|models_url| !is_default_models_url_for_base(&provider.base_url, models_url))
        .filter(|models_url| {
            known_models_url_for_provider(&provider)
                .as_deref()
                .is_none_or(|known_url| known_url.trim_end_matches('/') != models_url.as_str())
        });
    provider
}

fn is_default_models_url_for_base(base_url: &str, models_url: &str) -> bool {
    let normalized = models_url.trim().trim_end_matches('/');
    let raw = base_url.trim().trim_end_matches('/');
    let root = provider_api_root(raw);
    normalized == format!("{raw}/models") || normalized == format!("{root}/v1/models")
}

fn known_models_url_for_provider(provider: &ProviderConfig) -> Option<String> {
    let provider_name = provider.name.trim();
    let base_url = provider.base_url.trim().to_ascii_lowercase();
    if (matches!(
        provider.compatibility.as_deref(),
        Some("glm_anthropic" | "zhipu_anthropic")
    ) || provider_name.eq_ignore_ascii_case("glm")
        || provider_name.eq_ignore_ascii_case("zhipu"))
        && base_url.contains("open.bigmodel.cn")
    {
        return Some("https://open.bigmodel.cn/api/paas/v4/models".to_string());
    }

    None
}

fn normalize_optional_url(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
}

fn set_ai_gw_dialog_provider_type(
    text: GuiText,
    provider_type: ProviderType,
    compatibility: Option<&str>,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    radio_anthropic: &RadioButton,
    radio_glm: &RadioButton,
    type_input: &TextCtrl,
) {
    radio_openai.set_value(false);
    radio_deepseek.set_value(false);
    radio_anthropic.set_value(false);
    radio_glm.set_value(false);
    match provider_type {
        ProviderType::ChatCompletions => {
            radio_deepseek.set_value(true);
            type_input.change_value(text.provider_type_chat_completions());
        }
        ProviderType::OpenAiResponses => {
            radio_openai.set_value(true);
            type_input.change_value(text.provider_type_openai_responses());
        }
        ProviderType::AnthropicMessages => {
            if matches!(compatibility, Some("glm_anthropic" | "zhipu_anthropic")) {
                radio_glm.set_value(true);
                type_input.change_value(text.provider_type_glm_anthropic_messages());
            } else {
                radio_anthropic.set_value(true);
                type_input.change_value(text.provider_type_anthropic_messages());
            }
        }
    }
}

fn selected_ai_gw_dialog_compatibility(
    radio_anthropic: &RadioButton,
    radio_glm: &RadioButton,
) -> Option<String> {
    if radio_glm.get_value() {
        Some("glm_anthropic".to_string())
    } else if radio_anthropic.get_value() {
        Some("anthropic".to_string())
    } else {
        None
    }
}

fn selected_ai_gw_dialog_provider_type(
    radio_deepseek: &RadioButton,
    radio_anthropic: &RadioButton,
    radio_glm: &RadioButton,
) -> ProviderType {
    if radio_anthropic.get_value() || radio_glm.get_value() {
        ProviderType::AnthropicMessages
    } else if radio_deepseek.get_value() {
        ProviderType::ChatCompletions
    } else {
        ProviderType::OpenAiResponses
    }
}

fn parse_model_ids(value: &str) -> Vec<String> {
    value
        .split([',', '\n', '\r', ';'])
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn model_mapping_cell(rows: &ModelMappingRows, row: usize, col: usize) -> Variant {
    let rows = rows.borrow();
    let Some(row_data) = rows.get(row) else {
        return String::new().into();
    };
    match col {
        0 => row_data.upstream_model.clone().into(),
        1 => row_data.codex_models.join(", ").into(),
        _ => String::new().into(),
    }
}

fn replace_model_mapping_rows(
    list: &DataViewCtrl,
    rows: &ModelMappingRows,
    model: &ModelMappingModel,
    upstream_models: &[String],
    aliases: &BTreeMap<String, String>,
) {
    let new_rows = model_mapping_rows_from_config(upstream_models, aliases);
    replace_model_mapping_rows_data(list, rows, model, new_rows);
}

fn replace_model_mapping_rows_data(
    list: &DataViewCtrl,
    rows: &ModelMappingRows,
    model: &ModelMappingModel,
    new_rows: Vec<ModelMappingRow>,
) {
    let new_len = new_rows.len();
    {
        let mut current_rows = rows.borrow_mut();
        *current_rows = new_rows;
    }
    model.borrow_mut().reset(new_len);
    if new_len > 0 {
        list.select_row(0);
    }
}

fn append_model_mapping_rows(
    list: &DataViewCtrl,
    rows: &ModelMappingRows,
    model: &ModelMappingModel,
    upstream_models: impl IntoIterator<Item = String>,
    aliases: Option<&BTreeMap<String, String>>,
) -> usize {
    let mut added = 0;
    let mut select_row = None;
    {
        let mut current_rows = rows.borrow_mut();
        for upstream_model in upstream_models {
            let upstream_model = upstream_model.trim().to_string();
            if upstream_model.is_empty()
                || current_rows
                    .iter()
                    .any(|row| row.upstream_model == upstream_model)
            {
                continue;
            }
            let codex_models = aliases
                .map(|aliases| aliases_for_upstream_model(aliases, &upstream_model))
                .unwrap_or_default();
            current_rows.push(ModelMappingRow {
                upstream_model,
                codex_models,
            });
            select_row = Some(current_rows.len() - 1);
            added += 1;
        }
    }
    if added > 0 {
        let len = rows.borrow().len();
        model.borrow_mut().reset(len);
        if let Some(row) = select_row {
            list.select_row(row);
        }
    }
    added
}

fn delete_model_mapping_row(
    list: &DataViewCtrl,
    rows: &ModelMappingRows,
    model: &ModelMappingModel,
    index: usize,
) {
    let new_len = {
        let mut current_rows = rows.borrow_mut();
        if index >= current_rows.len() {
            return;
        }
        current_rows.remove(index);
        current_rows.len()
    };
    model.borrow_mut().reset(new_len);
    if new_len > 0 {
        list.select_row(index.min(new_len - 1));
    }
}

fn edit_model_mapping_row(
    parent: &dyn WxWidget,
    text: GuiText,
    row: usize,
    rows: &ModelMappingRows,
    model: &ModelMappingModel,
) {
    let (upstream_model, current_aliases) = {
        let current_rows = rows.borrow();
        let Some(row_data) = current_rows.get(row) else {
            return;
        };
        (
            row_data.upstream_model.clone(),
            row_data.codex_models.join(", "),
        )
    };
    let input = TextEntryDialog::builder(
        parent,
        &text.ai_gw_model_alias_prompt(&upstream_model),
        text.ai_gw_edit_model_aliases(),
    )
    .with_default_value(&current_aliases)
    .with_style(
        TextEntryDialogStyle::Ok | TextEntryDialogStyle::Cancel | TextEntryDialogStyle::Centre,
    )
    .build();
    let result = input.show_modal();
    if result != ID_OK {
        return;
    }
    let Some(value) = input.get_value() else {
        return;
    };
    let aliases = parse_model_ids(&value);
    {
        let mut current_rows = rows.borrow_mut();
        let Some(row_data) = current_rows.get_mut(row) else {
            return;
        };
        row_data.codex_models = aliases;
    }
    model.borrow().row_changed(row);
}

fn model_mapping_rows_from_config(
    upstream_models: &[String],
    aliases: &BTreeMap<String, String>,
) -> Vec<ModelMappingRow> {
    let aliases = with_inferred_model_aliases(upstream_models, aliases);
    let mut rows: Vec<ModelMappingRow> = upstream_models
        .iter()
        .map(|upstream_model| ModelMappingRow {
            upstream_model: upstream_model.clone(),
            codex_models: aliases_for_upstream_model(&aliases, upstream_model),
        })
        .collect();
    for upstream_model in aliases.values() {
        if upstream_model.trim().is_empty()
            || rows
                .iter()
                .any(|row| row.upstream_model.as_str() == upstream_model)
        {
            continue;
        }
        rows.push(ModelMappingRow {
            upstream_model: upstream_model.clone(),
            codex_models: aliases_for_upstream_model(&aliases, upstream_model),
        });
    }
    rows
}

fn aliases_for_upstream_model(
    aliases: &BTreeMap<String, String>,
    upstream_model: &str,
) -> Vec<String> {
    let mut codex_models = Vec::new();
    for (codex_model, mapped_upstream) in aliases {
        if mapped_upstream == upstream_model && !codex_models.iter().any(|item| item == codex_model)
        {
            codex_models.push(codex_model.clone());
        }
    }
    codex_models
}

fn model_mapping_aliases_by_upstream(rows: &ModelMappingRows) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    for row in rows.borrow().iter() {
        for codex_model in &row.codex_models {
            aliases.insert(codex_model.clone(), row.upstream_model.clone());
        }
    }
    aliases
}

fn model_mapping_rows_to_config(
    rows: &ModelMappingRows,
) -> (Vec<String>, BTreeMap<String, String>) {
    let mut models = Vec::new();
    let mut aliases = BTreeMap::new();
    for row in rows.borrow().iter() {
        let upstream_model = row.upstream_model.trim();
        if upstream_model.is_empty() || models.iter().any(|model| model == upstream_model) {
            continue;
        }
        models.push(upstream_model.to_string());
        for codex_model in &row.codex_models {
            let codex_model = codex_model.trim();
            if codex_model.is_empty() || codex_model == upstream_model {
                continue;
            }
            aliases.insert(codex_model.to_string(), upstream_model.to_string());
        }
    }
    (models, aliases)
}

fn build_model_aliases_for_save(
    models: &[String],
    mut explicit_aliases: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    for (codex_model, upstream_model) in inferred_model_aliases(models) {
        explicit_aliases
            .entry(codex_model)
            .or_insert(upstream_model);
    }
    explicit_aliases
}

fn with_inferred_model_aliases(
    models: &[String],
    aliases: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = aliases.clone();
    for (codex_model, upstream_model) in inferred_model_aliases(models) {
        merged.entry(codex_model).or_insert(upstream_model);
    }
    merged
}

fn inferred_model_aliases(models: &[String]) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    for model in models {
        let canonical = canonical_model_alias_key(model);
        if canonical != model.as_str() && models.iter().all(|item| item != &canonical) {
            aliases.insert(canonical, model.clone());
        }
    }
    aliases
}

fn canonical_model_alias_key(model: &str) -> String {
    match model.trim().to_ascii_lowercase().as_str() {
        "claude-opus-4-8" => "opus-4.8".to_string(),
        "claude-sonnet-4-6" => "sonnet-4.6".to_string(),
        model => model.to_string(),
    }
}

#[cfg(test)]
mod model_mapping_tests {
    use super::*;

    #[test]
    fn infers_anthropic_claude_model_aliases() {
        let models = vec![
            "claude-opus-4-8".to_string(),
            "claude-sonnet-4-6".to_string(),
        ];

        let aliases = inferred_model_aliases(&models);

        assert_eq!(
            aliases.get("opus-4.8").map(String::as_str),
            Some("claude-opus-4-8")
        );
        assert_eq!(
            aliases.get("sonnet-4.6").map(String::as_str),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn shows_inferred_anthropic_aliases_in_mapping_rows() {
        let models = vec![
            "claude-opus-4-8".to_string(),
            "claude-sonnet-4-6".to_string(),
        ];
        let rows = model_mapping_rows_from_config(&models, &BTreeMap::new());

        assert_eq!(rows[0].upstream_model, "claude-opus-4-8");
        assert_eq!(rows[0].codex_models, vec!["opus-4.8"]);
        assert_eq!(rows[1].upstream_model, "claude-sonnet-4-6");
        assert_eq!(rows[1].codex_models, vec!["sonnet-4.6"]);
    }

    #[test]
    fn min_frame_size_never_exceeds_actual_frame() {
        // On a tiny window the floor must shrink with it, otherwise the min-size
        // constraint would itself force off-screen content.
        let tiny = Size::new(640, 400);
        let floor = min_frame_size(tiny);
        assert!(floor.width <= tiny.width);
        assert!(floor.height <= tiny.height);

        // On a roomy window the floor stays at the usable minimums.
        let roomy = Size::new(1600, 1000);
        let floor = min_frame_size(roomy);
        assert_eq!(floor.width, MIN_FRAME_WIDTH);
        assert_eq!(floor.height, MIN_FRAME_HEIGHT);
    }

    #[test]
    fn codex_app_status_ignores_unknown_connection_initialization() {
        let remote = RemoteControlStatus {
            connected: true,
            initialized: false,
            active_source_kind: None,
            connections: vec![RemoteControlConnectionStatus {
                connected: true,
                initialized: false,
                source_kind: "unknown".to_string(),
            }],
        };

        assert_eq!(
            endpoint_status_state(Some(&remote), "codex_app", true),
            EndpointStatusState::NotConnected
        );
    }

    #[test]
    fn endpoint_status_is_uninitialized_when_not_configured() {
        assert_eq!(
            endpoint_status_state(
                Some(&RemoteControlStatus {
                    connected: false,
                    initialized: false,
                    active_source_kind: None,
                    connections: vec![],
                }),
                "codex_app",
                false
            ),
            EndpointStatusState::UninitializedConfig
        );
    }

    #[test]
    fn endpoint_status_is_loading_when_remote_snapshot_is_missing() {
        assert_eq!(
            endpoint_status_state(None, "codex_app", true),
            EndpointStatusState::Loading
        );
    }

    #[test]
    fn endpoint_status_is_not_connected_when_configured_without_source_connection() {
        let remote = RemoteControlStatus {
            connected: true,
            initialized: true,
            active_source_kind: Some("codex_app".to_string()),
            connections: vec![RemoteControlConnectionStatus {
                connected: true,
                initialized: true,
                source_kind: "codex_app".to_string(),
            }],
        };

        assert_eq!(
            endpoint_status_state(Some(&remote), "vscode", true),
            EndpointStatusState::NotConnected
        );
    }

    #[test]
    fn endpoint_status_initializing_is_source_specific() {
        let remote = RemoteControlStatus {
            connected: true,
            initialized: false,
            active_source_kind: None,
            connections: vec![RemoteControlConnectionStatus {
                connected: true,
                initialized: false,
                source_kind: "vscode".to_string(),
            }],
        };

        assert_eq!(
            endpoint_status_state(Some(&remote), "vscode", true),
            EndpointStatusState::Initializing
        );
        assert_eq!(
            endpoint_status_state(Some(&remote), "cli", true),
            EndpointStatusState::NotConnected
        );
    }

    #[test]
    fn endpoint_status_connected_takes_priority_over_config_state() {
        let remote = RemoteControlStatus {
            connected: true,
            initialized: true,
            active_source_kind: Some("codex_app".to_string()),
            connections: vec![],
        };

        assert_eq!(
            endpoint_status_state(Some(&remote), "codex_app", false),
            EndpointStatusState::Connected
        );
    }

    #[test]
    fn endpoint_status_labels_are_unified_for_zh_cn() {
        let text = GuiText::new(GuiLocale::ZhCn);

        assert_eq!(
            endpoint_status_label(text, EndpointStatusState::UninitializedConfig),
            "未初始化配置"
        );
        assert_eq!(
            endpoint_status_label(text, EndpointStatusState::NotConnected),
            "未连接"
        );
        assert_eq!(
            endpoint_status_label(text, EndpointStatusState::Initializing),
            "初始化中"
        );
        assert_eq!(
            endpoint_status_label(text, EndpointStatusState::Connected),
            "已连接"
        );
        assert_eq!(
            endpoint_status_label(text, EndpointStatusState::Loading),
            "读取中"
        );
    }

    #[test]
    fn codex_app_status_initializes_only_for_codex_app_connection() {
        let remote = RemoteControlStatus {
            connected: true,
            initialized: false,
            active_source_kind: None,
            connections: vec![RemoteControlConnectionStatus {
                connected: true,
                initialized: false,
                source_kind: "codex_app".to_string(),
            }],
        };

        assert_eq!(
            endpoint_status_state(Some(&remote), "codex_app", true),
            EndpointStatusState::Initializing
        );
    }

    #[test]
    fn vscode_status_is_not_connected_when_only_codex_app_is_connected() {
        let remote = RemoteControlStatus {
            connected: true,
            initialized: true,
            active_source_kind: Some("codex_app".to_string()),
            connections: vec![RemoteControlConnectionStatus {
                connected: true,
                initialized: true,
                source_kind: "codex_app".to_string(),
            }],
        };

        assert_eq!(
            endpoint_status_state(Some(&remote), "vscode", true),
            EndpointStatusState::NotConnected
        );
    }
}

fn fetch_remote_models(
    base_url: &str,
    models_url: Option<&str>,
    fallback_models_url: Option<&str>,
    api_key: &str,
    timeout_secs: u64,
) -> Result<(Vec<String>, String), String> {
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| err.to_string())?;
    let mut errors = Vec::new();
    for candidate in model_list_candidates(base_url, models_url, fallback_models_url) {
        let mut request = client.get(&candidate.url);
        if !api_key.trim().is_empty() {
            request = request.header("authorization", format!("Bearer {}", api_key.trim()));
        }

        let response = match request.send() {
            Ok(response) => response,
            Err(err) => {
                errors.push(format!("{}: {}", candidate.url, err));
                continue;
            }
        };
        let status = response.status();
        let body = match response.text() {
            Ok(body) => body,
            Err(err) => {
                errors.push(format!("{}: {}", candidate.url, err));
                continue;
            }
        };
        if !status.is_success() {
            errors.push(format!(
                "{}: HTTP {status}: {}",
                candidate.url,
                response_preview(&body)
            ));
            continue;
        }

        match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(json) => return Ok((extract_model_ids(&json), candidate.normalized_base_url)),
            Err(err) => errors.push(format!(
                "{}: response is not JSON ({err}): {}",
                candidate.url,
                response_preview(&body)
            )),
        }
    }
    Err(errors.join("; "))
}

struct ModelListCandidate {
    url: String,
    normalized_base_url: String,
}

fn model_list_candidates(
    base_url: &str,
    models_url: Option<&str>,
    fallback_models_url: Option<&str>,
) -> Vec<ModelListCandidate> {
    let raw = base_url.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    if let Some(models_url) = models_url.map(str::trim).filter(|value| !value.is_empty()) {
        push_configured_model_list_candidates(&mut candidates, models_url, raw);
    }

    let root = provider_api_root(raw);
    push_model_list_candidate(&mut candidates, format!("{raw}/models"), raw.to_string());
    push_model_list_candidate(
        &mut candidates,
        format!("{root}/v1/models"),
        provider_display_base_url(&root),
    );
    if models_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
        && let Some(fallback_models_url) = fallback_models_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        push_configured_model_list_candidates(&mut candidates, fallback_models_url, raw);
    }
    candidates
}

fn push_configured_model_list_candidates(
    candidates: &mut Vec<ModelListCandidate>,
    models_url: &str,
    base_url: &str,
) {
    let configured = models_url.trim().trim_end_matches('/');
    if configured.is_empty() {
        return;
    }

    let normalized_base_url = provider_display_base_url(base_url);
    if configured.to_ascii_lowercase().ends_with("/models") {
        push_model_list_candidate(candidates, configured.to_string(), normalized_base_url);
        return;
    }

    push_model_list_candidate(
        candidates,
        format!("{configured}/models"),
        normalized_base_url.clone(),
    );
    let root = provider_api_root(configured);
    push_model_list_candidate(candidates, format!("{root}/v1/models"), normalized_base_url);
}

fn push_model_list_candidate(
    candidates: &mut Vec<ModelListCandidate>,
    url: String,
    normalized_base_url: String,
) {
    if candidates.iter().any(|candidate| candidate.url == url) {
        return;
    }
    candidates.push(ModelListCandidate {
        url,
        normalized_base_url,
    });
}

fn response_preview(body: &str) -> String {
    let preview: String = body.chars().take(240).collect();
    preview.replace(['\r', '\n', '\t'], " ")
}

fn extract_model_ids(value: &serde_json::Value) -> Vec<String> {
    let mut models = Vec::new();
    if let Some(items) = value.get("data").and_then(|value| value.as_array()) {
        push_model_items(&mut models, items);
    } else if let Some(items) = value.get("models").and_then(|value| value.as_array()) {
        push_model_items(&mut models, items);
    } else if let Some(items) = value.as_array() {
        push_model_items(&mut models, items);
    }
    models
}

fn push_model_items(models: &mut Vec<String>, items: &[serde_json::Value]) {
    for item in items {
        let id = item
            .as_str()
            .or_else(|| item.get("id").and_then(|value| value.as_str()))
            .or_else(|| item.get("slug").and_then(|value| value.as_str()))
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = id {
            let id = id.to_string();
            if !models.iter().any(|existing| existing == &id) {
                models.push(id);
            }
        }
    }
}

fn handle_language_selected(frame: &Frame, text: GuiText, locale: GuiLocale) {
    if let Some(menu_bar) = frame.get_menu_bar() {
        menu_bar.check_item(ID_MENU_LANGUAGE_ZH_CN, locale == GuiLocale::ZhCn);
        menu_bar.check_item(ID_MENU_LANGUAGE_EN_US, locale == GuiLocale::EnUs);
    }
    match save_gui_locale(locale) {
        Ok(()) => show_info(frame, text.language_restart_message()),
        Err(err) => show_error(frame, &format!("{}: {err}", text.language_save_failed())),
    }
}

fn handle_theme_selected(frame: &Frame, text: GuiText, mode: ThemeMode) {
    if let Some(menu_bar) = frame.get_menu_bar() {
        menu_bar.check_item(ID_MENU_THEME_SYSTEM, mode == ThemeMode::System);
        menu_bar.check_item(ID_MENU_THEME_LIGHT, mode == ThemeMode::Light);
        menu_bar.check_item(ID_MENU_THEME_DARK, mode == ThemeMode::Dark);
    }
    match save_gui_theme(mode) {
        Ok(()) => show_info(frame, text.theme_restart_message()),
        Err(err) => show_error(frame, &format!("{}: {err}", text.theme_save_failed())),
    }
}

fn bind_service_connection_settings(frame: &Frame, handles: &UiHandles) {
    let button = handles.service_settings_button;
    let text = handles.text;
    let frame_for_button = *frame;
    button.on_click(move |_| {
        let current = load_local_connection_mode();
        let (label, help) = match current {
            LocalConnectionMode::Standard => (
                text.switch_to_vpn_compatible_connection(),
                text.local_connection_switch_help(),
            ),
            LocalConnectionMode::VpnCompatible => (
                text.switch_to_standard_connection(),
                text.local_connection_switch_help(),
            ),
        };
        let mut menu = Menu::builder()
            .append_item(ID_SERVICE_CONNECTION_SWITCH, label, help)
            .build();
        frame_for_button.popup_menu(&mut menu, None);
    });

    let frame_for_menu = *frame;
    frame.on_menu_selected(move |event| {
        if event.get_id() != ID_SERVICE_CONNECTION_SWITCH {
            return;
        }
        let current = load_local_connection_mode();
        let next = match current {
            LocalConnectionMode::Standard => LocalConnectionMode::VpnCompatible,
            LocalConnectionMode::VpnCompatible => LocalConnectionMode::Standard,
        };
        handle_local_connection_selected(&frame_for_menu, text, next);
    });
}

fn handle_local_connection_selected(frame: &Frame, text: GuiText, mode: LocalConnectionMode) {
    match save_local_connection_mode(mode) {
        Ok(()) => show_info(frame, text.local_connection_restart_message()),
        Err(err) => show_error(
            frame,
            &format!("{}: {err}", text.local_connection_save_failed()),
        ),
    }
}

#[derive(Clone)]
struct UiHandles {
    text: GuiText,
    service_status: StatusPanel,
    service_settings_button: Button,
    im_status: ImStatusPanel,
    codex_status: StatusPanel,
    vscode_status: StatusPanel,
    cli_status: StatusPanel,
    im_account_list: DataViewCtrl,
    im_account_rows: ImAccountRows,
    im_account_model: ImAccountModel,
    pending_im_toggle: PendingImToggle,
    delete_im_account_button: Button,
    save_telegram_button: Button,
    connect_wechat_button: Button,
    change_bot_button: Button,
    codex_tab: CodexTab,
    // AI Gateway fields
    ai_gw_provider_list: DataViewCtrl,
    ai_gw_provider_rows: AiGwProviderRows,
    ai_gw_provider_model: AiGwProviderModel,
    pending_ai_gw_channel_toggle: PendingAiGwChannelToggle,
    ai_gw_filter_image_generation: CheckBox,
    ai_gw_enable_logging: CheckBox,
    ai_gw_delete_button: Button,
    ai_gw_new_button: Button,
    ai_gw_edit_button: Button,
    // Request logs fields
    request_log_list: DataViewCtrl,
    request_log_rows: RequestLogRows,
    request_log_model: RequestLogModel,
    request_log_disabled_hint: StaticText,
}

#[derive(Clone)]
struct DashboardRefresh {
    in_flight: Arc<AtomicBool>,
    result: Arc<Mutex<Option<(u64, DashboardSnapshot)>>>,
    last_snapshot: Arc<Mutex<Option<DashboardSnapshot>>>,
    daemon_starting: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    connection_prompt_shown: Arc<AtomicBool>,
    compatible_probe_hits: Arc<AtomicU64>,
    pending_startup_child: Arc<Mutex<Option<Child>>>,
    gui_tx: tokio_mpsc::UnboundedSender<GuiMessage>,
}

impl DashboardRefresh {
    fn new(gui_tx: tokio_mpsc::UnboundedSender<GuiMessage>) -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
            last_snapshot: Arc::new(Mutex::new(None)),
            daemon_starting: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            closing: Arc::new(AtomicBool::new(false)),
            connection_prompt_shown: Arc::new(AtomicBool::new(false)),
            compatible_probe_hits: Arc::new(AtomicU64::new(0)),
            pending_startup_child: Arc::new(Mutex::new(None)),
            gui_tx,
        }
    }
}

enum ImActionResult {
    TelegramConfigure(Result<serde_json::Value, String>),
    AccountToggle {
        row: usize,
        previous_enabled: bool,
        result: Result<serde_json::Value, String>,
    },
    AccountDelete(Result<serde_json::Value, String>),
}

fn revert_im_toggle(handles: &UiHandles, row: usize, previous_enabled: bool) {
    if let Some(row_data) = handles.im_account_rows.borrow_mut().get_mut(row) {
        row_data[4] = previous_enabled.to_string();
    }
    handles.im_account_model.borrow().row_value_changed(row, 4);
}

fn revert_ai_gw_toggle(handles: &UiHandles, row: usize, previous_enabled: bool) {
    if let Some(row_data) = handles.ai_gw_provider_rows.borrow_mut().get_mut(row) {
        row_data.enabled = previous_enabled;
    }
    handles
        .ai_gw_provider_model
        .borrow()
        .row_value_changed(row, 0);
}

/// Process a pending IM account enable/disable toggle queued from the data view.
/// Runs on the idle loop instead of a polling timer; spawns the request and
/// reports the outcome back through the GUI message channel.
fn process_pending_im_toggle(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    refresh: &DashboardRefresh,
    gui_tx: &tokio_mpsc::UnboundedSender<GuiMessage>,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.load(Ordering::SeqCst) {
        return;
    }
    let Some(toggle) = handles.pending_im_toggle.borrow_mut().take() else {
        return;
    };
    if !ensure_service_ready_for_action(api, frame, refresh) {
        revert_im_toggle(handles, toggle.row, toggle.previous_enabled);
        return;
    }
    in_flight.store(true, Ordering::SeqCst);
    let request = SetImAccountEnabledRequest {
        platform: toggle.platform,
        account_id: toggle.account_id,
        enabled: toggle.enabled,
    };
    let row = toggle.row;
    let previous_enabled = toggle.previous_enabled;
    let thread_api = api.clone();
    let gui_tx = gui_tx.clone();
    let in_flight = in_flight.clone();
    thread::spawn(move || {
        let outcome = thread_api.set_im_account_enabled(&request);
        in_flight.store(false, Ordering::SeqCst);
        let _ = gui_tx.send(GuiMessage::ImAction(ImActionResult::AccountToggle {
            row,
            previous_enabled,
            result: outcome,
        }));
        wxdragon::wake_up_idle();
    });
}

/// Process a pending AI Gateway channel enable/disable toggle queued from the
/// data view. Idle-driven counterpart of the former polling timer.
fn process_pending_ai_gw_toggle(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    refresh: &DashboardRefresh,
    gui_tx: &tokio_mpsc::UnboundedSender<GuiMessage>,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.load(Ordering::SeqCst) {
        return;
    }
    let Some(toggle) = handles.pending_ai_gw_channel_toggle.borrow_mut().take() else {
        return;
    };
    if !ensure_service_ready_for_action(api, frame, refresh) {
        revert_ai_gw_toggle(handles, toggle.row, toggle.previous_enabled);
        return;
    }
    in_flight.store(true, Ordering::SeqCst);
    let row = toggle.row;
    let previous_enabled = toggle.previous_enabled;
    let name = toggle.name;
    let enabled = toggle.enabled;
    let worker_api = api.clone();
    let gui_tx = gui_tx.clone();
    let in_flight = in_flight.clone();
    thread::spawn(move || {
        let outcome = set_ai_gw_provider_enabled(&worker_api, &name, enabled);
        in_flight.store(false, Ordering::SeqCst);
        let _ = gui_tx.send(GuiMessage::AiGwAction(AiGwActionResult::ChannelToggle {
            row,
            previous_enabled,
            result: outcome,
        }));
        wxdragon::wake_up_idle();
    });
}

fn schedule_dashboard_refresh(api: &ApiClient, refresh: &DashboardRefresh) -> bool {
    if refresh.in_flight.swap(true, Ordering::SeqCst) {
        return false;
    }

    let generation = refresh.generation.load(Ordering::SeqCst);
    spawn_dashboard_refresh(api, refresh, generation);
    true
}

fn force_dashboard_refresh(api: &ApiClient, refresh: &DashboardRefresh) -> bool {
    let generation = refresh.generation.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut result) = refresh.result.lock() {
        result.take();
    }
    refresh.in_flight.store(true, Ordering::SeqCst);
    spawn_dashboard_refresh(api, refresh, generation);
    true
}

fn spawn_dashboard_refresh(api: &ApiClient, refresh: &DashboardRefresh, generation: u64) {
    let api = api.clone();
    let result = refresh.result.clone();
    let in_flight = refresh.in_flight.clone();
    let current_generation = refresh.generation.clone();
    let gui_tx = refresh.gui_tx.clone();
    thread::spawn(move || {
        let snapshot = api.dashboard();
        if generation == current_generation.load(Ordering::SeqCst)
            && let Ok(mut slot) = result.lock()
        {
            slot.replace((generation, snapshot));
        }
        in_flight.store(false, Ordering::SeqCst);
        let _ = gui_tx.send(GuiMessage::DashboardUpdate);
        wxdragon::wake_up_idle();
    });
}

fn apply_pending_dashboard(
    handles: &UiHandles,
    refresh: &DashboardRefresh,
    api: &ApiClient,
    frame: &Frame,
) -> bool {
    let result = refresh.result.lock().ok().and_then(|mut slot| slot.take());
    let Some((generation, snapshot)) = result else {
        return false;
    };
    if generation != refresh.generation.load(Ordering::SeqCst) {
        return false;
    }

    let daemon_starting = refresh.daemon_starting.load(Ordering::SeqCst);
    update_dashboard(handles, &snapshot, daemon_starting);
    maybe_prompt_compatible_connection(handles, refresh, api, frame, &snapshot, daemon_starting);
    if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
        last_snapshot.replace(snapshot);
    }
    true
}

fn cached_dashboard_snapshot(refresh: &DashboardRefresh) -> Option<DashboardSnapshot> {
    refresh
        .last_snapshot
        .lock()
        .ok()
        .and_then(|snapshot| snapshot.clone())
}

fn maybe_prompt_compatible_connection(
    handles: &UiHandles,
    refresh: &DashboardRefresh,
    api: &ApiClient,
    frame: &Frame,
    snapshot: &DashboardSnapshot,
    daemon_starting: bool,
) {
    if snapshot.service_online
        || daemon_starting
        || snapshot.local_connection_mode != LocalConnectionMode::Standard
        || !snapshot.compatible_connection_available
    {
        refresh.compatible_probe_hits.store(0, Ordering::SeqCst);
        return;
    }
    let hits = refresh.compatible_probe_hits.fetch_add(1, Ordering::SeqCst) + 1;
    if hits < 3 || api.is_online() || refresh.connection_prompt_shown.swap(true, Ordering::SeqCst) {
        return;
    }
    if confirm_switch_compatible_connection(frame, handles.text) {
        handle_local_connection_selected(frame, handles.text, LocalConnectionMode::VpnCompatible);
    }
}

fn local_connection_display_addr(bind: &str, mode: LocalConnectionMode) -> String {
    let port = bind
        .rsplit_once(':')
        .and_then(|(_, value)| value.parse::<u16>().ok())
        .unwrap_or(3847);
    match mode {
        LocalConnectionMode::Standard => format!("127.0.0.1:{port}"),
        LocalConnectionMode::VpnCompatible => format!("localhost:{port}"),
    }
}

fn ensure_service_ready_for_action(
    api: &ApiClient,
    frame: &Frame,
    refresh: &DashboardRefresh,
) -> bool {
    if refresh.daemon_starting.load(Ordering::SeqCst) {
        show_info(
            frame,
            GuiText::new(load_gui_locale()).service_starting_wait(),
        );
        return false;
    }
    if api.is_online() {
        return true;
    }

    force_dashboard_refresh(api, refresh);
    show_error(
        frame,
        GuiText::new(load_gui_locale()).service_not_ready_retry(),
    );
    false
}

fn show_dashboard_starting(handles: &UiHandles) {
    let text = handles.text;
    set_status_panel(
        &handles.service_status,
        text.starting(),
        text.local_connection_label(load_local_connection_mode()),
        StateTone::Warn,
    );
    set_im_channel_row(
        &handles.im_status.feishu,
        text.waiting_service(),
        text.service_reads_status(),
        StateTone::Muted,
    );
    set_im_channel_row(
        &handles.im_status.telegram,
        text.waiting_service(),
        text.service_reads_status(),
        StateTone::Muted,
    );
    set_im_channel_row(
        &handles.im_status.wechat,
        text.waiting_service(),
        text.service_reads_status(),
        StateTone::Muted,
    );
    set_disabled_status_panel(
        &handles.codex_status,
        text.waiting_service(),
        if CODEX_APP_GUI_UNSUPPORTED {
            text.app_gui_unsupported()
        } else {
            ""
        },
    );
    set_status_panel(
        &handles.vscode_status,
        text.waiting_service(),
        "",
        StateTone::Muted,
    );
    set_status_panel(
        &handles.cli_status,
        text.waiting_service(),
        "",
        StateTone::Muted,
    );
    set_actions_enabled(handles, false);
}

fn show_dashboard_startup_error(handles: &UiHandles, detail: &str) {
    set_status_panel(
        &handles.service_status,
        handles.text.startup_failed(),
        detail,
        StateTone::Error,
    );
    set_im_channel_row(
        &handles.im_status.feishu,
        handles.text.waiting_service(),
        handles.text.service_reads_status(),
        StateTone::Muted,
    );
    set_im_channel_row(
        &handles.im_status.telegram,
        handles.text.waiting_service(),
        handles.text.service_reads_status(),
        StateTone::Muted,
    );
    set_im_channel_row(
        &handles.im_status.wechat,
        handles.text.waiting_service(),
        handles.text.service_reads_status(),
        StateTone::Muted,
    );
    set_actions_enabled(handles, false);
}

fn update_dashboard(handles: &UiHandles, snapshot: &DashboardSnapshot, daemon_starting: bool) {
    let text = handles.text;
    refresh_service_settings_button(handles, snapshot);
    if !snapshot.service_online {
        if daemon_starting {
            show_dashboard_starting(handles);
            return;
        }
        set_status_panel(
            &handles.service_status,
            text.not_running(),
            &text.local_service_offline_detail(snapshot.local_connection_mode),
            StateTone::Error,
        );
        set_im_channel_row(
            &handles.im_status.feishu,
            text.unavailable(),
            text.local_service_not_running(),
            StateTone::Muted,
        );
        set_im_channel_row(
            &handles.im_status.telegram,
            text.unavailable(),
            text.local_service_not_running(),
            StateTone::Muted,
        );
        set_im_channel_row(
            &handles.im_status.wechat,
            text.unavailable(),
            text.local_service_not_running(),
            StateTone::Muted,
        );
        set_disabled_status_panel(
            &handles.codex_status,
            text.unavailable(),
            if CODEX_APP_GUI_UNSUPPORTED {
                text.app_gui_unsupported()
            } else {
                ""
            },
        );
        set_status_panel(
            &handles.vscode_status,
            text.unavailable(),
            "",
            StateTone::Muted,
        );
        set_status_panel(
            &handles.cli_status,
            text.unavailable(),
            "",
            StateTone::Muted,
        );
        set_actions_enabled(handles, false);
        return;
    }

    set_actions_enabled(handles, true);

    if let Some(codex_status) = &snapshot.codex_app {
        codex_tab::refresh_configured(&handles.codex_tab, codex_status.configured);
    } else {
        codex_tab::refresh_configured(&handles.codex_tab, false);
    }
    codex_tab::refresh_local_connection_mode(&handles.codex_tab, snapshot.local_connection_mode);
    if let Some(status) = &snapshot.status {
        codex_tab::refresh_fast_startup(&handles.codex_tab, status.codex_app_fast_startup);
    }
    if let Some(gw) = &snapshot.ai_gateway {
        refresh_ai_gw_filter_image_generation(handles, gw.filter_image_generation_tool);
        refresh_ai_gw_enable_logging(handles, gw.request_logging_enabled);
    }

    if let Some(status) = &snapshot.status {
        set_status_panel(
            &handles.service_status,
            text.running(),
            &text.local_service_detail(
                &local_connection_display_addr(&status.bind, status.local_connection_mode),
                text.local_connection_label(status.local_connection_mode),
            ),
            StateTone::Ok,
        );
    }

    refresh_im_account_list(handles, snapshot);

    let remote_status = snapshot.remote.as_ref();
    let codex_configured = snapshot
        .codex_app
        .as_ref()
        .map(|status| status.configured)
        .unwrap_or(false);
    let codex_app_status_state =
        endpoint_status_state(remote_status, "codex_app", codex_configured);
    let vscode_status_state = endpoint_status_state(remote_status, "vscode", true);
    let cli_status_state = endpoint_status_state(remote_status, "cli", true);
    codex_tab::refresh_remote_ready(
        &handles.codex_tab,
        codex_app_status_state == EndpointStatusState::Connected
            || vscode_status_state == EndpointStatusState::Connected
            || cli_status_state == EndpointStatusState::Connected,
    );

    if CODEX_APP_GUI_UNSUPPORTED {
        set_disabled_status_panel(
            &handles.codex_status,
            text.unavailable(),
            text.app_gui_unsupported(),
        );
    } else {
        set_endpoint_status_panel(&handles.codex_status, text, codex_app_status_state);
    }

    set_endpoint_status_panel(&handles.vscode_status, text, vscode_status_state);
    set_endpoint_status_panel(&handles.cli_status, text, cli_status_state);

    // AI Gateway status
    if let Some(gw) = &snapshot.ai_gateway {
        refresh_ai_gw_provider_list(handles, Some(gw));
        codex_tab::initialize_visible_model_checks(&handles.codex_tab, gw);
    } else {
        refresh_ai_gw_provider_list(handles, None);
    }
}

fn refresh_service_settings_button(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    let should_show = snapshot.local_connection_mode == LocalConnectionMode::VpnCompatible
        || (!snapshot.service_online
            && snapshot.local_connection_mode == LocalConnectionMode::Standard
            && snapshot.compatible_connection_available);
    if should_show {
        handles.service_settings_button.show(true);
    } else {
        handles.service_settings_button.hide();
    }
    handles.service_status.panel.layout();
}

fn set_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.change_bot_button.enable(enabled);
    handles.connect_wechat_button.enable(enabled);
    handles.save_telegram_button.enable(enabled);
    handles.delete_im_account_button.enable(enabled);
    codex_tab::set_actions_enabled(&handles.codex_tab, enabled);
    set_ai_gw_actions_enabled(handles, enabled);
}

fn start_request_log_timer(timer_store: &FrameTimerStore) {
    let store = timer_store.borrow();
    if let Some(timer) = store.as_ref()
        && !timer.is_running()
    {
        timer.start(REQUEST_LOG_REFRESH_INTERVAL_MS, false);
    }
}

fn stop_request_log_timer(timer_store: &FrameTimerStore) {
    let store = timer_store.borrow();
    if let Some(timer) = store.as_ref()
        && timer.is_running()
    {
        timer.stop();
    }
}

fn force_request_log_refresh(
    api: &ApiClient,
    result_store: &RequestLogResultStore,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    let thread_api = api.clone();
    let result_store = result_store.clone();
    let in_flight = in_flight.clone();
    thread::spawn(move || {
        let outcome = thread_api
            .ai_gateway_request_logs()
            .map(|response| response.logs);
        if let Ok(mut slot) = result_store.lock() {
            slot.replace(outcome);
        }
        in_flight.store(false, Ordering::SeqCst);
        wxdragon::wake_up_idle();
    });
}

fn schedule_request_log_refresh(
    api: &ApiClient,
    result_store: &RequestLogResultStore,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.load(Ordering::SeqCst) {
        return;
    }
    force_request_log_refresh(api, result_store, in_flight);
}

fn start_request_log_detail_load(
    api: &ApiClient,
    handles: &UiHandles,
    row: usize,
    result_store: &RequestLogDetailResultStore,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(id) = handles.request_log_rows.borrow().get(row).map(|log| log.id) else {
        in_flight.store(false, Ordering::SeqCst);
        return;
    };
    let thread_api = api.clone();
    let result_store = result_store.clone();
    let in_flight = in_flight.clone();
    thread::spawn(move || {
        let outcome = thread_api
            .ai_gateway_request_log_detail(id)
            .map(|response| response.log);
        if let Ok(mut slot) = result_store.lock() {
            slot.replace((id, outcome));
        }
        in_flight.store(false, Ordering::SeqCst);
        wxdragon::wake_up_idle();
    });
}

fn apply_pending_request_log_detail(
    frame: &Frame,
    handles: &UiHandles,
    result_store: &RequestLogDetailResultStore,
) {
    let result = result_store.lock().ok().and_then(|mut slot| slot.take());
    let Some((_id, result)) = result else {
        return;
    };
    match result {
        Ok(detail) => {
            request_log_detail::show(frame, handles.text, &detail);
        }
        Err(err) => {
            show_error(frame, &handles.text.request_log_detail_failed(&err));
        }
    }
}

fn apply_pending_request_logs(handles: &UiHandles, result_store: &RequestLogResultStore) {
    let result = result_store.lock().ok().and_then(|mut slot| slot.take());
    let Some(result) = result else {
        return;
    };
    match result {
        Ok(logs) => {
            refresh_request_log_list(handles, logs);
        }
        Err(err) => {
            tracing::warn!("failed to load request logs: {err}");
        }
    }
}

fn apply_pending_request_log_clear(
    frame: &Frame,
    text: GuiText,
    clear_old_button: &Button,
    clear_all_button: &Button,
    result_store: &RequestLogClearResultStore,
) -> bool {
    let result = result_store.lock().ok().and_then(|mut slot| slot.take());
    let Some(result) = result else {
        return false;
    };
    clear_old_button.enable(true);
    clear_all_button.enable(true);
    match result {
        Ok(deleted) => {
            show_info(frame, &text.request_log_clear_done(deleted));
            true
        }
        Err(err) => {
            show_error(frame, &text.request_log_clear_failed(&err));
            false
        }
    }
}

fn remote_connection_ready(remote: Option<&RemoteControlStatus>, source_kind: &str) -> bool {
    remote
        .map(|remote| {
            remote.connections.iter().any(|connection| {
                connection.source_kind == source_kind
                    && connection.connected
                    && connection.initialized
            })
        })
        .unwrap_or(false)
}

fn remote_active_ready(remote: Option<&RemoteControlStatus>, source_kind: &str) -> bool {
    remote
        .filter(|remote| remote.connected && remote.initialized)
        .and_then(|remote| remote.active_source_kind.as_deref())
        == Some(source_kind)
}

fn remote_source_initializing(remote: Option<&RemoteControlStatus>, source_kind: &str) -> bool {
    remote
        .map(|remote| {
            remote.connections.iter().any(|connection| {
                connection.source_kind == source_kind
                    && connection.connected
                    && !connection.initialized
            })
        })
        .unwrap_or(false)
}

fn endpoint_status_state(
    remote: Option<&RemoteControlStatus>,
    source_kind: &str,
    configured: bool,
) -> EndpointStatusState {
    if remote.is_none() {
        EndpointStatusState::Loading
    } else if remote_connection_ready(remote, source_kind)
        || remote_active_ready(remote, source_kind)
    {
        EndpointStatusState::Connected
    } else if remote_source_initializing(remote, source_kind) {
        EndpointStatusState::Initializing
    } else if configured {
        EndpointStatusState::NotConnected
    } else {
        EndpointStatusState::UninitializedConfig
    }
}

fn set_endpoint_status_panel(panel: &StatusPanel, text: GuiText, state: EndpointStatusState) {
    let label = endpoint_status_label(text, state);
    let tone = endpoint_status_tone(state);
    set_status_panel(panel, label, "", tone);
}

fn endpoint_status_label(text: GuiText, state: EndpointStatusState) -> &'static str {
    match state {
        EndpointStatusState::Connected => text.connected(),
        EndpointStatusState::Loading => text.reading(),
        EndpointStatusState::Initializing => text.initializing(),
        EndpointStatusState::NotConnected => text.not_connected(),
        EndpointStatusState::UninitializedConfig => text.uninitialized_config(),
    }
}

fn endpoint_status_tone(state: EndpointStatusState) -> StateTone {
    match state {
        EndpointStatusState::Connected => StateTone::Ok,
        EndpointStatusState::Loading => StateTone::Muted,
        EndpointStatusState::Initializing
        | EndpointStatusState::NotConnected
        | EndpointStatusState::UninitializedConfig => StateTone::Warn,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointStatusState {
    Connected,
    Loading,
    Initializing,
    NotConnected,
    UninitializedConfig,
}

fn show_about_dialog(parent: &Frame) {
    let dialog = Dialog::builder(parent, "About CodexHub")
        .with_style(DialogStyle::DefaultDialogStyle)
        .with_size(520, 260)
        .build();
    dialog.set_icon(&app_icon_bitmap(48));
    dialog.set_background_color(theme::theme().bg_card);

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(theme::theme().bg_card);
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label(&format!("CodexHub {}", env!("CARGO_PKG_VERSION")))
        .build();
    title.set_foreground_color(theme::theme().ink_primary);
    title.set_font(&theme::font(theme::TextRole::Title));
    sizer.add(
        &title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let description = StaticText::builder(&panel)
        .with_label(&GuiText::new(load_gui_locale()).about_description())
        .build();
    description.set_foreground_color(theme::theme().ink_secondary);
    description.wrap(460);
    sizer.add(
        &description,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let link = HyperlinkCtrl::builder(&panel)
        .with_label(PROJECT_HOME_URL)
        .with_url(PROJECT_HOME_URL)
        .build();
    sizer.add(
        &link,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&panel)
        .with_label(GuiText::new(load_gui_locale()).close())
        .build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 0, SizerFlag::AlignLeft, 0);
    sizer.add_sizer(&buttons, 0, SizerFlag::Expand | SizerFlag::All, 18);

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    {
        let dialog = dialog;
        close_button.on_click(move |_| dialog.end_modal(ID_OK));
    }

    dialog.show_modal();
    dialog.destroy();
}

fn show_info(parent: &dyn WxWidget, message: &str) {
    MessageDialog::builder(parent, message, "CodexHub")
        .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
        .build()
        .show_modal();
}

fn show_error(parent: &dyn WxWidget, message: &str) {
    MessageDialog::builder(parent, message, "CodexHub")
        .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
        .build()
        .show_modal();
}

fn confirm_open_update_release(parent: &dyn WxWidget, text: GuiText, message: &str) -> bool {
    MessageDialog::builder(parent, message, text.update_dialog_title())
        .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
        .build()
        .show_modal()
        == ID_YES
}

fn confirm_switch_compatible_connection(parent: &dyn WxWidget, text: GuiText) -> bool {
    MessageDialog::builder(
        parent,
        text.local_connection_detected_message(),
        text.local_connection_detected_title(),
    )
    .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
    .build()
    .show_modal()
        == ID_YES
}

fn confirm_uninstall_codex_app_config(parent: &dyn WxWidget, text: GuiText) -> bool {
    MessageDialog::builder(
        parent,
        text.confirm_uninstall_codex_app_message(),
        text.confirm_uninstall_codex_app_title(),
    )
    .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
    .build()
    .show_modal()
        == ID_YES
}

fn confirm_delete_im_account(parent: &dyn WxWidget, text: GuiText, account_name: &str) -> bool {
    MessageDialog::builder(
        parent,
        &text.confirm_delete_im_account_message(account_name),
        text.confirm_delete_im_account_title(),
    )
    .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
    .build()
    .show_modal()
        == ID_YES
}

fn confirm_clear_old_request_logs(parent: &dyn WxWidget, text: GuiText) -> bool {
    MessageDialog::builder(
        parent,
        text.request_log_clear_old_confirm_message(),
        text.request_log_clear_old_confirm_title(),
    )
    .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
    .build()
    .show_modal()
        == ID_YES
}

fn confirm_clear_all_request_logs(parent: &dyn WxWidget, text: GuiText) -> bool {
    MessageDialog::builder(
        parent,
        text.request_log_clear_all_confirm_message(),
        text.request_log_clear_all_confirm_title(),
    )
    .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
    .build()
    .show_modal()
        == ID_YES
}
