use serde_json::{Value, json};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use wxdragon::{prelude::*, timer::Timer};

use super::{
    api::ApiClient,
    provider::strip_nul,
    show_error,
    text::{GuiLocale, GuiText},
    theme,
};
use crate::ai_gateway::{
    chatgpt_auth::{
        AccountSummary, AccountUsage, BASE_URL, ImportRequest, LoginStart, LoginStatus,
    },
    config::{ProviderConfig, ProviderType},
};

enum Event {
    Started(Result<LoginStart, String>),
    Poll(Result<LoginStatus, String>),
    Account(Result<AccountSummary, String>),
    Connected(Result<AccountSummary, String>),
    Usage(String, Result<AccountUsage, String>),
    Models(Result<Vec<String>, String>),
    Logout(Result<Value, String>),
}
type Events = Arc<Mutex<Vec<Event>>>;

mod usage;

fn label(text: GuiText, zh: &'static str, en: &'static str) -> &'static str {
    if text.locale == GuiLocale::ZhCn {
        zh
    } else {
        en
    }
}

fn post(api: &ApiClient, path: &str, id: &str) -> Result<Value, String> {
    api.post_json(path, &json!({"id":id}))
}

fn send(events: &Events, event: Event) {
    if let Ok(mut queue) = events.lock() {
        queue.push(event);
    }
}

fn fetch_models(api: ApiClient, id: String, events: Events) {
    thread::spawn(move || {
        send(
            &events,
            Event::Models(api.post_json_with_timeout(
                "/api/chatgpt/account/models",
                &json!({"id":id}),
                Duration::from_secs(75),
            )),
        )
    });
}

fn fetch_usage(api: ApiClient, id: String, events: Events) {
    thread::spawn(move || {
        let result = api.post_json_with_timeout(
            "/api/chatgpt/account/usage",
            &json!({"id":id}),
            Duration::from_secs(135),
        );
        send(&events, Event::Usage(id, result));
    });
}

fn account_text(account: &AccountSummary, text: GuiText) -> String {
    let mut value = format!(
        "{}  {}",
        account.email.as_deref().unwrap_or("ChatGPT"),
        account.plan.as_deref().unwrap_or_default()
    );
    if account.needs_login {
        value.push_str(label(
            text,
            "\n登录已过期，请重新登录",
            "\nAuthorization expired. Sign in again",
        ));
    } else if !account.can_refresh {
        value.push_str(label(
            text,
            "\n此凭证到期后需要重新登录",
            "\nSign in again when this authorization expires",
        ));
    }
    value
}

fn selected_models(list: CheckListBox) -> Vec<String> {
    (0..list.get_count())
        .filter(|i| list.is_checked(*i))
        .filter_map(|i| list.get_string(i as usize))
        .collect()
}

