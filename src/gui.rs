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

use crate::config::AppConfig;

#[cfg(target_os = "windows")]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
#[cfg(not(target_os = "windows"))]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
const DEFAULT_PROVIDER_NAME: &str = "ai-codex";
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
type ProviderRows = Rc<RefCell<Vec<[String; 5]>>>;
type ProviderModel = Rc<RefCell<CustomDataViewVirtualListModel>>;
type PendingImToggle = Rc<RefCell<Option<(String, String, bool)>>>;
type PendingProviderWebsocketToggle = Rc<RefCell<Option<(String, bool)>>>;

type FrameTimerStore = Rc<RefCell<Option<Timer<Frame>>>>;
type ConfigActionResultStore = Arc<Mutex<Option<ConfigActionResult>>>;
type ImActionResultStore = Arc<Mutex<Option<ImActionResult>>>;

mod api;
mod daemon;
mod im_accounts;
mod onboarding;
mod provider;
mod text;
mod update;
mod widgets;

use self::api::{
    ApiClient, CodexAppProviderStatus, CodexAppStatus, ConfigureTelegramBotRequest,
    DashboardSnapshot, DeleteImAccountRequest, DeleteProviderRequest, RemoteControlStatus,
    SetImAccountEnabledRequest,
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
use self::provider::{
    apply_pending_config_action, apply_provider_row_to_form, apply_provider_to_form,
    change_text_value_if_changed, clean_provider_text, clear_provider_list_selection,
    configure_codex_app_and_verify, delete_codex_provider_and_verify, fill_provider_form_if_empty,
    find_provider, is_real_provider_name, provider_config_request_from_ui, provider_from_list_row,
    provider_name_from_ui, save_codex_provider_and_verify, set_combo_value_if_changed,
    set_provider_websocket_and_verify,
};
use self::text::{GuiLocale, GuiText};
use self::widgets::{
    ImStatusPanel, StateTone, StatusIconKind, StatusPanel, app_icon_bitmap, im_status_panel,
    provider_combo_row, set_disabled_status_panel, set_im_channel_row, set_status_panel,
    status_panel, text_field_row, topology_connector, topology_splitter,
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
        .with_size(Size::new(1100, 760))
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
    if CODEX_APP_GUI_UNSUPPORTED {
        set_disabled_status_panel(
            &codex_status,
            text.unavailable(),
            text.app_gui_unsupported(),
        );
    }
    let service_status = status_panel(
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
    entry_column.add(&vscode_status.panel, 1, SizerFlag::Expand, 0);
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

    let config_static_box = StaticBox::builder(&codex_page)
        .with_label(text.provider_management())
        .build();
    let config_box =
        StaticBoxSizerBuilder::new_with_box(&config_static_box, Orientation::Vertical).build();
    let provider_image_generation = CheckBox::builder(&config_static_box)
        .with_label(text.image_generation_feature())
        .with_value(false)
        .build();
    provider_image_generation.set_tooltip(text.image_generation_feature_help());
    let provider_image_generation_note = StaticText::builder(&config_static_box)
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
    config_box.add_sizer(
        &provider_image_generation_row,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        12,
    );

    let new_provider_button = Button::builder(&config_static_box)
        .with_label(text.add())
        .build();
    new_provider_button.set_tooltip(text.new_provider_help());
    let save_provider_button = Button::builder(&config_static_box)
        .with_label(text.save())
        .build();
    save_provider_button.set_tooltip(text.save_provider_help());
    let delete_provider_button = Button::builder(&config_static_box)
        .with_label(text.delete())
        .build();
    delete_provider_button.set_tooltip(text.delete_provider_help());
    let configure_button = Button::builder(&config_static_box)
        .with_label(text.enable())
        .build();
    configure_button.set_tooltip(text.configure_provider_help());

    let provider_catalog = StaticText::builder(&config_static_box)
        .with_label(text.provider_catalog_loading())
        .build();
    provider_catalog.set_foreground_color(Colour::rgb(103, 111, 124));
    provider_catalog.wrap(980);
    config_box.add(
        &provider_catalog,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        8,
    );

    let provider_rows: ProviderRows = Rc::new(RefCell::new(Vec::new()));
    let pending_provider_websocket: PendingProviderWebsocketToggle = Rc::new(RefCell::new(None));
    let pending_provider_websocket_for_model = pending_provider_websocket.clone();
    let provider_model: ProviderModel = Rc::new(RefCell::new(CustomDataViewVirtualListModel::new(
        0,
        provider_rows.clone(),
        |rows: &ProviderRows, row, col| -> Variant {
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
            move |rows: &ProviderRows, row, col, value: &Variant| -> bool {
                if col != 4 {
                    return false;
                }
                let Some(enabled) = value.get_bool() else {
                    return false;
                };
                let mut rows = rows.borrow_mut();
                let Some(row_data): Option<&mut [String; 5]> = rows.get_mut(row) else {
                    return false;
                };
                if !is_real_provider_name(&row_data[0]) {
                    return false;
                }
                let provider_name = row_data[0].clone();
                row_data[4] = enabled.to_string();
                pending_provider_websocket_for_model
                    .borrow_mut()
                    .replace((provider_name, enabled));
                true
            },
        ),
        None::<fn(&ProviderRows, usize, usize) -> Option<DataViewItemAttr>>,
        Some(|rows: &ProviderRows, row, col| -> bool {
            if col != 4 {
                return true;
            }
            rows.borrow()
                .get(row)
                .map(|row_data: &[String; 5]| is_real_provider_name(&row_data[0]))
                .unwrap_or(false)
        }),
    )));
    let provider_list = DataViewCtrl::builder(&config_static_box)
        .with_style(
            DataViewStyle::Single | DataViewStyle::RowLines | DataViewStyle::HorizontalRules,
        )
        .with_size(Size::new(-1, 142))
        .build();
    provider_list.append_text_column(
        text.name(),
        0,
        160,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    provider_list.append_text_column(
        "Base URL",
        1,
        420,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    provider_list.append_text_column(
        text.current(),
        2,
        90,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    provider_list.append_text_column(
        "API Key",
        3,
        160,
        DataViewAlign::Left,
        DataViewColumnFlags::Resizable,
    );
    provider_list.append_toggle_column(
        text.provider_websocket(),
        4,
        100,
        DataViewAlign::Center,
        DataViewColumnFlags::Resizable,
    );
    provider_list.associate_model(&*provider_model.borrow());
    config_box.add(
        &provider_list,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );

    let provider_actions = BoxSizer::builder(Orientation::Horizontal).build();
    provider_actions.add_stretch_spacer(1);
    provider_actions.add(&new_provider_button, 0, SizerFlag::Right, 8);
    provider_actions.add(&delete_provider_button, 0, SizerFlag::Right, 8);
    provider_actions.add(&configure_button, 0, SizerFlag::Right, 0);
    config_box.add_sizer(
        &provider_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );

    let provider_help = StaticText::builder(&config_static_box)
        .with_label(text.api_key_help())
        .build();
    provider_help.set_foreground_color(Colour::rgb(91, 100, 114));
    provider_help.wrap(980);
    config_box.add(
        &provider_help,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        8,
    );

    let form = FlexGridSizer::builder(0, 2)
        .with_gap(Size::new(12, 10))
        .build();
    form.add_growable_col(1, 1);
    let provider_name = provider_combo_row(
        &config_static_box,
        &form,
        text.provider_name(),
        DEFAULT_PROVIDER_NAME,
    );
    let provider_base_url = text_field_row(&config_static_box, &form, "Base URL", "");
    let provider_key = text_field_row(&config_static_box, &form, "API Key", "");
    config_box.add_sizer(
        &form,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        8,
    );
    let save_actions = BoxSizer::builder(Orientation::Horizontal).build();
    save_actions.add_stretch_spacer(1);
    save_actions.add(&save_provider_button, 0, SizerFlag::Right, 0);
    config_box.add_sizer(
        &save_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        12,
    );
    codex_sizer.add_sizer(
        &config_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let uninstall_button = Button::builder(&codex_page)
        .with_label(text.clear_codex_access())
        .build();
    uninstall_button.set_tooltip(text.clear_codex_access_help());
    let codex_maintenance_actions = BoxSizer::builder(Orientation::Horizontal).build();
    codex_maintenance_actions.add_stretch_spacer(1);
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
        im_account_list,
        im_account_rows,
        im_account_model,
        pending_im_toggle,
        pending_provider_websocket,
        delete_im_account_button,
        save_telegram_button,
        connect_wechat_button,
        change_bot_button,
        uninstall_button,
        new_provider_button,
        save_provider_button,
        delete_provider_button,
        configure_button,
        provider_image_generation,
        provider_name,
        provider_base_url,
        provider_key,
        provider_list,
        provider_rows,
        provider_model,
        provider_catalog,
    };

    let daemon_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    let dashboard_refresh = DashboardRefresh::new();
    let config_action_result: ConfigActionResultStore = Arc::new(Mutex::new(None));
    let config_action_in_flight = Arc::new(AtomicBool::new(false));
    show_dashboard_starting(&handles);
    show_local_codex_app_config_preview(&handles, &api, &dashboard_refresh);

    {
        let handles = handles.clone();
        new_provider_button.on_click(move |_| {
            clear_provider_list_selection(&handles.provider_list);
            set_combo_value_if_changed(&handles.provider_name, "");
            change_text_value_if_changed(&handles.provider_base_url, "");
            change_text_value_if_changed(&handles.provider_key, "");
            handles
                .provider_catalog
                .set_label(handles.text.new_provider_prompt());
            handles.provider_catalog.wrap(980);
            handles.provider_catalog.layout();
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let provider_name = provider_name;
        let provider_base_url = provider_base_url;
        let provider_key = provider_key;
        let frame = frame;
        let handles = handles.clone();
        let config_action_result = config_action_result.clone();
        let config_action_in_flight = config_action_in_flight.clone();
        save_provider_button.on_click(move |_| {
            if config_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                config_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            handles
                .provider_catalog
                .set_label(handles.text.saving_provider());
            handles.provider_catalog.wrap(980);
            handles
                .save_provider_button
                .set_label(handles.text.save_in_progress());
            set_actions_enabled(&handles, false);
            frame.refresh(true, None);
            frame.update();

            let (selected_provider, request) = provider_config_request_from_ui(
                &handles,
                &provider_name,
                &provider_base_url,
                &provider_key,
                cached_dashboard_snapshot(&dashboard_refresh).as_ref(),
                false,
            );
            let api = api.clone();
            let config_action_result = config_action_result.clone();
            let config_action_in_flight = config_action_in_flight.clone();
            let text = handles.text;
            thread::spawn(move || {
                let outcome =
                    save_codex_provider_and_verify(&api, &request, &selected_provider, text);
                if let Ok(mut slot) = config_action_result.lock() {
                    slot.replace(ConfigActionResult::Save {
                        provider_name: selected_provider,
                        result: outcome,
                    });
                }
                config_action_in_flight.store(false, Ordering::SeqCst);
            });
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let provider_name = provider_name;
        let frame = frame;
        let handles = handles.clone();
        let config_action_result = config_action_result.clone();
        let config_action_in_flight = config_action_in_flight.clone();
        delete_provider_button.on_click(move |_| {
            if config_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                config_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            let provider_name = provider_name_from_ui(
                &handles,
                &provider_name,
                cached_dashboard_snapshot(&dashboard_refresh).as_ref(),
            );
            if provider_name.trim().is_empty() {
                config_action_in_flight.store(false, Ordering::SeqCst);
                show_error(&frame, handles.text.select_provider_to_delete());
                return;
            }
            if !confirm_delete_provider(&frame, handles.text, &provider_name) {
                config_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }

            handles
                .provider_catalog
                .set_label(handles.text.deleting_provider());
            handles.provider_catalog.wrap(980);
            handles
                .delete_provider_button
                .set_label(handles.text.delete_in_progress());
            set_actions_enabled(&handles, false);
            frame.refresh(true, None);
            frame.update();

            let request = DeleteProviderRequest { provider_name };
            let api = api.clone();
            let config_action_result = config_action_result.clone();
            let config_action_in_flight = config_action_in_flight.clone();
            let text = handles.text;
            thread::spawn(move || {
                let outcome = delete_codex_provider_and_verify(&api, &request, text);
                if let Ok(mut slot) = config_action_result.lock() {
                    slot.replace(ConfigActionResult::Delete(outcome));
                }
                config_action_in_flight.store(false, Ordering::SeqCst);
            });
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let provider_name = provider_name;
        let provider_base_url = provider_base_url;
        let provider_key = provider_key;
        let frame = frame;
        let handles = handles.clone();
        let config_action_result = config_action_result.clone();
        let config_action_in_flight = config_action_in_flight.clone();
        configure_button.on_click(move |_| {
            if config_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                config_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }
            handles
                .provider_catalog
                .set_label(handles.text.enabling_provider());
            handles.provider_catalog.wrap(980);
            handles
                .configure_button
                .set_label(handles.text.enable_in_progress());
            set_actions_enabled(&handles, false);
            frame.refresh(true, None);
            frame.update();

            let (selected_provider, request) = provider_config_request_from_ui(
                &handles,
                &provider_name,
                &provider_base_url,
                &provider_key,
                cached_dashboard_snapshot(&dashboard_refresh).as_ref(),
                true,
            );
            let api = api.clone();
            let config_action_result = config_action_result.clone();
            let config_action_in_flight = config_action_in_flight.clone();
            let text = handles.text;
            thread::spawn(move || {
                let outcome =
                    configure_codex_app_and_verify(&api, &request, &selected_provider, text);
                if let Ok(mut slot) = config_action_result.lock() {
                    slot.replace(ConfigActionResult::Configure {
                        provider_name: selected_provider,
                        result: outcome,
                    });
                }
                config_action_in_flight.store(false, Ordering::SeqCst);
            });
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles.clone();
        uninstall_button.on_click(move |_| {
            if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                return;
            }
            if !confirm_uninstall_codex_app_config(&frame, handles.text) {
                return;
            }

            match api.uninstall_codex_app() {
                Ok(_) => {
                    show_info(&frame, handles.text.codex_app_config_uninstalled());
                    show_local_codex_app_config_preview(&handles, &api, &dashboard_refresh);
                    schedule_dashboard_refresh(&api, &dashboard_refresh);
                }
                Err(err) => show_error(&frame, &err),
            }
        });
    }

    {
        let handles = handles.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        provider_name.on_selection_changed(move |_| {
            let selected = clean_provider_text(&provider_name.get_value());
            let Some(snapshot) = cached_dashboard_snapshot(&dashboard_refresh) else {
                return;
            };
            if let Some(provider) = find_provider(&snapshot, &selected) {
                apply_provider_to_form(&handles, &provider, true);
            }
        });
    }

    {
        let handles = handles.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        provider_list.on_selection_changed(move |_| {
            let Some(index) = provider_list.get_selected_row() else {
                return;
            };
            if let Some(snapshot) = cached_dashboard_snapshot(&dashboard_refresh) {
                if let Some(provider) = provider_from_list_row(&snapshot, index) {
                    apply_provider_to_form(&handles, &provider, true);
                    return;
                }
            }
            apply_provider_row_to_form(&handles, index);
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
        result_timer.on_tick(move |_| {
            apply_pending_dashboard(&handles, &dashboard_refresh);
        });
    }
    result_timer.start(DASHBOARD_RESULT_POLL_MS, false);
    result_timer_store.borrow_mut().replace(result_timer);
    gui_timers.track(&result_timer_store);

    let config_action_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let config_action_timer = Timer::new(&frame);
    {
        let api = api.clone();
        let handles = handles.clone();
        let frame = frame;
        let dashboard_refresh = dashboard_refresh.clone();
        let config_action_result = config_action_result.clone();
        let config_action_in_flight = config_action_in_flight.clone();
        config_action_timer.on_tick(move |_| {
            if !config_action_in_flight.load(Ordering::SeqCst)
                && let Some((provider_name, enabled)) =
                    handles.pending_provider_websocket.borrow_mut().take()
            {
                if !ensure_service_ready_for_action(&api, &frame, &dashboard_refresh) {
                    force_dashboard_refresh(&api, &dashboard_refresh);
                    return;
                }
                config_action_in_flight.store(true, Ordering::SeqCst);
                handles.provider_list.enable(false);
                let thread_api = api.clone();
                let config_action_result = config_action_result.clone();
                let config_action_in_flight = config_action_in_flight.clone();
                let text = handles.text;
                thread::spawn(move || {
                    let outcome = set_provider_websocket_and_verify(
                        &thread_api,
                        &provider_name,
                        enabled,
                        text,
                    );
                    if let Ok(mut slot) = config_action_result.lock() {
                        slot.replace(ConfigActionResult::ProviderWebSocket {
                            provider_name,
                            result: outcome,
                        });
                    }
                    config_action_in_flight.store(false, Ordering::SeqCst);
                });
            }
            apply_pending_config_action(
                &api,
                &handles,
                &frame,
                &dashboard_refresh,
                &config_action_result,
            );
        });
    }
    config_action_timer.start(DASHBOARD_RESULT_POLL_MS, false);
    config_action_timer_store
        .borrow_mut()
        .replace(config_action_timer);
    gui_timers.track(&config_action_timer_store);

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
    im_account_list: DataViewCtrl,
    im_account_rows: ImAccountRows,
    im_account_model: ImAccountModel,
    pending_im_toggle: PendingImToggle,
    pending_provider_websocket: PendingProviderWebsocketToggle,
    delete_im_account_button: Button,
    save_telegram_button: Button,
    connect_wechat_button: Button,
    change_bot_button: Button,
    uninstall_button: Button,
    new_provider_button: Button,
    save_provider_button: Button,
    delete_provider_button: Button,
    configure_button: Button,
    provider_image_generation: CheckBox,
    provider_name: ComboBox,
    provider_base_url: TextCtrl,
    provider_key: TextCtrl,
    provider_list: DataViewCtrl,
    provider_rows: ProviderRows,
    provider_model: ProviderModel,
    provider_catalog: StaticText,
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

enum ConfigActionResult {
    Configure {
        provider_name: String,
        result: Result<CodexAppStatus, String>,
    },
    Save {
        provider_name: String,
        result: Result<CodexAppStatus, String>,
    },
    ProviderWebSocket {
        provider_name: String,
        result: Result<CodexAppStatus, String>,
    },
    Delete(Result<CodexAppStatus, String>),
}

enum ImActionResult {
    TelegramConfigure(Result<serde_json::Value, String>),
    AccountToggle(Result<serde_json::Value, String>),
    AccountDelete(Result<serde_json::Value, String>),
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
        text.starting_backend(),
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
            text.service_reads_config()
        },
    );
    set_status_panel(
        &handles.vscode_status,
        text.waiting_service(),
        text.service_vscode_connect(),
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

fn show_local_codex_app_config_preview(
    handles: &UiHandles,
    api: &ApiClient,
    refresh: &DashboardRefresh,
) {
    if CODEX_APP_GUI_UNSUPPORTED {
        return;
    }
    let status = crate::codex_app_config::inspect_codex_app_config(None, &api.url("/backend-api"));
    let snapshot = DashboardSnapshot {
        service_online: false,
        codex_app: Some(local_codex_app_status(status)),
        ..DashboardSnapshot::default()
    };
    if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
        last_snapshot.replace(snapshot.clone());
    }
    fill_provider_form_if_empty(handles, &snapshot);
}

fn local_codex_app_status(status: crate::codex_app_config::CodexAppConfigStatus) -> CodexAppStatus {
    CodexAppStatus {
        configured: status.configured,
        provider: status.provider.map(local_codex_app_provider_status),
        providers: status
            .providers
            .into_iter()
            .map(local_codex_app_provider_status)
            .collect(),
        image_generation_enabled: status.image_generation_enabled,
    }
}

fn local_codex_app_provider_status(
    provider: crate::codex_app_config::CodexAppProviderStatus,
) -> CodexAppProviderStatus {
    CodexAppProviderStatus {
        name: provider.name,
        base_url: provider.base_url,
        key: provider.key,
        supports_websockets: provider.supports_websockets,
    }
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
                text.local_service_not_running()
            },
        );
        set_status_panel(
            &handles.vscode_status,
            text.unavailable(),
            text.local_service_not_running(),
            StateTone::Muted,
        );
        set_actions_enabled(handles, false);
        return;
    }

    set_actions_enabled(handles, true);

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
    let codex_control_ready = remote_connected && remote_initialized;
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
    } else if codex_control_ready {
        let detail = snapshot
            .remote
            .as_ref()
            .map(|remote| codex_remote_detail(text, remote))
            .unwrap_or_else(|| text.codex_remote_connected_detail().to_string());
        set_status_panel(
            &handles.codex_status,
            text.connected(),
            &detail,
            StateTone::Ok,
        );
    } else if remote_connected {
        set_status_panel(
            &handles.codex_status,
            text.initializing(),
            text.codex_initializing(),
            StateTone::Warn,
        );
    } else if codex_configured {
        set_status_panel(
            &handles.codex_status,
            text.control_not_open(),
            text.control_not_open_detail(),
            StateTone::Warn,
        );
    } else {
        set_status_panel(
            &handles.codex_status,
            text.not_injected(),
            text.fill_provider_then_enable(),
            StateTone::Warn,
        );
    }

    if codex_control_ready {
        let detail = snapshot
            .remote
            .as_ref()
            .map(|remote| codex_remote_detail(text, remote))
            .unwrap_or_else(|| text.remote_connected_detail().to_string());
        set_status_panel(
            &handles.vscode_status,
            text.connected(),
            &detail,
            StateTone::Ok,
        );
    } else {
        set_status_panel(
            &handles.vscode_status,
            text.can_connect(),
            text.vscode_wrapper_detail(),
            StateTone::Warn,
        );
    }
}

fn set_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.change_bot_button.enable(enabled);
    handles.connect_wechat_button.enable(enabled);
    handles.save_telegram_button.enable(enabled);
    handles.delete_im_account_button.enable(enabled);
    handles.configure_button.enable(enabled);
    handles.new_provider_button.enable(enabled);
    handles.save_provider_button.enable(enabled);
    handles.delete_provider_button.enable(enabled);
    handles.provider_image_generation.enable(enabled);
    handles.provider_list.enable(enabled);
    handles.uninstall_button.enable(enabled);
}

fn codex_remote_detail(text: GuiText, remote: &RemoteControlStatus) -> String {
    if remote.stale.unwrap_or(false) {
        return text.remote_stale().to_string();
    }
    if let Some(err) = &remote.last_error {
        return text.recent_error(err);
    }
    if remote.healthy.unwrap_or(false) {
        if let Some(status) = remote.last_app_pong_status.as_deref() {
            return text.remote_heartbeat(status);
        }
    }
    text.remote_connected_detail().to_string()
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

fn confirm_delete_provider(parent: &dyn WxWidget, text: GuiText, provider_name: &str) -> bool {
    MessageDialog::builder(
        parent,
        &text.confirm_delete_provider_message(provider_name),
        text.confirm_delete_provider_title(),
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
