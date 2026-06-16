use std::{
    cell::RefCell,
    process::Child,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use wxdragon::widgets::dataview::{
    CustomDataViewVirtualListModel, DataViewAlign, DataViewColumnFlags, DataViewCtrl,
    DataViewItemAttr, DataViewStyle, Variant,
};
use wxdragon::widgets::scrolled_window::ScrollBarConfig;
use wxdragon::{prelude::*, timer::Timer};

use crate::ai_gateway::config::{
    ProviderConfig, ProviderType, provider_api_root, provider_display_base_url,
};
use crate::config::AppConfig;

#[cfg(target_os = "windows")]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
#[cfg(not(target_os = "windows"))]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
const CODEX_APP_GUI_UNSUPPORTED: bool = !(cfg!(target_os = "macos") || cfg!(target_os = "windows"));
const PROJECT_HOME_URL: &str = "https://github.com/happy-loki/codex-remote";
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/happy-loki/codex-remote/releases/latest/download/latest.json";
const UPDATE_RELEASE_API_URL: &str =
    "https://api.github.com/repos/happy-loki/codex-remote/releases/latest";
const UPDATE_RELEASE_PAGE_URL: &str = "https://github.com/happy-loki/codex-remote/releases/latest";
const DASHBOARD_REFRESH_INTERVAL_MS: i32 = 2500;
const DASHBOARD_RESULT_POLL_MS: i32 = 100;
const GUI_CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const GUI_STATUS_TIMEOUT: Duration = Duration::from_millis(650);
const GUI_ACTION_TIMEOUT: Duration = Duration::from_secs(2);
const GUI_CONFIG_TIMEOUT: Duration = Duration::from_secs(15);
const GUI_STARTUP_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const ID_MENU_CLOSE_WINDOW: i32 = 10_001;
const ID_MENU_MINIMIZE: i32 = 10_002;
const ID_MENU_CHECK_UPDATE: i32 = 10_003;
const ID_MENU_LANGUAGE_ZH_CN: i32 = 10_004;
const ID_MENU_LANGUAGE_EN_US: i32 = 10_005;

type ImAccountRows = Rc<RefCell<Vec<[String; 5]>>>;
type ImAccountModel = Rc<RefCell<CustomDataViewVirtualListModel>>;
type PendingImToggle = Rc<RefCell<Option<(String, String, bool)>>>;

type FrameTimerStore = Rc<RefCell<Option<Timer<Frame>>>>;
type CodexActionResultStore = Arc<Mutex<Option<CodexActionResult>>>;
type ImActionResultStore = Arc<Mutex<Option<ImActionResult>>>;

mod ai_gateway;
mod api;
mod daemon;
mod im_accounts;
mod onboarding;
mod provider;
mod text;
mod update;
mod widgets;

use self::ai_gateway::{
    AiGwActionResult, AiGwActionResultStore, AiGwProviderModel, AiGwProviderRow, AiGwProviderRows,
    PendingAiGwChannelToggle, apply_pending_ai_gw_action, delete_ai_gw_provider,
    provider_logo_variant, provider_type_display, refresh_ai_gw_provider_list, save_ai_gw_provider,
    set_ai_gw_actions_enabled, set_ai_gw_provider_enabled,
};
use self::api::{
    ApiClient, ConfigureRequest, ConfigureTelegramBotRequest, DashboardSnapshot,
    DeleteImAccountRequest, RemoteControlStatus, SetImAccountEnabledRequest,
};
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
use self::text::{GuiLocale, GuiText};
use self::widgets::{
    ImStatusPanel, ProviderLogoKind, StateTone, StatusIconKind, StatusPanel, app_icon_bitmap,
    centered_status_panel, im_status_panel, provider_logo_bitmap, set_disabled_status_panel,
    set_im_channel_row, set_status_panel, status_panel, text_field_row, topology_connector,
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

pub fn run() {
    if let Err(err) = wxdragon::main(|_| build_ui()) {
        eprintln!("failed to start Codex Remote GUI: {err:?}");
    }
}

fn build_ui() {
    let locale = load_gui_locale();
    let text = GuiText::new(locale);
    let api = ApiClient::new(default_base_url(), text);
    let gui_timers = GuiTimers::new();

    let frame = Frame::builder()
        .with_title("Codex Remote")
        .with_size(Size::new(1280, 760))
        .build();
    frame.set_icon(&app_icon_bitmap(48));
    install_system_menu(&frame, &gui_timers, text);
    frame.set_background_color(Colour::rgb(246, 247, 250));
    let _status_bar = StatusBar::builder(&frame)
        .with_fields_count(1)
        .with_status_widths(vec![-1])
        .add_initial_text(0, &text.version())
        .build();

    let root = Panel::builder(&frame).build();
    root.set_background_color(Colour::rgb(246, 247, 250));

    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let status_box = StaticBox::builder(&root)
        .with_label(text.status_overview())
        .build();
    let status_section =
        StaticBoxSizerBuilder::new_with_box(&status_box, Orientation::Vertical).build();
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
    let im_status = im_status_panel(&status_box, text);
    let entry_connector = topology_connector(&status_box);
    let bridge_connector = topology_splitter(&status_box);
    let entry_column = BoxSizer::builder(Orientation::Vertical).build();
    entry_column.add(
        &codex_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::Bottom,
        8,
    );
    entry_column.add(
        &vscode_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::Bottom,
        8,
    );
    entry_column.add(&cli_status.panel, 1, SizerFlag::Expand, 0);
    status_row.add_sizer(&entry_column, 1, SizerFlag::Expand | SizerFlag::All, 8);
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
        8,
    );
    status_row.add(
        &bridge_connector,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Left | SizerFlag::Right,
        2,
    );
    status_row.add(&im_status.panel, 1, SizerFlag::Expand | SizerFlag::All, 8);
    status_section.add_sizer(
        &status_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        8,
    );
    let ai_gw_status_label = StaticText::builder(&status_box)
        .with_label(&text.ai_gw_status_enabled(0))
        .build();
    ai_gw_status_label.set_foreground_color(Colour::rgb(103, 111, 124));
    status_section.add(
        &ai_gw_status_label,
        0,
        SizerFlag::Left | SizerFlag::Bottom,
        8,
    );
    root_sizer.add_sizer(
        &status_section,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );

    let notebook = Notebook::builder(&root).build();

    let codex_page = ScrolledWindow::builder(&notebook)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    codex_page.set_background_color(Colour::rgb(250, 251, 253));
    let codex_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let provider_image_generation = CheckBox::builder(&codex_page)
        .with_label(text.image_generation_feature())
        .with_value(false)
        .build();
    provider_image_generation.set_tooltip(text.image_generation_feature_help());
    let provider_image_generation_note = StaticText::builder(&codex_page)
        .with_label(text.image_generation_feature_note())
        .build();
    provider_image_generation_note.set_foreground_color(Colour::rgb(103, 111, 124));
    let provider_image_generation_row = BoxSizer::builder(Orientation::Horizontal).build();
    provider_image_generation_row.add(
        &provider_image_generation,
        0,
        SizerFlag::Right | SizerFlag::AlignCenterVertical,
        8,
    );
    provider_image_generation_row.add(
        &provider_image_generation_note,
        0,
        SizerFlag::AlignCenterVertical,
        0,
    );
    codex_sizer.add_sizer(
        &provider_image_generation_row,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        12,
    );

    let inject_codex_button = Button::builder(&codex_page)
        .with_label(text.inject_codex_access())
        .build();
    inject_codex_button.set_tooltip(text.inject_codex_access_help());
    let uninstall_button = Button::builder(&codex_page)
        .with_label(text.clear_codex_access())
        .build();
    uninstall_button.set_tooltip(text.clear_codex_access_help());
    let codex_maintenance_actions = BoxSizer::builder(Orientation::Horizontal).build();
    codex_maintenance_actions.add_stretch_spacer(1);
    codex_maintenance_actions.add(&inject_codex_button, 0, SizerFlag::Right, 8);
    codex_maintenance_actions.add(&uninstall_button, 0, SizerFlag::Right, 0);
    codex_sizer.add_stretch_spacer(1);
    codex_sizer.add_sizer(
        &codex_maintenance_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        20,
    );
    codex_page.set_sizer(codex_sizer, true);
    codex_page.set_scroll_rate(10, 10);
    let codex_best_size = codex_page.get_best_size();
    codex_page.set_scrollbars(ScrollBarConfig {
        pixels_per_unit_x: 10,
        pixels_per_unit_y: 10,
        no_units_x: (codex_best_size.width + 20).max(1) / 10,
        no_units_y: (codex_best_size.height + 80).max(1) / 10,
        x_pos: 0,
        y_pos: 0,
        no_refresh: true,
    });

    // --- AI Gateway Tab ---
    let ai_gw_page = ScrolledWindow::builder(&notebook)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    ai_gw_page.set_background_color(Colour::rgb(250, 251, 253));
    let ai_gw_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let ai_gw_list_box = StaticBox::builder(&ai_gw_page)
        .with_label(text.ai_gw_channel_list())
        .build();
    let ai_gw_list_section =
        StaticBoxSizerBuilder::new_with_box(&ai_gw_list_box, Orientation::Vertical).build();

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
                    2 => provider_logo_variant(&row_data.provider_type),
                    3 => provider_type_display(&row_data.provider_type).into(),
                    4 => row_data.base_url.clone().into(),
                    5 => row_data.timeout_secs.to_string().into(),
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
                    pending_ai_gw_channel_toggle_for_model
                        .borrow_mut()
                        .replace((name, enabled));
                    false
                },
            ),
            None::<fn(&AiGwProviderRows, usize, usize) -> Option<DataViewItemAttr>>,
            None::<fn(&AiGwProviderRows, usize, usize) -> bool>,
        )));
    let ai_gw_provider_list = DataViewCtrl::builder(&ai_gw_list_box)
        .with_style(
            DataViewStyle::Single | DataViewStyle::RowLines | DataViewStyle::HorizontalRules,
        )
        .with_size(Size::new(-1, 330))
        .build();
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
        text.ai_gw_timeout(),
        5,
        80,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    ai_gw_provider_list.associate_model(&*ai_gw_provider_model.borrow());
    ai_gw_list_section.add(
        &ai_gw_provider_list,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );

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
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );

    ai_gw_sizer.add_sizer(
        &ai_gw_list_section,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );
    ai_gw_sizer.add_stretch_spacer(1);
    ai_gw_page.set_sizer(ai_gw_sizer, true);
    ai_gw_page.set_scroll_rate(10, 10);
    let ai_gw_best_size = ai_gw_page.get_best_size();
    ai_gw_page.set_scrollbars(ScrollBarConfig {
        pixels_per_unit_x: 10,
        pixels_per_unit_y: 10,
        no_units_x: (ai_gw_best_size.width + 20).max(1) / 10,
        no_units_y: (ai_gw_best_size.height + 80).max(1) / 10,
        x_pos: 0,
        y_pos: 0,
        no_refresh: true,
    });

    let feishu_page = ScrolledWindow::builder(&notebook)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    feishu_page.set_background_color(Colour::rgb(250, 251, 253));
    let feishu_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let im_access_hint = StaticText::builder(&feishu_page)
        .with_label(text.im_access_hint())
        .build();
    im_access_hint.set_foreground_color(Colour::rgb(64, 72, 86));
    im_access_hint.wrap(1180);
    feishu_sizer.add(
        &im_access_hint,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let im_accounts_static_box = StaticBox::builder(&feishu_page)
        .with_label(text.bot_pool())
        .build();
    let im_accounts_box =
        StaticBoxSizerBuilder::new_with_box(&im_accounts_static_box, Orientation::Vertical).build();
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
                    pending_im_toggle_for_model
                        .borrow_mut()
                        .replace((platform, account_id, enabled));
                    false
                },
            ),
            None::<fn(&ImAccountRows, usize, usize) -> Option<DataViewItemAttr>>,
            None::<fn(&ImAccountRows, usize, usize) -> bool>,
        )));
    let im_account_list = DataViewCtrl::builder(&im_accounts_static_box)
        .with_style(
            DataViewStyle::Single | DataViewStyle::RowLines | DataViewStyle::HorizontalRules,
        )
        .with_size(Size::new(-1, 190))
        .build();
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
    let add_im_static_box = StaticBox::builder(&feishu_page)
        .with_label(text.add_bot())
        .build();
    let add_im_box =
        StaticBoxSizerBuilder::new_with_box(&add_im_static_box, Orientation::Vertical).build();
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
    let wechat_context_warning = StaticText::builder(&add_im_static_box)
        .with_label(text.wechat_context_token_warning())
        .build();
    wechat_context_warning.set_foreground_color(Colour::rgb(210, 36, 36));
    wechat_context_warning.wrap(620);
    add_im_actions.add(
        &wechat_context_warning,
        0,
        SizerFlag::Left | SizerFlag::AlignCenterVertical,
        24,
    );
    add_im_box.add_sizer(
        &add_im_actions,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        12,
    );
    feishu_sizer.add_sizer(&add_im_box, 0, SizerFlag::Expand | SizerFlag::All, 8);
    feishu_sizer.add_sizer(&im_accounts_box, 0, SizerFlag::Expand | SizerFlag::All, 8);
    feishu_sizer.add_stretch_spacer(1);
    feishu_page.set_sizer(feishu_sizer, true);
    let feishu_best_size = feishu_page.get_best_size();
    feishu_page.set_scrollbars(ScrollBarConfig {
        pixels_per_unit_x: 10,
        pixels_per_unit_y: 10,
        no_units_x: (feishu_best_size.width + 20).max(1) / 10,
        no_units_y: (feishu_best_size.height + 80).max(1) / 10,
        x_pos: 0,
        y_pos: 0,
        no_refresh: true,
    });

    notebook.add_page(&codex_page, text.codex_tab(), true, None);
    notebook.add_page(&ai_gw_page, text.ai_gateway_tab(), false, None);
    notebook.add_page(&feishu_page, text.chat_tab(), false, None);

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
        text,
        service_status,
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
        inject_codex_button,
        uninstall_button,
        provider_image_generation,
        ai_gw_provider_list,
        ai_gw_provider_rows,
        ai_gw_provider_model,
        pending_ai_gw_channel_toggle,
        ai_gw_delete_button,
        ai_gw_new_button,
        ai_gw_edit_button,
        ai_gw_status_label,
    };

    let daemon_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    let dashboard_refresh = DashboardRefresh::new();
    show_dashboard_starting(&handles);

    let codex_action_result: CodexActionResultStore = Arc::new(Mutex::new(None));
    let codex_action_in_flight = Arc::new(AtomicBool::new(false));

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let codex_action_result = codex_action_result.clone();
        let codex_action_in_flight = codex_action_in_flight.clone();
        inject_codex_button.on_click(move |_| {
            if codex_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                codex_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            handles
                .inject_codex_button
                .set_label(handles.text.injecting_codex_access());
            handles.inject_codex_button.enable(false);
            let request = ConfigureRequest {
                provider_name: None,
                provider_base_url: None,
                provider_key: None,
                activate: true,
                image_generation_enabled: Some(handles.provider_image_generation.get_value()),
                supports_websockets: false,
            };
            let thread_api = api.clone();
            let codex_action_result = codex_action_result.clone();
            let codex_action_in_flight = codex_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api.configure_codex_app(&request);
                if let Ok(mut slot) = codex_action_result.lock() {
                    slot.replace(CodexActionResult::Inject(outcome));
                }
                codex_action_in_flight.store(false, Ordering::SeqCst);
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let codex_action_result = codex_action_result.clone();
        let codex_action_in_flight = codex_action_in_flight.clone();
        uninstall_button.on_click(move |_| {
            if codex_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                codex_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            if !confirm_uninstall_codex_app_config(&frame, handles.text) {
                codex_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            handles
                .uninstall_button
                .set_label(handles.text.clearing_codex_access());
            handles.uninstall_button.enable(false);
            let thread_api = api.clone();
            let codex_action_result = codex_action_result.clone();
            let codex_action_in_flight = codex_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api.uninstall_codex_app();
                if let Ok(mut slot) = codex_action_result.lock() {
                    slot.replace(CodexActionResult::Clear(outcome));
                }
                codex_action_in_flight.store(false, Ordering::SeqCst);
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

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

    let im_action_result: ImActionResultStore = Arc::new(Mutex::new(None));
    let im_action_in_flight = Arc::new(AtomicBool::new(false));

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let im_action_result = im_action_result.clone();
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
            let im_action_result = im_action_result.clone();
            let im_action_in_flight = im_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api.delete_im_account(&request);
                if let Ok(mut slot) = im_action_result.lock() {
                    slot.replace(ImActionResult::AccountDelete(outcome));
                }
                im_action_in_flight.store(false, Ordering::SeqCst);
            });
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let im_action_result = im_action_result.clone();
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
            let im_action_result = im_action_result.clone();
            let im_action_in_flight = im_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = thread_api.configure_telegram_bot(&request);
                if let Ok(mut slot) = im_action_result.lock() {
                    slot.replace(ImActionResult::TelegramConfigure(outcome));
                }
                im_action_in_flight.store(false, Ordering::SeqCst);
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

    let result_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let result_timer = Timer::new(&frame);
    {
        let handles = handles.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let api = api.clone();
        let frame = frame;
        let codex_action_result = codex_action_result.clone();
        result_timer.on_tick(move |_| {
            apply_pending_dashboard(&handles, &dashboard_refresh);
            apply_pending_codex_action(
                &api,
                &handles,
                &frame,
                &dashboard_refresh,
                &codex_action_result,
            );
        });
    }
    result_timer.start(DASHBOARD_RESULT_POLL_MS, false);
    result_timer_store.borrow_mut().replace(result_timer);
    gui_timers.track(&result_timer_store);

    let im_action_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let im_action_timer = Timer::new(&frame);
    {
        let api = api.clone();
        let handles = handles.clone();
        let frame = frame;
        let dashboard_refresh = dashboard_refresh.clone();
        let im_action_result = im_action_result.clone();
        let im_action_in_flight = im_action_in_flight.clone();
        im_action_timer.on_tick(move |_| {
            if !im_action_in_flight.load(Ordering::SeqCst)
                && let Some((platform, account_id, enabled)) =
                    handles.pending_im_toggle.borrow_mut().take()
            {
                if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                    force_dashboard_refresh(&api, &dashboard_refresh);
                    return;
                }
                im_action_in_flight.store(true, Ordering::SeqCst);
                set_actions_enabled(&handles, false);
                let request = SetImAccountEnabledRequest {
                    platform,
                    account_id,
                    enabled,
                };
                let thread_api = api.clone();
                let im_action_result = im_action_result.clone();
                let im_action_in_flight = im_action_in_flight.clone();
                thread::spawn(move || {
                    let outcome = thread_api.set_im_account_enabled(&request);
                    if let Ok(mut slot) = im_action_result.lock() {
                        slot.replace(ImActionResult::AccountToggle(outcome));
                    }
                    im_action_in_flight.store(false, Ordering::SeqCst);
                });
            }
            apply_pending_im_action(
                &api,
                &handles,
                &frame,
                &dashboard_refresh,
                &im_action_result,
            );
        });
    }
    im_action_timer.start(DASHBOARD_RESULT_POLL_MS, false);
    im_action_timer_store.borrow_mut().replace(im_action_timer);
    gui_timers.track(&im_action_timer_store);

    // --- AI Gateway event handlers ---
    let ai_gw_action_result: AiGwActionResultStore = Arc::new(Mutex::new(None));
    let ai_gw_action_in_flight = Arc::new(AtomicBool::new(false));

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        let ai_gw_action_result = ai_gw_action_result.clone();
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
                    &ai_gw_action_result,
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
        let ai_gw_action_result = ai_gw_action_result.clone();
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
                    &ai_gw_action_result,
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
        let ai_gw_action_result = ai_gw_action_result.clone();
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
            let ai_gw_action_result = ai_gw_action_result.clone();
            let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = delete_ai_gw_provider(&worker_api, &name);
                if let Ok(mut slot) = ai_gw_action_result.lock() {
                    slot.replace(AiGwActionResult::Delete(outcome));
                }
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
        let ai_gw_action_result = ai_gw_action_result.clone();
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
                    &ai_gw_action_result,
                    &ai_gw_action_in_flight,
                    provider,
                );
            }
        });
    }

    let ai_gw_action_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let ai_gw_action_timer = Timer::new(&frame);
    {
        let handles = handles.clone();
        let frame = frame;
        let dashboard_refresh = dashboard_refresh.clone();
        let api = api.clone();
        let ai_gw_action_result = ai_gw_action_result.clone();
        let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
        ai_gw_action_timer.on_tick(move |_| {
            if !ai_gw_action_in_flight.load(Ordering::SeqCst)
                && let Some((name, enabled)) =
                    handles.pending_ai_gw_channel_toggle.borrow_mut().take()
            {
                if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                    force_dashboard_refresh(&api, &dashboard_refresh);
                    return;
                }
                ai_gw_action_in_flight.store(true, Ordering::SeqCst);
                set_ai_gw_actions_enabled(&handles, false);
                let worker_api = api.clone();
                let ai_gw_action_result = ai_gw_action_result.clone();
                let ai_gw_action_in_flight = ai_gw_action_in_flight.clone();
                thread::spawn(move || {
                    let outcome = set_ai_gw_provider_enabled(&worker_api, &name, enabled);
                    if let Ok(mut slot) = ai_gw_action_result.lock() {
                        slot.replace(AiGwActionResult::ChannelToggle(outcome));
                    }
                    ai_gw_action_in_flight.store(false, Ordering::SeqCst);
                });
            }
            if apply_pending_ai_gw_action(&handles, &frame, &ai_gw_action_result) {
                force_dashboard_refresh(&api, &dashboard_refresh);
            }
        });
    }
    ai_gw_action_timer.start(DASHBOARD_RESULT_POLL_MS, false);
    ai_gw_action_timer_store
        .borrow_mut()
        .replace(ai_gw_action_timer);
    gui_timers.track(&ai_gw_action_timer_store);

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
        frame.on_close(move |_| {
            dashboard_refresh.closing.store(true, Ordering::SeqCst);
            gui_timers.stop_all();
            stop_pending_startup_daemon(&dashboard_refresh);
            stop_daemon_on_exit(&api, &daemon_child);
            frame.destroy();
        });
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