pub(super) fn show_channel_dialog(
    parent: &Frame,
    text: GuiText,
    api: ApiClient,
    initial: Option<&ProviderConfig>,
) -> Option<ProviderConfig> {
    let dialog = Dialog::builder(parent, text.chatgpt_channel())
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(680, 760)
        .build();
    dialog.set_min_size(Size::new(610, 500));
    let panel = ScrolledWindow::builder(&dialog)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    panel.set_background_color(theme::theme().bg_card);
    let root = BoxSizer::builder(Orientation::Vertical).build();
    let title = StaticText::builder(&panel)
        .with_label(text.chatgpt_channel())
        .build();
    title.set_font(&theme::font(theme::TextRole::Title));
    root.add(&title, 0, SizerFlag::All, 18);
    let name_label = StaticText::builder(&panel)
        .with_label(text.ai_gw_col_name())
        .build();
    root.add(&name_label, 0, SizerFlag::Left | SizerFlag::Right, 18);
    let name = TextCtrl::builder(&panel)
        .with_value(initial.map(|p| p.name.as_str()).unwrap_or("chatgpt"))
        .build();
    root.add(
        &name,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );
    if initial.is_some() {
        name.enable(false);
    }

    let status = StaticText::builder(&panel)
        .with_label(label(text, "未登录", "Not signed in"))
        .build();
    status.set_min_size(Size::new(480, 42));
    status.wrap(530);
    root.add(
        &status,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right,
        18,
    );
    let row = BoxSizer::builder(Orientation::Horizontal).build();
    let login = Button::builder(&panel)
        .with_label(label(text, "登录 ChatGPT", "Sign in to ChatGPT"))
        .build();
    let logout = Button::builder(&panel)
        .with_label(label(text, "退出账号", "Sign out"))
        .build();
    let import = Button::builder(&panel)
        .with_label(label(text, "导入 auth.json", "Import auth.json"))
        .build();
    import.set_tooltip(label(text,
        "导入凭证副本，不修改原文件。若与原程序同时使用后登录失效，请重新登录 ChatGPT。",
        "Import a copy without changing the source. If sharing authorization invalidates it, sign in to ChatGPT again."));
    let fetch = Button::builder(&panel)
        .with_label(text.ai_gw_fetch_models())
        .build();
    row.add(&login, 0, SizerFlag::Right, 8);
    row.add(&import, 0, SizerFlag::Right, 8);
    row.add(&logout, 0, SizerFlag::Right, 8);
    root.add_sizer(&row, 0, SizerFlag::Expand | SizerFlag::All, 18);
    let link = HyperlinkCtrl::builder(&panel)
        .with_label(label(text, "打开登录网页", "Open sign-in page"))
        .with_url("https://auth.openai.com")
        .build();
    link.show(false);
    root.add(
        &link,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );
    let usage_row = BoxSizer::builder(Orientation::Horizontal).build();
    let usage_title = StaticText::builder(&panel)
        .with_label(label(text, "账号用量", "Account usage"))
        .build();
    let refresh_usage = Button::builder(&panel)
        .with_label(label(text, "刷新用量", "Refresh usage"))
        .build();
    usage_row.add(&usage_title, 0, SizerFlag::AlignCenterVertical, 0);
    usage_row.add_stretch_spacer(1);
    usage_row.add(&refresh_usage, 0, SizerFlag::Right, 0);
    root.add_sizer(
        &usage_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right,
        18,
    );
    let usage_view = TextCtrl::builder(&panel)
        .with_value(label(
            text,
            "登录或导入账号后显示用量",
            "Sign in or import an account to view usage",
        ))
        .with_style(TextCtrlStyle::MultiLine | TextCtrlStyle::ReadOnly)
        .build();
    usage_view.set_min_size(Size::new(480, 150));
    root.add(
        &usage_view,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );
    let model_row = BoxSizer::builder(Orientation::Horizontal).build();
    let model_title = StaticText::builder(&panel)
        .with_label(text.ai_gw_models())
        .build();
    model_row.add(&model_title, 0, SizerFlag::AlignCenterVertical, 0);
    model_row.add_stretch_spacer(1);
    model_row.add(&fetch, 0, SizerFlag::Right, 0);
    root.add_sizer(
        &model_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right,
        18,
    );
    let models = CheckListBox::builder(&panel).build();
    models.set_min_size(Size::new(480, 120));
    if let Some(provider) = initial {
        for model in &provider.models {
            models.append(&strip_nul(model));
            models.check(models.get_count() - 1, true);
        }
    }
    root.add(
        &models,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );
    let priority_row = BoxSizer::builder(Orientation::Horizontal).build();
    let priority_label = StaticText::builder(&panel)
        .with_label(text.ai_gw_weight())
        .build();
    let priority = TextCtrl::builder(&panel)
        .with_value(&initial.map(|p| p.weight).unwrap_or(100).to_string())
        .build();
    priority_row.add(
        &priority_label,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        12,
    );
    priority_row.add(&priority, 0, SizerFlag::Right, 0);
    let add = Button::builder(&panel)
        .with_label(text.ai_gw_add_model())
        .build();
    priority_row.add_stretch_spacer(1);
    priority_row.add(&add, 0, SizerFlag::Right, 0);
    root.add_sizer(
        &priority_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right,
        18,
    );
    let footer = Panel::builder(&dialog).build();
    footer.set_background_color(theme::theme().bg_card);
    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let cancel = Button::builder(&footer)
        .with_id(ID_CANCEL)
        .with_label(text.cancel())
        .build();
    let save = Button::builder(&footer)
        .with_label(text.ai_gw_save_channel())
        .build();
    buttons.add_stretch_spacer(1);
    buttons.add(&cancel, 0, SizerFlag::Right, 8);
    buttons.add(&save, 0, SizerFlag::Right, 0);
    let footer_sizer = BoxSizer::builder(Orientation::Vertical).build();
    footer_sizer.add_sizer(&buttons, 0, SizerFlag::Expand | SizerFlag::All, 18);
    footer.set_sizer(footer_sizer, true);
    panel.set_sizer(root, true);
    panel.set_scroll_rate(0, 10);
    panel.fit_inside();
    let outer = BoxSizer::builder(Orientation::Vertical).build();
    outer.add(&panel, 1, SizerFlag::Expand, 0);
    outer.add(&footer, 0, SizerFlag::Expand, 0);
    dialog.set_sizer(outer, true);
    dialog.center();

    let auth_id = Rc::new(RefCell::new(
        initial.and_then(|p| p.chatgpt_auth_id.clone()),
    ));
    let original_id = auth_id.borrow().clone();
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let closed = Arc::new(AtomicBool::new(false));
    let session_id = Arc::new(Mutex::new(None::<String>));
    let issued_auth_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let account_label = Rc::new(RefCell::new(String::new()));
    let busy = Rc::new(RefCell::new(false));
    let usage_loading = Rc::new(RefCell::new(false));
    logout.enable(auth_id.borrow().is_some());
    fetch.enable(auth_id.borrow().is_some());
    save.enable(auth_id.borrow().is_some());
    refresh_usage.enable(false);

    if let Some(id) = original_id.clone() {
        let api = api.clone();
        let events = events.clone();
        status.set_label(label(text, "正在读取账号…", "Loading account..."));
        *busy.borrow_mut() = true;
        login.enable(false);
        import.enable(false);
        logout.enable(false);
        fetch.enable(false);
        save.enable(false);
        thread::spawn(move || {
            send(
                &events,
                Event::Account(api.post_json("/api/chatgpt/account/status", &json!({"id":id}))),
            )
        });
    }
    {
        let api = api.clone();
        let events = events.clone();
        let busy = busy.clone();
        let closed = closed.clone();
        let session_id = session_id.clone();
        let issued_auth_ids = issued_auth_ids.clone();
        login.on_click(move |_| {
            if *busy.borrow() {
                return;
            }
            *busy.borrow_mut() = true;
            login.enable(false);
            import.enable(false);
            refresh_usage.enable(false);
            logout.enable(false);
            fetch.enable(false);
            save.enable(false);
            status.set_label(label(text, "正在打开登录…", "Starting sign-in..."));
            let api = api.clone();
            let events = events.clone();
            let closed = closed.clone();
            let session_id = session_id.clone();
            let issued_auth_ids = issued_auth_ids.clone();
            thread::spawn(move || {
                let start: Result<LoginStart, String> =
                    api.post_json("/api/chatgpt/login/start", &json!({}));
                let id = match &start {
                    Ok(start) => start.session_id.clone(),
                    Err(_) => {
                        send(&events, Event::Started(start));
                        return;
                    }
                };
                *session_id.lock().unwrap() = Some(id.clone());
                send(&events, Event::Started(start));
                while !closed.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_secs(1));
                    let poll: Result<LoginStatus, String> =
                        api.post_json("/api/chatgpt/login/status", &json!({"id":id}));
                    let done = poll.as_ref().map(|s| s.done).unwrap_or(true);
                    if let Ok(result) = &poll
                        && let Some(account) = &result.account
                    {
                        issued_auth_ids
                            .lock()
                            .unwrap()
                            .push(account.auth_id.clone());
                    }
                    if closed.load(Ordering::SeqCst)
                        && let Ok(ref result) = poll
                        && let Some(account) = &result.account
                    {
                        let _ = post(&api, "/api/chatgpt/account/logout", &account.auth_id);
                    }
                    send(&events, Event::Poll(poll));
                    if done {
                        break;
                    }
                }
                let _ = post(&api, "/api/chatgpt/login/cancel", &id);
            });
        });
    }
    {
        let api = api.clone();
        let events = events.clone();
        let busy = busy.clone();
        let closed = closed.clone();
        let issued_auth_ids = issued_auth_ids.clone();
        import.on_click(move |_| {
            if *busy.borrow() {
                return;
            }
            let picker = FileDialog::builder(&dialog)
                .with_message(label(
                    text,
                    "选择 Codex 登录凭证",
                    "Select Codex account credentials",
                ))
                .with_default_file("auth.json")
                .with_wildcard("JSON (*.json)|*.json")
                .with_style(FileDialogStyle::Open | FileDialogStyle::FileMustExist)
                .build();
            if picker.show_modal() != ID_OK {
                return;
            }
            let Some(path) = picker.get_path() else {
                return;
            };
            *busy.borrow_mut() = true;
            login.enable(false);
            import.enable(false);
            logout.enable(false);
            fetch.enable(false);
            save.enable(false);
            refresh_usage.enable(false);
            status.set_label(label(text, "正在导入账号…", "Importing account..."));
            let api = api.clone();
            let events = events.clone();
            let closed = closed.clone();
            let issued_auth_ids = issued_auth_ids.clone();
            thread::spawn(move || {
                let result = (|| -> Result<AccountSummary, String> {
                    use std::io::Read;
                    let file = std::fs::File::open(path).map_err(|_| {
                        label(text, "无法读取所选文件", "Cannot read the selected file").to_string()
                    })?;
                    let mut bytes = Vec::new();
                    file.take(1024 * 1024 + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|_| {
                            label(text, "无法读取所选文件", "Cannot read the selected file")
                                .to_string()
                        })?;
                    if bytes.len() > 1024 * 1024 {
                        return Err(label(
                            text,
                            "文件过大，请选择 auth.json",
                            "File too large. Select auth.json",
                        )
                        .into());
                    }
                    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
                    let auth: Value = serde_json::from_slice(bytes).map_err(|_| {
                        label(text, "文件不是有效的 JSON", "The file is not valid JSON").to_string()
                    })?;
                    api.post_json("/api/chatgpt/account/import", &ImportRequest { auth })
                })();
                if let Ok(account) = &result {
                    issued_auth_ids
                        .lock()
                        .unwrap()
                        .push(account.auth_id.clone());
                    if closed.load(Ordering::SeqCst) {
                        let _ = post(&api, "/api/chatgpt/account/logout", &account.auth_id);
                    }
                }
                send(&events, Event::Connected(result));
            });
        });
    }
    {
        let api = api.clone();
        let events = events.clone();
        let auth_id = auth_id.clone();
        let usage_loading = usage_loading.clone();
        refresh_usage.on_click(move |_| {
            if *usage_loading.borrow() {
                return;
            }
            if let Some(id) = auth_id.borrow().clone() {
                *usage_loading.borrow_mut() = true;
                refresh_usage.enable(false);
                usage_view.set_value(label(text, "正在查询用量…", "Loading usage..."));
                fetch_usage(api.clone(), id, events.clone());
            }
        });
    }
    {
        let api = api.clone();
        let events = events.clone();
        let auth_id = auth_id.clone();
        let busy = busy.clone();
        fetch.on_click(move |_| {
            if let Some(id) = auth_id.borrow().clone() {
                *busy.borrow_mut() = true;
                fetch.enable(false);
                login.enable(false);
                import.enable(false);
                refresh_usage.enable(false);
                logout.enable(false);
                save.enable(false);
                status.set_label(label(text, "正在拉取模型…", "Loading models..."));
                fetch_models(api.clone(), id, events.clone());
            }
        });
    }
    {
        let api = api.clone();
        let events = events.clone();
        let auth_id = auth_id.clone();
        let busy = busy.clone();
        logout.on_click(move |_| {
            if let Some(id) = auth_id.borrow().clone() {
                *busy.borrow_mut() = true;
                login.enable(false);
                import.enable(false);
                refresh_usage.enable(false);
                logout.enable(false);
                fetch.enable(false);
                save.enable(false);
                let api = api.clone();
                let events = events.clone();
                thread::spawn(move || {
                    send(
                        &events,
                        Event::Logout(post(&api, "/api/chatgpt/account/logout", &id)),
                    )
                });
            }
        });
    }
    add.on_click(move |_| {
        let entry =
            TextEntryDialog::builder(&dialog, text.ai_gw_add_model(), text.ai_gw_upstream_model())
                .build();
        if entry.show_modal() == ID_OK {
            let value = strip_nul(&entry.get_value().unwrap_or_default())
                .trim()
                .to_string();
            if !value.is_empty()
                && !(0..models.get_count())
                    .any(|i| models.get_string(i as usize).as_deref() == Some(value.as_str()))
            {
                models.append(&value);
                models.check(models.get_count() - 1, true);
            }
        }
        entry.destroy();
    });
    {
        let auth_id = auth_id.clone();
        let busy = busy.clone();
        save.on_click(move |_| {
            if *busy.borrow() || auth_id.borrow().is_none() {
                return;
            }
            if strip_nul(&name.get_value()).trim().is_empty() {
                show_error(&dialog, text.ai_gw_provider_name_empty());
                return;
            }
            if selected_models(models).is_empty() {
                show_error(
                    &dialog,
                    label(text, "请至少选择一个模型", "Select at least one model"),
                );
                return;
            }
            if strip_nul(&priority.get_value())
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .is_none()
            {
                show_error(
                    &dialog,
                    label(
                        text,
                        "优先级需要填写正整数",
                        "Priority must be a positive integer",
                    ),
                );
                return;
            }
            dialog.end_modal(ID_OK);
        });
    }
    cancel.on_click(move |_| dialog.end_modal(ID_CANCEL));
    let timer = Timer::new(&dialog);
    {
        let api = api.clone();
        let events = events.clone();
        let auth_id = auth_id.clone();
        let busy = busy.clone();
        let account_label = account_label.clone();
        let usage_loading = usage_loading.clone();
        timer.on_tick(move |_| {
            let pending = { std::mem::take(&mut *events.lock().unwrap()) };
            for event in pending {
                match event {
                    Event::Started(Ok(start)) => {
                        link.set_url(&start.authorization_url);
                        link.show(true);
                        panel.layout();
                        panel.fit_inside();
                        status.set_label(label(
                            text,
                            "等待浏览器登录…",
                            "Waiting for browser sign-in...",
                        ));
                        if let Err(error) =
                            super::update::open_url_in_browser(text, &start.authorization_url)
                        {
                            show_error(&dialog, &error);
                        }
                    }
                    Event::Poll(Ok(result)) if !result.done => {}
                    Event::Poll(Ok(result)) => {
                        link.show(false);
                        panel.layout();
                        panel.fit_inside();
                        if let Some(account) = result.account {
                            send(&events, Event::Connected(Ok(account)));
                        } else {
                            *busy.borrow_mut() = false;
                            status.set_label(label(
                                text,
                                "登录未完成，请重试",
                                "Sign-in did not complete. Please retry",
                            ));
                            if let Some(error) = result.error {
                                show_error(&dialog, &error);
                            }
                        }
                    }
                    Event::Connected(Ok(account)) => {
                        *auth_id.borrow_mut() = Some(account.auth_id.clone());
                        *account_label.borrow_mut() = account_text(&account, text);
                        status.set_label(label(
                            text,
                            "账号已保存，正在拉取模型…",
                            "Account saved. Loading models...",
                        ));
                        *usage_loading.borrow_mut() = true;
                        refresh_usage.enable(false);
                        usage_view.set_value(label(text, "正在查询用量…", "Loading usage..."));
                        fetch_usage(api.clone(), account.auth_id.clone(), events.clone());
                        fetch_models(api.clone(), account.auth_id, events.clone());
                    }
                    Event::Account(Ok(account)) => {
                        *busy.borrow_mut() = false;
                        *account_label.borrow_mut() = account_text(&account, text);
                        status.set_label(&account_label.borrow());
                        if account.needs_login {
                            usage_view.set_value(label(
                                text,
                                "请重新登录后查询用量",
                                "Sign in again to view usage",
                            ));
                        } else {
                            *usage_loading.borrow_mut() = true;
                            usage_view.set_value(label(text, "正在查询用量…", "Loading usage..."));
                            fetch_usage(api.clone(), account.auth_id, events.clone());
                        }
                    }
                    Event::Usage(id, result) => {
                        if auth_id.borrow().as_ref() != Some(&id) {
                            continue;
                        }
                        *usage_loading.borrow_mut() = false;
                        match result {
                            Ok(result) => {
                                *account_label.borrow_mut() = account_text(&result.account, text);
                                if !*busy.borrow() {
                                    status.set_label(&account_label.borrow());
                                }
                                usage_view.set_value(&usage::format_usage(text, &result));
                            }
                            Err(error) => {
                                usage_view.set_value(&format!(
                                    "{}\n{}",
                                    label(
                                        text,
                                        "用量查询失败，请稍后重试",
                                        "Usage unavailable. Please retry later"
                                    ),
                                    strip_nul(&error)
                                ));
                            }
                        }
                    }
                    Event::Models(Ok(fetched)) => {
                        let selected = selected_models(models);
                        let mut all = fetched;
                        all.extend(selected.clone());
                        all.sort();
                        all.dedup();
                        models.clear();
                        for model in &all {
                            models.append(&strip_nul(model));
                            models.check(
                                models.get_count() - 1,
                                selected.contains(model)
                                    || (selected.is_empty() && model == "gpt-5.5"),
                            );
                        }
                        *busy.borrow_mut() = false;
                        if account_label.borrow().is_empty() {
                            status.set_label(label(text, "已登录", "Signed in"));
                        } else {
                            status.set_label(&account_label.borrow());
                        }
                    }
                    Event::Logout(Ok(_)) => {
                        *auth_id.borrow_mut() = None;
                        account_label.borrow_mut().clear();
                        *usage_loading.borrow_mut() = false;
                        usage_view.set_value(label(
                            text,
                            "登录或导入账号后显示用量",
                            "Sign in or import an account to view usage",
                        ));
                        *busy.borrow_mut() = false;
                        status.set_label(label(text, "未登录", "Not signed in"));
                    }
                    Event::Started(Err(error))
                    | Event::Poll(Err(error))
                    | Event::Account(Err(error))
                    | Event::Connected(Err(error))
                    | Event::Models(Err(error))
                    | Event::Logout(Err(error)) => {
                        *busy.borrow_mut() = false;
                        status.set_label(label(
                            text,
                            "操作未完成，请重试",
                            "Operation failed. Please retry",
                        ));
                        show_error(&dialog, &error);
                    }
                }
                if !*busy.borrow() {
                    login.enable(true);
                    import.enable(true);
                    logout.enable(auth_id.borrow().is_some());
                    fetch.enable(auth_id.borrow().is_some());
                    save.enable(auth_id.borrow().is_some());
                    refresh_usage.enable(auth_id.borrow().is_some() && !*usage_loading.borrow());
                }
            }
        });
    }
    timer.start(150, false);
    let result = dialog.show_modal();
    closed.store(true, Ordering::SeqCst);
    timer.stop();
    let provider = if result == ID_OK {
        let mut provider = initial.cloned().unwrap_or_default();
        provider.name = strip_nul(&name.get_value()).trim().into();
        provider.provider_type = ProviderType::ChatGptResponses;
        provider.base_url = BASE_URL.into();
        provider.api_key.clear();
        provider.models_url = None;
        provider.chatgpt_auth_id = auth_id.borrow().clone();
        provider.models = selected_models(models);
        provider.weight = strip_nul(&priority.get_value())
            .trim()
            .parse()
            .unwrap_or(100);
        Some(provider)
    } else {
        None
    };
    let issued_ids = issued_auth_ids.lock().unwrap().clone();
    let kept_id = provider.as_ref().and_then(|p| p.chatgpt_auth_id.clone());
    let pending_session = session_id.lock().unwrap().clone();
    thread::spawn(move || {
        if let Some(id) = pending_session {
            let _ = post(&api, "/api/chatgpt/login/cancel", &id);
        }
        for id in issued_ids {
            if original_id.as_ref() != Some(&id) && kept_id.as_ref() != Some(&id) {
                let _ = post(&api, "/api/chatgpt/account/logout", &id);
            }
        }
    });
    dialog.destroy();
    provider
}
