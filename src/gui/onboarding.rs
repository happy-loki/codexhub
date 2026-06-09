use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use qrcode::{Color, QrCode};
use wxdragon::{prelude::*, timer::Timer};

use super::api::{ApiClient, WechatOnboardPoll};
use super::provider::strip_nul;
use super::show_error;
use super::text::GuiText;

fn qr_bitmap(value: &str) -> Option<(Bitmap, i32)> {
    let code = QrCode::new(value.as_bytes()).ok()?;
    const TARGET_PIXELS: usize = 560;
    let quiet_zone = 4usize;
    let cells = code.width() + quiet_zone * 2;
    let module_size = (TARGET_PIXELS / cells).clamp(3, 12);
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

pub(super) fn prompt_telegram_bot_token(parent: &Frame, text: GuiText) -> Option<String> {
    let dialog = Dialog::builder(parent, text.telegram_dialog_title())
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(520, 300)
        .build();
    dialog.set_min_size(Size::new(520, 280));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label(text.telegram_token_title())
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(
        &title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let input = TextCtrl::builder(&panel)
        .with_value("")
        .with_style(TextCtrlStyle::Default | TextCtrlStyle::ProcessEnter)
        .build();
    input.set_min_size(Size::new(460, 30));
    sizer.add(
        &input,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let hint = StaticText::builder(&panel)
        .with_label(text.telegram_private_hint())
        .build();
    hint.set_foreground_color(Colour::rgb(103, 111, 124));
    sizer.add(
        &hint,
        0,
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
        .with_label(text.save_and_connect())
        .build();
    save_button.set_default();
    buttons.add_stretch_spacer(1);
    buttons.add(&cancel_button, 0, SizerFlag::Right, 8);
    buttons.add(&save_button, 0, SizerFlag::Right, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom | SizerFlag::Top,
        18,
    );

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    {
        let dialog = dialog;
        cancel_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
    }
    {
        let dialog = dialog;
        save_button.on_click(move |_| dialog.end_modal(ID_OK));
    }
    {
        let dialog = dialog;
        input.on_text_enter(move |_| dialog.end_modal(ID_OK));
    }

    input.set_focus();
    let result = dialog.show_modal();
    let token = strip_nul(&input.get_value()).trim().to_string();
    dialog.destroy();

    if result != ID_OK {
        return None;
    }
    if token.is_empty() {
        show_error(parent, text.telegram_token_required());
        return None;
    }
    Some(token)
}

pub(super) fn show_feishu_onboard_dialog(parent: &Frame, text: GuiText, api: ApiClient) {
    let start = match api.start_feishu_onboard() {
        Ok(start) => start,
        Err(err) => {
            show_error(parent, &err);
            return;
        }
    };

    let dialog = Dialog::builder(parent, text.feishu_onboard_title())
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(660, 760)
        .build();
    dialog.set_min_size(Size::new(560, 660));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label(text.scan_feishu())
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(&title, 0, SizerFlag::All, 18);

    if let Some((bitmap, qr_size)) = qr_bitmap(&start.verification_uri_complete) {
        let qr_panel = Panel::builder(&panel).build();
        qr_panel.set_background_color(Colour::rgb(255, 255, 255));
        qr_panel.set_min_size(Size::new(500, 500));

        let qr = StaticBitmap::builder(&qr_panel)
            .with_bitmap(Some(bitmap))
            .with_scale_mode(Some(ScaleMode::AspectFit))
            .with_size(Size::new(qr_size.max(500), qr_size.max(500)))
            .build();
        qr.set_min_size(Size::new(500, 500));

        let qr_sizer = BoxSizer::builder(Orientation::Vertical).build();
        qr_sizer.add(&qr, 1, SizerFlag::Expand | SizerFlag::All, 0);
        qr_panel.set_sizer(qr_sizer, true);

        sizer.add(
            &qr_panel,
            1,
            SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
            12,
        );
    } else {
        let qr_error = StaticText::builder(&panel)
            .with_label(text.qr_open_browser_failed())
            .build();
        qr_error.set_foreground_color(Colour::rgb(185, 55, 55));
        sizer.add(
            &qr_error,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            80,
        );
    }

    let fallback_link = HyperlinkCtrl::builder(&panel)
        .with_label(text.feishu_fallback_link())
        .with_url(&start.verification_uri_complete)
        .build();
    sizer.add(
        &fallback_link,
        0,
        SizerFlag::AlignCenterHorizontal | SizerFlag::Bottom,
        12,
    );

    let info = StaticText::builder(&panel)
        .with_label(text.scan_done_auto_close())
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(600);
    sizer.add(
        &info,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&panel).with_label(text.close()).build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

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
                    info.set_label(text.scan_done_auto_close());
                } else if result.error.is_some() {
                    info.set_label(text.onboard_failed_retry());
                }
            }
            Err(_) => {
                info.set_label(text.onboard_failed_retry());
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

pub(super) fn show_wechat_onboard_dialog(parent: &Frame, text: GuiText, api: ApiClient) {
    let start = match api.start_wechat_onboard() {
        Ok(start) => start,
        Err(err) => {
            show_error(parent, &err);
            return;
        }
    };

    let dialog = Dialog::builder(parent, text.wechat_onboard_title())
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(660, 760)
        .build();
    dialog.set_min_size(Size::new(560, 660));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label(text.scan_wechat())
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(&title, 0, SizerFlag::All, 18);

    if let Some((bitmap, qr_size)) = qr_bitmap(&start.qrcode_url) {
        let qr_panel = Panel::builder(&panel).build();
        qr_panel.set_background_color(Colour::rgb(255, 255, 255));
        qr_panel.set_min_size(Size::new(500, 500));

        let qr = StaticBitmap::builder(&qr_panel)
            .with_bitmap(Some(bitmap))
            .with_scale_mode(Some(ScaleMode::AspectFit))
            .with_size(Size::new(qr_size.max(500), qr_size.max(500)))
            .build();
        qr.set_min_size(Size::new(500, 500));

        let qr_sizer = BoxSizer::builder(Orientation::Vertical).build();
        qr_sizer.add(&qr, 1, SizerFlag::Expand | SizerFlag::All, 0);
        qr_panel.set_sizer(qr_sizer, true);

        sizer.add(
            &qr_panel,
            1,
            SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
            12,
        );
    } else {
        let qr_error = StaticText::builder(&panel)
            .with_label(text.qr_retry_failed())
            .build();
        qr_error.set_foreground_color(Colour::rgb(185, 55, 55));
        sizer.add(
            &qr_error,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            80,
        );
    }

    let verify_row = BoxSizer::builder(Orientation::Horizontal).build();
    let verify_label = StaticText::builder(&panel)
        .with_label(text.verify_code())
        .build();
    verify_label.set_foreground_color(Colour::rgb(78, 86, 98));
    let verify_code = TextCtrl::builder(&panel).with_value("").build();
    verify_code.set_min_size(Size::new(220, 30));
    verify_code.enable(false);
    verify_row.add(
        &verify_label,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        8,
    );
    verify_row.add(&verify_code, 0, SizerFlag::Right, 0);
    sizer.add_sizer(
        &verify_row,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let info = StaticText::builder(&panel)
        .with_label(&text.wechat_expire_notice(start.expires_in))
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(600);
    sizer.add(
        &info,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&panel).with_label(text.close()).build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    let poll_result: Arc<Mutex<Option<Result<WechatOnboardPoll, String>>>> =
        Arc::new(Mutex::new(None));
    let poll_in_flight = Arc::new(AtomicBool::new(false));
    let poll_closed = Arc::new(AtomicBool::new(false));
    let timer = Timer::new(&dialog);
    {
        let api = api.clone();
        let session_key = start.session_key.clone();
        let dialog = dialog;
        let poll_result = poll_result.clone();
        let poll_in_flight = poll_in_flight.clone();
        let poll_closed = poll_closed.clone();
        timer.on_tick(move |_| {
            let poll = poll_result.lock().ok().and_then(|mut slot| slot.take());
            if let Some(poll) = poll {
                poll_in_flight.store(false, Ordering::SeqCst);
                match poll {
                    Ok(result) if result.done => {
                        poll_closed.store(true, Ordering::SeqCst);
                        dialog.end_modal(ID_OK);
                        return;
                    }
                    Ok(result) => {
                        if result.need_verify_code.unwrap_or(false) {
                            verify_code.enable(true);
                        }
                        info.set_label(&wechat_onboard_status_text(text, &result));
                        info.wrap(600);
                    }
                    Err(_) => {
                        info.set_label(text.onboard_failed_retry());
                    }
                }
            }

            if poll_closed.load(Ordering::SeqCst) || poll_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }

            let api = api.clone();
            let session_key = session_key.clone();
            let code = strip_nul(&verify_code.get_value()).trim().to_string();
            let poll_result = poll_result.clone();
            let poll_in_flight = poll_in_flight.clone();
            let poll_closed = poll_closed.clone();
            thread::spawn(move || {
                let verify_code = (!code.is_empty()).then_some(code);
                let poll = api.poll_wechat_onboard(&session_key, verify_code.as_deref());
                if poll_closed.load(Ordering::SeqCst) {
                    poll_in_flight.store(false, Ordering::SeqCst);
                    return;
                }
                if let Ok(mut slot) = poll_result.lock() {
                    slot.replace(poll);
                } else {
                    poll_in_flight.store(false, Ordering::SeqCst);
                }
            });
        });
    }
    timer.start(1500, false);

    {
        let dialog = dialog;
        let poll_closed = poll_closed.clone();
        close_button.on_click(move |_| {
            poll_closed.store(true, Ordering::SeqCst);
            dialog.end_modal(ID_CANCEL);
        });
    }

    dialog.show_modal();
    poll_closed.store(true, Ordering::SeqCst);
    timer.stop();
    dialog.destroy();
}

fn wechat_onboard_status_text(text: GuiText, result: &WechatOnboardPoll) -> String {
    if result.need_verify_code.unwrap_or(false) {
        return text.wechat_need_verify().to_string();
    }
    if let Some(error) = result.error.as_ref().and_then(|value| value.as_str()) {
        return match error {
            "expired" => text.wechat_qr_expired().to_string(),
            "verify_code_blocked" => text.wechat_verify_blocked().to_string(),
            _ => text.onboard_pending_error(error),
        };
    }
    match result.status.as_deref() {
        Some("wait") => text.wechat_wait().to_string(),
        Some("scaned") => text.wechat_scanned().to_string(),
        Some("scaned_but_redirect") => text.wechat_redirect().to_string(),
        Some("confirmed") => text.wechat_confirmed().to_string(),
        Some("binded_redirect") if result.already_connected.unwrap_or(false) => {
            text.wechat_bound().to_string()
        }
        Some(status) => text.current_status(status),
        None => text.scan_done_auto_close().to_string(),
    }
}

fn is_feishu_onboard_pending(error: Option<&serde_json::Value>) -> bool {
    matches!(
        error.and_then(|value| value.as_str()),
        Some("authorization_pending" | "slow_down")
    )
}
