use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use wxdragon::prelude::*;

use super::{GuiTimers, ID_MENU_CHECK_UPDATE, ID_MENU_QUIT, update, widgets::app_icon_bitmap};
use super::{show_info, text::GuiText};

const ID_TRAY_SHOW_WINDOW: i32 = 11_001;

pub(super) struct TrayController {
    taskbar: TaskBarIcon,
    _menu: Rc<RefCell<Menu>>,
}

impl TrayController {
    pub(super) fn remove_icon(&self) {
        let _ = self.taskbar.remove_icon();
    }
}

pub(super) fn install(
    frame: &Frame,
    gui_timers: &GuiTimers,
    text: GuiText,
    update_check_in_flight: Arc<AtomicBool>,
    quitting: Rc<AtomicBool>,
) -> TrayController {
    let icon_type = platform_icon_type();
    let taskbar = TaskBarIcon::builder()
        .with_icon_type(icon_type)
        .with_icon(app_icon_bitmap(32))
        .with_tooltip("Codex Remote Gateway")
        .build();

    let menu = Rc::new(RefCell::new(
        Menu::builder()
            .append_item(ID_TRAY_SHOW_WINDOW, text.tray_open(), text.tray_open_help())
            .append_item(
                ID_MENU_CHECK_UPDATE,
                text.check_updates(),
                text.check_updates_help(),
            )
            .append_separator()
            .append_item(ID_MENU_QUIT, text.quit(), text.quit_help())
            .build(),
    ));
    taskbar.set_popup_menu(&mut menu.borrow_mut());

    {
        let frame = *frame;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        taskbar.on_left_double_click(move |_| {
            show_main_window(&frame);
        });
    }

    {
        let frame = *frame;
        let gui_timers = gui_timers.clone();
        let update_check_in_flight = update_check_in_flight.clone();
        let quitting = quitting.clone();
        taskbar.on_menu(move |event: Event| match event.get_id() {
            ID_TRAY_SHOW_WINDOW => show_main_window(&frame),
            ID_MENU_CHECK_UPDATE => {
                show_main_window(&frame);
                update::check_for_updates_async(
                    &frame,
                    &gui_timers,
                    text,
                    &update_check_in_flight,
                    &quitting,
                );
            }
            ID_MENU_QUIT => request_app_quit(&frame, &quitting),
            _ => event.skip(true),
        });
    }

    TrayController {
        taskbar,
        _menu: menu,
    }
}

pub(super) fn show_main_window(frame: &Frame) {
    frame.iconize(false);
    frame.show(true);
    frame.raise();
    frame.set_focus();
}

pub(super) fn hide_main_window(frame: &Frame, text: GuiText) {
    frame.show(false);
    show_info(frame, text.tray_still_running_message());
}

pub(super) fn request_app_quit(frame: &Frame, quitting: &Rc<AtomicBool>) {
    quitting.store(true, Ordering::SeqCst);
    frame.close(true);
}

#[cfg(target_os = "macos")]
fn platform_icon_type() -> TaskBarIconType {
    TaskBarIconType::CustomStatusItem
}

#[cfg(not(target_os = "macos"))]
fn platform_icon_type() -> TaskBarIconType {
    TaskBarIconType::Default
}