fn install_system_menu(frame: &Frame, gui_timers: &GuiTimers, text: GuiText) {
    let file_menu = Menu::builder()
        .append_item(
            ID_MENU_CLOSE_WINDOW,
            text.close_window(),
            text.close_window_help(),
        )
        .append_item(ID_MENU_MINIMIZE, text.minimize(), text.minimize_help())
        .append_separator()
        .append_item(ID_EXIT, text.quit(), "Quit Codex Remote")
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
    let help_menu = Menu::builder()
        .append_item(
            ID_MENU_CHECK_UPDATE,
            text.check_updates(),
            text.check_updates_help(),
        )
        .append_separator()
        .append_item(ID_ABOUT, text.about(), "About Codex Remote")
        .build();
    let menu_bar = MenuBar::builder()
        .append(file_menu, text.file_menu())
        .append(language_menu, text.language_menu())
        .append(help_menu, text.help_menu())
        .build();
    frame.set_menu_bar(menu_bar);

    let frame = *frame;
    let gui_timers = gui_timers.clone();
    let update_check_in_flight = Arc::new(AtomicBool::new(false));
    frame.on_menu_selected(move |event| match event.get_id() {
        ID_EXIT | ID_MENU_CLOSE_WINDOW => frame.close(true),
        ID_MENU_MINIMIZE => frame.iconize(true),
        ID_MENU_CHECK_UPDATE => {
            update::check_for_updates_async(&frame, &gui_timers, text, &update_check_in_flight);
        }
        ID_MENU_LANGUAGE_ZH_CN => {
            handle_language_selected(&frame, text, GuiLocale::ZhCn);
        }
        ID_MENU_LANGUAGE_EN_US => {
            handle_language_selected(&frame, text, GuiLocale::EnUs);
        }
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
    let row_panel = Panel::builder(parent)
        .with_style(PanelStyle::BorderStatic)
        .build();
    row_panel.set_background_color(Colour::rgb(250, 251, 253));
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
    result_store: &AiGwActionResultStore,
    in_flight: &Arc<AtomicBool>,
    provider: ProviderConfig,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    set_ai_gw_actions_enabled(handles, false);

    let worker_api = api.clone();
    let result_store = result_store.clone();
    let in_flight = in_flight.clone();
    thread::spawn(move || {
        let outcome = save_ai_gw_provider(&worker_api, provider);
        if let Ok(mut slot) = result_store.lock() {
            slot.replace(AiGwActionResult::Save(outcome));
        }
        in_flight.store(false, Ordering::SeqCst);
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
    dialog.set_background_color(Colour::rgb(250, 251, 253));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(250, 251, 253));
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label(text.ai_gw_channel_editor())
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    root.add(
        &title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let help = StaticText::builder(&panel)
        .with_label(text.ai_gw_channel_editor_help())
        .build();
    help.set_foreground_color(Colour::rgb(103, 111, 124));
    root.add(
        &help,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let workspace = BoxSizer::builder(Orientation::Horizontal).build();

    let service_panel = Panel::builder(&panel)
        .with_style(PanelStyle::BorderStatic)
        .build();
    service_panel.set_background_color(Colour::rgb(255, 255, 255));
    service_panel.set_min_size(Size::new(300, 500));
    let service_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let service_title = StaticText::builder(&service_panel)
        .with_label(text.ai_gw_provider_service())
        .build();
    service_title.set_foreground_color(Colour::rgb(21, 25, 31));
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
    service_sizer.add_stretch_spacer(1);
    service_panel.set_sizer(service_sizer, true);
    workspace.add(&service_panel, 0, SizerFlag::Expand | SizerFlag::Right, 14);

    let form_panel = Panel::builder(&panel)
        .with_style(PanelStyle::BorderStatic)
        .build();
    form_panel.set_background_color(Colour::rgb(255, 255, 255));
    form_panel.set_min_size(Size::new(620, 500));
    let form_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let form_title = StaticText::builder(&form_panel)
        .with_label(text.ai_gw_channel_settings())
        .build();
    form_title.set_foreground_color(Colour::rgb(21, 25, 31));
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
    let key_input = text_field_row(&form_panel, &grid, text.ai_gw_col_api_key(), "");
    let timeout_input = text_field_row(&form_panel, &grid, text.ai_gw_timeout(), "60");

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
    model_title.set_foreground_color(Colour::rgb(78, 86, 98));
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

    let models_list = ListBox::builder(&form_panel)
        .with_style(
            ListBoxStyle::Default
                | ListBoxStyle::AlwaysScrollbar
                | ListBoxStyle::HorizontalScrollbar,
        )
        .with_size(Size::new(-1, 170))
        .build();
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
        &type_input,
        &name_input,
        &base_url_input,
        &models_list,
        &timeout_input,
    );
    if let Some(provider) = initial {
        key_input.change_value(&provider.api_key);
        radio_openai.enable(false);
        radio_deepseek.enable(false);
    }

    let service_template_applying = Rc::new(RefCell::new(false));
    if initial.is_none() {
        let type_input = type_input;
        let name_input = name_input;
        let base_url_input = base_url_input;
        let key_input = key_input;
        let models_list = models_list;
        let service_template_applying = service_template_applying.clone();
        radio_openai.on_selected(move |_| {
            if radio_openai.get_value() && !*service_template_applying.borrow() {
                apply_ai_gw_service_template(
                    text,
                    ProviderType::OpenAiResponses,
                    &radio_openai,
                    &radio_deepseek,
                    &type_input,
                    &name_input,
                    &base_url_input,
                    &key_input,
                    &models_list,
                    &timeout_input,
                    &service_template_applying,
                );
            }
        });
    }
    if initial.is_none() {
        let type_input = type_input;
        let name_input = name_input;
        let base_url_input = base_url_input;
        let key_input = key_input;
        let models_list = models_list;
        let service_template_applying = service_template_applying.clone();
        radio_deepseek.on_selected(move |_| {
            if radio_deepseek.get_value() && !*service_template_applying.borrow() {
                apply_ai_gw_service_template(
                    text,
                    ProviderType::ChatCompletions,
                    &radio_openai,
                    &radio_deepseek,
                    &type_input,
                    &name_input,
                    &base_url_input,
                    &key_input,
                    &models_list,
                    &timeout_input,
                    &service_template_applying,
                );
            }
        });
    }
    {
        let models_list = models_list;
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
            append_models_to_list(&models_list, models);
        });
    }
    {
        let models_list = models_list;
        delete_model_button.on_click(move |_| {
            let Some(index) = models_list.get_selection() else {
                show_error(&dialog, text.ai_gw_select_model());
                return;
            };
            models_list.delete(index);
            let count = models_list.get_count();
            if count > 0 {
                models_list.set_selection(index.min(count - 1), true);
            }
        });
    }
    {
        let base_url_input = base_url_input;
        let key_input = key_input;
        let timeout_input = timeout_input;
        let models_list = models_list;
        fetch_models_button.on_click(move |_| {
            let base_url = strip_nul(&base_url_input.get_value()).trim().to_string();
            if base_url.is_empty() {
                show_error(&dialog, text.ai_gw_base_url_empty());
                return;
            }

            fetch_models_button.enable(false);
            fetch_models_button.set_label(text.ai_gw_fetching_models());
            let api_key = strip_nul(&key_input.get_value()).trim().to_string();
            let timeout_secs = strip_nul(&timeout_input.get_value())
                .trim()
                .parse::<u64>()
                .unwrap_or(60);
            match fetch_remote_models(&base_url, &api_key, timeout_secs) {
                Ok((models, normalized_base_url)) => {
                    if models.is_empty() {
                        show_error(&dialog, text.ai_gw_models_empty());
                    } else {
                        base_url_input.change_value(&normalized_base_url);
                        models_list.clear();
                        let count = append_models_to_list(&models_list, models);
                        show_info(&dialog, &text.ai_gw_models_fetched(count));
                    }
                }
                Err(err) => show_error(&dialog, &text.ai_gw_models_fetch_failed(&err)),
            }
            fetch_models_button.set_label(text.ai_gw_fetch_models());
            fetch_models_button.enable(true);
        });
    }
    {
        let dialog = dialog;
        cancel_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
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
            let provider_type = selected_ai_gw_dialog_provider_type(&radio_deepseek);
            let models = list_box_models(&models_list);
            let timeout_secs = strip_nul(&timeout_input.get_value())
                .trim()
                .parse::<u64>()
                .unwrap_or(60);
            Some(ProviderConfig {
                name,
                enabled: initial.map(|provider| provider.enabled).unwrap_or(true),
                provider_type,
                base_url: strip_nul(&base_url_input.get_value()).trim().to_string(),
                api_key: strip_nul(&key_input.get_value()).trim().to_string(),
                models,
                prompt_cache_retention: initial
                    .and_then(|provider| provider.prompt_cache_retention.clone()),
                timeout_secs,
            })
        }
    } else {
        None
    };

    dialog.destroy();
    provider
}

fn apply_ai_gw_dialog_template(
    text: GuiText,
    provider: Option<&ProviderConfig>,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    type_input: &TextCtrl,
    name_input: &TextCtrl,
    base_url_input: &TextCtrl,
    models_list: &ListBox,
    timeout_input: &TextCtrl,
) {
    let provider = provider.cloned().unwrap_or_else(|| ProviderConfig {
        name: "openai".to_string(),
        provider_type: ProviderType::OpenAiResponses,
        base_url: "https://api.openai.com/v1".to_string(),
        timeout_secs: 60,
        ..Default::default()
    });

    set_ai_gw_dialog_provider_type(
        text,
        provider.provider_type.clone(),
        radio_openai,
        radio_deepseek,
        type_input,
    );
    name_input.change_value(&provider.name);
    base_url_input.change_value(&provider.base_url);
    replace_model_list(models_list, &provider.models);
    timeout_input.change_value(&provider.timeout_secs.to_string());
}

fn apply_ai_gw_service_template(
    text: GuiText,
    provider_type: ProviderType,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    type_input: &TextCtrl,
    name_input: &TextCtrl,
    base_url_input: &TextCtrl,
    key_input: &TextCtrl,
    models_list: &ListBox,
    timeout_input: &TextCtrl,
    service_template_applying: &Rc<RefCell<bool>>,
) {
    *service_template_applying.borrow_mut() = true;
    let provider = default_ai_gw_service_provider(provider_type);
    apply_ai_gw_dialog_template(
        text,
        Some(&provider),
        radio_openai,
        radio_deepseek,
        type_input,
        name_input,
        base_url_input,
        models_list,
        timeout_input,
    );
    key_input.change_value("");
    *service_template_applying.borrow_mut() = false;
}

fn default_ai_gw_service_provider(provider_type: ProviderType) -> ProviderConfig {
    match provider_type {
        ProviderType::OpenAiResponses => ProviderConfig {
            name: "openai".to_string(),
            provider_type: ProviderType::OpenAiResponses,
            base_url: "https://api.openai.com/v1".to_string(),
            timeout_secs: 60,
            ..Default::default()
        },
        ProviderType::ChatCompletions => ProviderConfig {
            name: "deepseek".to_string(),
            provider_type: ProviderType::ChatCompletions,
            base_url: "https://api.deepseek.com/v1".to_string(),
            timeout_secs: 60,
            ..Default::default()
        },
    }
}

fn set_ai_gw_dialog_provider_type(
    text: GuiText,
    provider_type: ProviderType,
    radio_openai: &RadioButton,
    radio_deepseek: &RadioButton,
    type_input: &TextCtrl,
) {
    radio_openai.set_value(false);
    radio_deepseek.set_value(false);
    match provider_type {
        ProviderType::ChatCompletions => {
            radio_deepseek.set_value(true);
            type_input.change_value(text.provider_type_chat_completions());
        }
        ProviderType::OpenAiResponses => {
            radio_openai.set_value(true);
            type_input.change_value(text.provider_type_openai_responses());
        }
    }
}

fn selected_ai_gw_dialog_provider_type(radio_deepseek: &RadioButton) -> ProviderType {
    if radio_deepseek.get_value() {
        ProviderType::ChatCompletions
    } else {
        ProviderType::OpenAiResponses
    }
}

fn replace_model_list(list: &ListBox, models: &[String]) {
    list.clear();
    append_models_to_list(list, models.iter().cloned());
}

fn append_models_to_list(list: &ListBox, models: impl IntoIterator<Item = String>) -> usize {
    let mut added = 0;
    for model in models {
        let model = model.trim();
        if model.is_empty() || list_box_contains(list, model) {
            continue;
        }
        list.append(model);
        added += 1;
    }
    added
}

fn list_box_contains(list: &ListBox, model: &str) -> bool {
    (0..list.get_count()).any(|index| {
        list.get_string(index)
            .map(|value| value == model)
            .unwrap_or(false)
    })
}

fn list_box_models(list: &ListBox) -> Vec<String> {
    (0..list.get_count())
        .filter_map(|index| list.get_string(index))
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect()
}

fn parse_model_ids(value: &str) -> Vec<String> {
    value
        .split([',', '\n', '\r', ';'])
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn fetch_remote_models(
    base_url: &str,
    api_key: &str,
    timeout_secs: u64,
) -> Result<(Vec<String>, String), String> {
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|err| err.to_string())?;
    let mut errors = Vec::new();
    for candidate in model_list_candidates(base_url) {
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

fn model_list_candidates(base_url: &str) -> Vec<ModelListCandidate> {
    let raw = base_url.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Vec::new();
    }

    let root = provider_api_root(raw);
    let mut candidates = Vec::new();
    push_model_list_candidate(&mut candidates, format!("{raw}/models"), raw.to_string());
    push_model_list_candidate(
        &mut candidates,
        format!("{root}/v1/models"),
        provider_display_base_url(&root),
    );
    candidates
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

#[derive(Clone)]
struct UiHandles {
    text: GuiText,
    service_status: StatusPanel,
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
    inject_codex_button: Button,
    uninstall_button: Button,
    provider_image_generation: CheckBox,
    // AI Gateway fields
    ai_gw_provider_list: DataViewCtrl,
    ai_gw_provider_rows: AiGwProviderRows,
    ai_gw_provider_model: AiGwProviderModel,
    pending_ai_gw_channel_toggle: PendingAiGwChannelToggle,
    ai_gw_delete_button: Button,
    ai_gw_new_button: Button,
    ai_gw_edit_button: Button,
    ai_gw_status_label: StaticText,
}

#[derive(Clone)]
struct DashboardRefresh {
    in_flight: Arc<AtomicBool>,
    result: Arc<Mutex<Option<(u64, DashboardSnapshot)>>>,
    last_snapshot: Arc<Mutex<Option<DashboardSnapshot>>>,
    daemon_starting: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    pending_startup_child: Arc<Mutex<Option<Child>>>,
}

impl DashboardRefresh {
    fn new() -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
            last_snapshot: Arc::new(Mutex::new(None)),
            daemon_starting: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            closing: Arc::new(AtomicBool::new(false)),
            pending_startup_child: Arc::new(Mutex::new(None)),
        }
    }
}

enum ImActionResult {
    TelegramConfigure(Result<serde_json::Value, String>),
    AccountToggle(Result<serde_json::Value, String>),
    AccountDelete(Result<serde_json::Value, String>),
}

enum CodexActionResult {
    Inject(Result<serde_json::Value, String>),
    Clear(Result<serde_json::Value, String>),
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
    thread::spawn(move || {
        let snapshot = api.dashboard();
        if generation == current_generation.load(Ordering::SeqCst)
            && let Ok(mut slot) = result.lock()
        {
            slot.replace((generation, snapshot));
        }
        in_flight.store(false, Ordering::SeqCst);
    });
}

fn apply_pending_dashboard(handles: &UiHandles, refresh: &DashboardRefresh) -> bool {
    let result = refresh.result.lock().ok().and_then(|mut slot| slot.take());
    let Some((generation, snapshot)) = result else {
        return false;
    };
    if generation != refresh.generation.load(Ordering::SeqCst) {
        return false;
    }

    let daemon_starting = refresh.daemon_starting.load(Ordering::SeqCst);
    update_dashboard(handles, &snapshot, daemon_starting);
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

fn apply_pending_codex_action(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    refresh: &DashboardRefresh,
    result: &CodexActionResultStore,
) -> bool {
    let result = result.lock().ok().and_then(|mut slot| slot.take());
    let Some(result) = result else {
        return false;
    };

    handles
        .inject_codex_button
        .set_label(handles.text.inject_codex_access());
    handles
        .uninstall_button
        .set_label(handles.text.clear_codex_access());
    handles.inject_codex_button.enable(true);
    handles.uninstall_button.enable(true);

    match result {
        CodexActionResult::Inject(Ok(_)) => {
            show_info(frame, handles.text.codex_app_config_injected());
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::Inject(Err(err)) => {
            show_error(frame, &err);
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::Clear(Ok(_)) => {
            show_info(frame, handles.text.codex_app_config_uninstalled());
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::Clear(Err(err)) => {
            show_error(frame, &err);
            force_dashboard_refresh(api, refresh);
        }
    }
    true
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
        "",
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
    if !snapshot.service_online {
        if daemon_starting {
            show_dashboard_starting(handles);
            return;
        }
        set_status_panel(
            &handles.service_status,
            text.not_running(),
            text.gui_auto_start_service(),
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
        if !handles.provider_image_generation.has_focus() {
            handles
                .provider_image_generation
                .set_value(codex_status.image_generation_enabled);
        }
    }

    if let Some(status) = &snapshot.status {
        set_status_panel(
            &handles.service_status,
            text.running(),
            &text.listening(&status.bind),
            StateTone::Ok,
        );
    }

    refresh_im_account_list(handles, snapshot);

    let remote_connected = snapshot
        .remote
        .as_ref()
        .map(|remote| remote.connected)
        .unwrap_or(false);
    let remote_initialized = snapshot
        .remote
        .as_ref()
        .map(|remote| remote.initialized)
        .unwrap_or(false);
    let remote_status = snapshot.remote.as_ref();
    let codex_app_remote_ready = remote_connection_ready(remote_status, "codex_app")
        || remote_active_ready(remote_status, "codex_app");
    let vscode_remote_ready = remote_connection_ready(remote_status, "vscode")
        || remote_active_ready(remote_status, "vscode");
    let cli_remote_ready =
        remote_connection_ready(remote_status, "cli") || remote_active_ready(remote_status, "cli");
    let remote_initializing = remote_connected && !remote_initialized;
    let codex_configured = snapshot
        .codex_app
        .as_ref()
        .map(|status| status.configured)
        .unwrap_or(false);

    if CODEX_APP_GUI_UNSUPPORTED {
        set_disabled_status_panel(
            &handles.codex_status,
            text.unavailable(),
            text.app_gui_unsupported(),
        );
    } else if codex_app_remote_ready {
        set_status_panel(&handles.codex_status, text.connected(), "", StateTone::Ok);
    } else if remote_initializing {
        set_status_panel(
            &handles.codex_status,
            text.initializing(),
            "",
            StateTone::Warn,
        );
    } else if codex_configured {
        set_status_panel(
            &handles.codex_status,
            text.control_not_open(),
            "",
            StateTone::Warn,
        );
    } else {
        set_status_panel(
            &handles.codex_status,
            text.not_injected(),
            "",
            StateTone::Warn,
        );
    }

    if vscode_remote_ready {
        set_status_panel(&handles.vscode_status, text.connected(), "", StateTone::Ok);
    } else {
        set_status_panel(
            &handles.vscode_status,
            text.can_connect(),
            "",
            StateTone::Warn,
        );
    }

    if cli_remote_ready {
        set_status_panel(&handles.cli_status, text.connected(), "", StateTone::Ok);
    } else {
        set_status_panel(&handles.cli_status, text.can_connect(), "", StateTone::Warn);
    }

    // AI Gateway status
    if let Some(gw) = &snapshot.ai_gateway {
        handles
            .ai_gw_status_label
            .set_label(&text.ai_gw_status_enabled(gw.providers.len()));
        refresh_ai_gw_provider_list(handles, Some(gw));
    } else {
        handles
            .ai_gw_status_label
            .set_label(&text.ai_gw_status_enabled(0));
        refresh_ai_gw_provider_list(handles, None);
    }
}

fn set_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.change_bot_button.enable(enabled);
    handles.connect_wechat_button.enable(enabled);
    handles.save_telegram_button.enable(enabled);
    handles.delete_im_account_button.enable(enabled);
    handles.provider_image_generation.enable(enabled);
    handles.inject_codex_button.enable(enabled);
    handles.uninstall_button.enable(enabled);
    set_ai_gw_actions_enabled(handles, enabled);
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

fn show_about_dialog(parent: &Frame) {
    let dialog = Dialog::builder(parent, "About Codex Remote")
        .with_style(DialogStyle::DefaultDialogStyle)
        .with_size(520, 260)
        .build();
    dialog.set_icon(&app_icon_bitmap(48));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label(&format!("Codex Remote {}", env!("CARGO_PKG_VERSION")))
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(
        &title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let description = StaticText::builder(&panel)
        .with_label(&GuiText::new(load_gui_locale()).about_description())
        .build();
    description.set_foreground_color(Colour::rgb(88, 96, 108));
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

fn confirm_open_update_release(parent: &dyn WxWidget, text: GuiText, message: &str) -> bool {
    MessageDialog::builder(parent, message, text.update_dialog_title())
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
