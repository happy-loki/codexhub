use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;
use wxdragon::prelude::*;

use super::api::{RequestLogDetail, RequestLogItem};
use super::provider::strip_nul;
use super::text::GuiText;
use super::theme;
use super::widgets::apply_notebook_theme;

const STYLE_JSON_DEFAULT: i32 = 0;
const STYLE_JSON_KEY: i32 = 1;
const STYLE_JSON_STRING: i32 = 2;
const STYLE_JSON_NUMBER: i32 = 3;
const STYLE_JSON_KEYWORD: i32 = 4;
const STYLE_JSON_PUNCT: i32 = 5;
const STYLE_LINE_NUMBER: i32 = 33;
const STYLE_INDENT_GUIDE: i32 = 37;
const INDICATOR_FIND_MATCH: i32 = 8;
const FOLD_BASE: i32 = 0x400;
const FOLD_HEADER: i32 = 0x2000;
const FOLD_MARGIN: i32 = 2;
const FOLD_MARKER_MASK: i32 = 0xFE00_0000u32 as i32;
const FOLD_FLAG_LINE_BEFORE_CONTRACTED: i32 = 0x0004;
const FOLD_FLAG_LINE_AFTER_CONTRACTED: i32 = 0x0010;
const MARKER_FOLDER_END: i32 = 25;
const MARKER_FOLDER_OPEN_MID: i32 = 26;
const MARKER_FOLDER_MID_TAIL: i32 = 27;
const MARKER_FOLDER_TAIL: i32 = 28;
const MARKER_FOLDER_SUB: i32 = 29;
const MARKER_FOLDER: i32 = 30;
const MARKER_FOLDER_OPEN: i32 = 31;
const KEY_F3: i32 = 342;
const KEY_CTRL_F: i32 = 6;
const KEY_ESCAPE: i32 = 27;

#[derive(Default)]
struct SearchState {
    query: String,
}

pub(super) fn show(parent: &Frame, text: GuiText, detail: &RequestLogDetail) {
    let id = detail.summary.id;
    let dialog = Dialog::builder(parent, &text.request_log_detail_title(id))
        .with_style(
            DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder | DialogStyle::MaximizeBox,
        )
        .with_size(1480, 900)
        .build();
    dialog.set_min_size(Size::new(1120, 760));
    dialog.set_background_color(theme::theme().bg_card_alt);

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(theme::theme().bg_card_alt);

    let root = BoxSizer::builder(Orientation::Vertical).build();

    let summary = StaticText::builder(&panel)
        .with_label(&summary_text(&detail.summary))
        .build();
    summary.set_foreground_color(theme::theme().ink_secondary);
    root.add(
        &summary,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let notebook = Notebook::builder(&panel).build();
    apply_notebook_theme(&notebook);
    let codex_request_detail = request_detail_json(
        detail.request_headers_json.as_deref(),
        detail.request_json.as_deref(),
    );
    let upstream_request_detail = request_detail_json(
        detail.upstream_request_headers_json.as_deref(),
        detail.upstream_request_json.as_deref(),
    );
    add_json_tab(
        &notebook,
        text.request_log_detail_codex_request(),
        codex_request_detail.as_deref(),
        text,
    );
    add_json_tab(
        &notebook,
        text.request_log_detail_upstream_request(),
        upstream_request_detail.as_deref(),
        text,
    );
    add_text_tab(
        &notebook,
        text.request_log_detail_upstream_sse(),
        detail
            .upstream_response_sse
            .as_deref()
            .unwrap_or_else(|| text.request_log_detail_empty()),
    );
    add_json_tab(
        &notebook,
        text.request_log_detail_response(),
        detail.response_json.as_deref(),
        text,
    );
    if let Some(error) = detail.summary.error_message.as_deref() {
        add_text_tab(&notebook, text.request_log_detail_error(), error);
    }
    root.add(&notebook, 1, SizerFlag::Expand | SizerFlag::All, 12);

    let close_button = Button::builder(&panel)
        .with_label(match text.locale {
            super::text::GuiLocale::ZhCn => "关闭",
            super::text::GuiLocale::EnUs => "Close",
        })
        .build();
    {
        let dialog = dialog;
        close_button.on_click(move |_| {
            dialog.end_modal(ID_OK);
        });
    }
    let button_row = BoxSizer::builder(Orientation::Horizontal).build();
    button_row.add(&close_button, 0, SizerFlag::All, 0);
    root.add_sizer(
        &button_row,
        0,
        SizerFlag::AlignRight | SizerFlag::Right | SizerFlag::Bottom,
        12,
    );

    panel.set_sizer(root, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();
    let _ = dialog.show_modal();
    dialog.destroy();
}

fn request_detail_json(headers: Option<&str>, body: Option<&str>) -> Option<String> {
    if headers.is_none() && body.is_none() {
        return None;
    }

    let headers = headers
        .map(parse_json_or_string)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let body = body
        .map(parse_json_or_string)
        .unwrap_or(serde_json::Value::Null);

    serde_json::to_string(&serde_json::json!({
        "headers": headers,
        "body": body,
    }))
    .ok()
}

fn parse_json_or_string(content: &str) -> serde_json::Value {
    serde_json::from_str(content).unwrap_or_else(|_| serde_json::Value::String(content.to_string()))
}

fn add_json_tab(parent: &Notebook, label: &str, content: Option<&str>, text: GuiText) {
    let Some(content) = content else {
        add_text_tab(parent, label, text.request_log_detail_empty());
        return;
    };
    let json_value = serde_json::from_str::<serde_json::Value>(content).ok();
    let display = json_value
        .as_ref()
        .and_then(format_json_pretty)
        .unwrap_or_else(|| content.to_string());

    let panel = Panel::builder(parent).build();
    panel.set_background_color(theme::theme().bg_card);

    let search_bar = Panel::builder(&panel).build();
    search_bar.set_background_color(theme::theme().bg_muted);
    let search_input = SearchCtrl::builder(&search_bar)
        .with_style(SearchCtrlStyle::ProcessEnter)
        .with_size(Size::new(360, -1))
        .build();
    search_input.show_search_button(true);
    search_input.show_cancel_button(true);
    let prev_button = Button::builder(&search_bar)
        .with_label(search_prev_text(text))
        .build();
    let next_button = Button::builder(&search_bar)
        .with_label(search_next_text(text))
        .build();
    let close_search_button = Button::builder(&search_bar)
        .with_label(search_close_text(text))
        .build();
    let search_status = StaticText::builder(&search_bar)
        .with_label(search_idle_text(text))
        .build();
    search_status.set_foreground_color(theme::theme().ink_muted);

    let search_bar_sizer = BoxSizer::builder(Orientation::Horizontal).build();
    search_bar_sizer.add(
        &search_input,
        0,
        SizerFlag::Left | SizerFlag::Top | SizerFlag::Bottom | SizerFlag::AlignCenterVertical,
        6,
    );
    search_bar_sizer.add(
        &prev_button,
        0,
        SizerFlag::Left | SizerFlag::AlignCenterVertical,
        6,
    );
    search_bar_sizer.add(
        &next_button,
        0,
        SizerFlag::Left | SizerFlag::AlignCenterVertical,
        6,
    );
    search_bar_sizer.add(
        &close_search_button,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::AlignCenterVertical,
        6,
    );
    search_bar_sizer.add(
        &search_status,
        1,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::AlignCenterVertical,
        6,
    );
    search_bar.set_sizer(search_bar_sizer, true);
    search_bar.hide();

    let editor = StyledTextCtrl::builder(&panel)
        .with_size(Size::new(1200, 720))
        .build();
    configure_json_editor(&editor, &display);
    let search_state = Rc::new(RefCell::new(SearchState::default()));
    {
        let panel = panel;
        let editor = editor;
        let search_bar = search_bar;
        let search_input = search_input;
        let search_status = search_status;
        let search_state = Rc::clone(&search_state);
        bind_search_shortcuts(
            &editor,
            panel,
            editor,
            search_bar,
            search_input,
            search_status,
            search_state,
            text,
        );
    }
    bind_search_bar_controls(
        panel,
        search_bar,
        search_input,
        prev_button,
        next_button,
        close_search_button,
        editor,
        search_status,
        Rc::clone(&search_state),
        text,
    );
    {
        let panel = panel;
        let search_bar = search_bar;
        let search_input = search_input;
        let search_status = search_status;
        let search_state = Rc::clone(&search_state);
        search_input.on_key_down(move |event| {
            if handle_search_input_key(
                &event,
                panel,
                search_bar,
                search_input,
                editor,
                search_status,
                &search_state,
                text,
            ) {
                event.skip(false);
            } else {
                event.skip(true);
            }
        });
    }
    {
        let editor = editor;
        editor.on_stc_margin_click(move |event| {
            if event.get_margin() == Some(FOLD_MARGIN)
                && let Some(position) = event.get_position()
            {
                toggle_editor_fold_at_position(&editor, position);
            }
        });
    }
    {
        let editor = editor;
        editor.on_stc_double_click(move |_| {
            toggle_editor_fold_at_line(&editor, editor.get_current_line());
        });
    }

    let sizer = BoxSizer::builder(Orientation::Vertical).build();
    sizer.add(&search_bar, 0, SizerFlag::Expand | SizerFlag::All, 0);
    sizer.add(&editor, 1, SizerFlag::Expand | SizerFlag::All, 8);
    panel.set_sizer(sizer, true);
    parent.add_page(&panel, label, false, None);
}

fn update_editor_search(
    editor: &StyledTextCtrl,
    query: &str,
    status: &StaticText,
    state: &Rc<RefCell<SearchState>>,
    backwards: bool,
    text: GuiText,
) {
    let query = strip_nul(query);
    let query = query.trim();
    clear_search_indicators(editor);
    if query.is_empty() {
        editor.set_selection(0, 0);
        status.set_label(search_idle_text(text));
        *state.borrow_mut() = SearchState::default();
        return;
    }

    let same_query = state.borrow().query == query;
    let start_pos = search_start_position(editor, backwards, same_query);
    let match_count = mark_search_matches(editor, query);
    if let Some(position) =
        editor.find_and_select(start_pos, query, FindFlags::None, backwards, true)
    {
        state.borrow_mut().query = query.to_string();
        status.set_label(&search_result_text(
            text,
            query,
            search_match_index(editor, query, position, match_count),
            match_count,
        ));
    } else {
        editor.set_selection(0, 0);
        status.set_label(&search_result_text(text, query, 0, 0));
        state.borrow_mut().query = query.to_string();
    }
}

fn bind_search_shortcuts(
    editor: &StyledTextCtrl,
    panel: Panel,
    target: StyledTextCtrl,
    search_bar: Panel,
    search_input: SearchCtrl,
    status: StaticText,
    state: Rc<RefCell<SearchState>>,
    text: GuiText,
) {
    {
        let panel = panel;
        let target = target;
        let search_bar = search_bar;
        let search_input = search_input;
        let status = status;
        let state = Rc::clone(&state);
        editor.on_key_down(move |event| {
            if handle_search_shortcut(
                &event,
                panel,
                target,
                search_bar,
                search_input,
                status,
                &state,
                text,
            ) {
                event.skip(false);
            } else {
                event.skip(true);
            }
        });
    }
    {
        let panel = panel;
        let target = target;
        let search_bar = search_bar;
        let search_input = search_input;
        let status = status;
        let state = Rc::clone(&state);
        editor.on_char(move |event| {
            if handle_search_shortcut(
                &event,
                panel,
                target,
                search_bar,
                search_input,
                status,
                &state,
                text,
            ) {
                event.skip(false);
            } else {
                event.skip(true);
            }
        });
    }
}

fn handle_search_shortcut(
    event: &WindowEventData,
    panel: Panel,
    editor: StyledTextCtrl,
    search_bar: Panel,
    search_input: SearchCtrl,
    status: StaticText,
    state: &Rc<RefCell<SearchState>>,
    text: GuiText,
) -> bool {
    let WindowEventData::Keyboard(key_event) = event else {
        return false;
    };
    let key_code = key_event.get_key_code().unwrap_or_default();
    let unicode_key = key_event.get_unicode_key().unwrap_or_default();
    let is_find = key_code == KEY_CTRL_F
        || unicode_key == KEY_CTRL_F
        || (key_event.cmd_down()
            && (key_code == b'F' as i32
                || key_code == b'f' as i32
                || unicode_key == b'F' as i32
                || unicode_key == b'f' as i32));
    if is_find {
        show_search_bar(panel, search_bar, search_input, editor, status, state, text);
        return true;
    }
    if key_code == KEY_F3 {
        let query = state.borrow().query.clone();
        update_editor_search(
            &editor,
            &query,
            &status,
            state,
            key_event.shift_down(),
            text,
        );
        return true;
    }
    false
}

fn bind_search_bar_controls(
    panel: Panel,
    search_bar: Panel,
    search_input: SearchCtrl,
    prev_button: Button,
    next_button: Button,
    close_search_button: Button,
    editor: StyledTextCtrl,
    status: StaticText,
    state: Rc<RefCell<SearchState>>,
    text: GuiText,
) {
    {
        let editor = editor;
        let status = status;
        let state = Rc::clone(&state);
        search_input.on_text_updated(move |_| {
            let query = search_input.get_value();
            update_editor_search(&editor, &query, &status, &state, false, text);
        });
    }
    {
        let editor = editor;
        let status = status;
        let state = Rc::clone(&state);
        search_input.on_enter_pressed(move |_| {
            let query = search_input.get_value();
            update_editor_search(&editor, &query, &status, &state, false, text);
        });
    }
    {
        let editor = editor;
        let status = status;
        let state = Rc::clone(&state);
        search_input.on_search_button_clicked(move |_| {
            let query = search_input.get_value();
            update_editor_search(&editor, &query, &status, &state, false, text);
        });
    }
    {
        let editor = editor;
        let status = status;
        let state = Rc::clone(&state);
        search_input.on_cancel_button_clicked(move |_| {
            search_input.set_value("");
            clear_search_indicators(&editor);
            editor.set_selection(0, 0);
            status.set_label(search_idle_text(text));
            *state.borrow_mut() = SearchState::default();
        });
    }
    {
        let editor = editor;
        let status = status;
        let state = Rc::clone(&state);
        prev_button.on_click(move |_| {
            let query = search_input.get_value();
            update_editor_search(&editor, &query, &status, &state, true, text);
        });
    }
    {
        let editor = editor;
        let status = status;
        let state = Rc::clone(&state);
        next_button.on_click(move |_| {
            let query = search_input.get_value();
            update_editor_search(&editor, &query, &status, &state, false, text);
        });
    }
    {
        let editor = editor;
        let status = status;
        let state = Rc::clone(&state);
        close_search_button.on_click(move |_| {
            hide_search_bar(panel, search_bar, editor, status, &state, text);
        });
    }
}

fn handle_search_input_key(
    event: &WindowEventData,
    panel: Panel,
    search_bar: Panel,
    search_input: SearchCtrl,
    editor: StyledTextCtrl,
    status: StaticText,
    state: &Rc<RefCell<SearchState>>,
    text: GuiText,
) -> bool {
    let WindowEventData::Keyboard(key_event) = event else {
        return false;
    };
    let key_code = key_event.get_key_code().unwrap_or_default();
    if key_code == KEY_ESCAPE {
        hide_search_bar(panel, search_bar, editor, status, state, text);
        return true;
    }
    if key_code == KEY_F3 {
        let query = search_input.get_value();
        update_editor_search(
            &editor,
            &query,
            &status,
            state,
            key_event.shift_down(),
            text,
        );
        return true;
    }
    false
}

fn show_search_bar(
    panel: Panel,
    search_bar: Panel,
    search_input: SearchCtrl,
    editor: StyledTextCtrl,
    status: StaticText,
    state: &Rc<RefCell<SearchState>>,
    text: GuiText,
) {
    if !search_bar.is_shown() {
        search_bar.show(true);
        panel.layout();
    }
    let selected_text = strip_nul(&editor.get_selected_text()).trim().to_string();
    let query = if selected_text.is_empty() {
        state.borrow().query.clone()
    } else {
        selected_text
    };
    if !query.is_empty() && search_input.get_value() != query {
        search_input.set_value(&query);
    }
    status.set_label(search_idle_text(text));
    search_input.set_focus();
}

fn hide_search_bar(
    panel: Panel,
    search_bar: Panel,
    editor: StyledTextCtrl,
    status: StaticText,
    state: &Rc<RefCell<SearchState>>,
    text: GuiText,
) {
    clear_search_indicators(&editor);
    editor.set_selection(0, 0);
    status.set_label(search_idle_text(text));
    *state.borrow_mut() = SearchState::default();
    search_bar.hide();
    panel.layout();
    editor.set_focus();
}

fn search_idle_text(text: GuiText) -> &'static str {
    match text.locale {
        super::text::GuiLocale::ZhCn => {
            "输入关键词查找。Enter/下一个继续，Shift+F3/上一个返回，Esc 关闭。"
        }
        super::text::GuiLocale::EnUs => {
            "Type to find. Enter/Next continues, Shift+F3/Previous goes back, Esc closes."
        }
    }
}

fn search_prev_text(text: GuiText) -> &'static str {
    match text.locale {
        super::text::GuiLocale::ZhCn => "上一个",
        super::text::GuiLocale::EnUs => "Previous",
    }
}

fn search_next_text(text: GuiText) -> &'static str {
    match text.locale {
        super::text::GuiLocale::ZhCn => "下一个",
        super::text::GuiLocale::EnUs => "Next",
    }
}

fn search_close_text(text: GuiText) -> &'static str {
    match text.locale {
        super::text::GuiLocale::ZhCn => "关闭查找",
        super::text::GuiLocale::EnUs => "Close Find",
    }
}

fn search_result_text(text: GuiText, query: &str, current: usize, total: usize) -> String {
    match text.locale {
        super::text::GuiLocale::ZhCn => {
            format!("查找：{query}  {current}/{total}。F3 下一个，Shift+F3 上一个。")
        }
        super::text::GuiLocale::EnUs => {
            format!("Find: {query}  {current}/{total}. F3 next, Shift+F3 previous.")
        }
    }
}

fn search_start_position(editor: &StyledTextCtrl, backwards: bool, same_query: bool) -> i32 {
    if !same_query {
        return if backwards { editor.get_length() } else { 0 };
    }
    if backwards {
        editor.get_selection_start().saturating_sub(1)
    } else {
        editor.get_selection_end()
    }
}

fn mark_search_matches(editor: &StyledTextCtrl, query: &str) -> usize {
    let query_len = byte_len_to_i32(query.len());
    if query_len <= 0 {
        return 0;
    }

    let mut count = 0usize;
    let mut start = 0;
    let length = editor.get_length();
    while start <= length {
        let Some(position) = editor.find_text(start, length, query, FindFlags::None) else {
            break;
        };
        editor.set_indicator_current(INDICATOR_FIND_MATCH);
        editor.indicator_fill_range(position, query_len);
        count += 1;
        start = position.saturating_add(query_len.max(1));
    }
    count
}

fn search_match_index(
    editor: &StyledTextCtrl,
    query: &str,
    selected_position: i32,
    total: usize,
) -> usize {
    if total == 0 {
        return 0;
    }

    let query_len = byte_len_to_i32(query.len());
    let mut index = 0usize;
    let mut start = 0;
    let length = editor.get_length();
    while start <= length {
        let Some(position) = editor.find_text(start, length, query, FindFlags::None) else {
            break;
        };
        index += 1;
        if position == selected_position {
            return index;
        }
        start = position.saturating_add(query_len.max(1));
    }
    1
}

fn clear_search_indicators(editor: &StyledTextCtrl) {
    editor.set_indicator_current(INDICATOR_FIND_MATCH);
    editor.indicator_clear_range(0, editor.get_length());
}

fn format_json_pretty(value: &serde_json::Value) -> Option<String> {
    let mut output = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    value.serialize(&mut serializer).ok()?;
    String::from_utf8(output).ok()
}

fn add_text_tab(parent: &Notebook, label: &str, content: &str) {
    let panel = Panel::builder(parent).build();
    panel.set_background_color(theme::theme().bg_card);

    let editor = TextCtrl::builder(&panel)
        .with_style(TextCtrlStyle::MultiLine | TextCtrlStyle::ReadOnly | TextCtrlStyle::DontWrap)
        .build();
    editor.set_value(content);

    let sizer = BoxSizer::builder(Orientation::Vertical).build();
    sizer.add(&editor, 1, SizerFlag::Expand | SizerFlag::All, 0);
    panel.set_sizer(sizer, true);
    parent.add_page(&panel, label, false, None);
}

fn configure_json_editor(editor: &StyledTextCtrl, content: &str) {
    editor.set_read_only(false);
    editor.set_wrap_mode(WrapMode::None);
    editor.set_tab_width(4);
    editor.set_indent(4);
    editor.set_use_tabs(false);
    editor.set_margin_line_numbers(0, true);
    editor.set_margin_width(0, 52);
    editor.set_margin_width(1, 0);
    editor.set_margin_type_typed(FOLD_MARGIN, MarginType::Symbol);
    editor.set_margin_mask(FOLD_MARGIN, FOLD_MARKER_MASK);
    editor.set_margin_width(FOLD_MARGIN, 18);
    editor.set_margin_sensitive(FOLD_MARGIN, true);
    editor.set_fold_flags(FOLD_FLAG_LINE_BEFORE_CONTRACTED | FOLD_FLAG_LINE_AFTER_CONTRACTED);
    configure_fold_markers(editor);
    editor.set_indentation_guides_typed(IndentationGuide::LookBoth);
    editor.set_view_white_space_typed(WhiteSpaceView::Invisible);
    let t = theme::theme();
    editor.set_caret_line_visible(true);
    editor.set_caret_line_background(t.code_caret_line);

    editor.style_set_foreground(STYLE_JSON_DEFAULT, t.code_fg);
    editor.style_set_background(STYLE_JSON_DEFAULT, t.code_bg);
    editor.style_set_size(STYLE_JSON_DEFAULT, 10);
    editor.style_clear_all();
    editor.style_set_foreground(STYLE_JSON_KEY, t.code_key);
    editor.style_set_bold(STYLE_JSON_KEY, true);
    editor.style_set_foreground(STYLE_JSON_STRING, t.code_string);
    editor.style_set_foreground(STYLE_JSON_NUMBER, t.code_number);
    editor.style_set_foreground(STYLE_JSON_KEYWORD, t.code_keyword);
    editor.style_set_foreground(STYLE_JSON_PUNCT, t.code_punct);
    editor.style_set_foreground(STYLE_LINE_NUMBER, t.code_line_number);
    editor.style_set_background(STYLE_LINE_NUMBER, t.code_gutter_bg);
    editor.style_set_foreground(STYLE_INDENT_GUIDE, t.code_indent_guide);
    editor.style_set_background(STYLE_INDENT_GUIDE, t.code_bg);
    editor.indicator_set_style(INDICATOR_FIND_MATCH, IndicatorStyle::RoundBox);
    editor.indicator_set_foreground(INDICATOR_FIND_MATCH, t.code_find_match);
    editor.indicator_set_alpha(INDICATOR_FIND_MATCH, 80);
    editor.indicator_set_outline_alpha(INDICATOR_FIND_MATCH, 180);

    editor.set_text(content);
    apply_json_stc_highlight(editor, content);
    apply_json_stc_folding(editor, content);
    editor.empty_undo_buffer();
    editor.set_read_only(true);
}

fn configure_fold_markers(editor: &StyledTextCtrl) {
    let t = theme::theme();
    let marker_foreground = t.code_punct;
    let marker_background = t.code_gutter_bg;
    editor.marker_define_symbol(
        MARKER_FOLDER,
        MarkerSymbol::BoxPlus,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_OPEN,
        MarkerSymbol::BoxMinus,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_SUB,
        MarkerSymbol::VLine,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_END,
        MarkerSymbol::BoxPlusConnected,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_OPEN_MID,
        MarkerSymbol::BoxMinusConnected,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_MID_TAIL,
        MarkerSymbol::TCorner,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_TAIL,
        MarkerSymbol::LCorner,
        marker_foreground,
        marker_background,
    );
}

fn apply_json_stc_highlight(editor: &StyledTextCtrl, content: &str) {
    editor.start_styling(0);
    let bytes = content.as_bytes();
    let mut styled_until = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'"' => {
                let start = idx;
                idx += 1;
                let mut escaped = false;
                while idx < bytes.len() {
                    let byte = bytes[idx];
                    idx += 1;
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        break;
                    }
                }
                let end = idx;
                let mut probe = idx;
                while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
                    probe += 1;
                }
                let style = if probe < bytes.len() && bytes[probe] == b':' {
                    STYLE_JSON_KEY
                } else {
                    STYLE_JSON_STRING
                };
                set_json_style(editor, &mut styled_until, start, end, style);
            }
            b'-' | b'0'..=b'9' => {
                let start = idx;
                idx += 1;
                while idx < bytes.len()
                    && matches!(bytes[idx], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    idx += 1;
                }
                set_json_style(editor, &mut styled_until, start, idx, STYLE_JSON_NUMBER);
            }
            b't' | b'f' | b'n' => {
                let start = idx;
                idx += 1;
                while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
                    idx += 1;
                }
                set_json_style(editor, &mut styled_until, start, idx, STYLE_JSON_KEYWORD);
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                set_json_style(editor, &mut styled_until, idx, idx + 1, STYLE_JSON_PUNCT);
                idx += 1;
            }
            _ => idx += 1,
        }
    }
    if styled_until < bytes.len() {
        editor.set_styling(
            byte_len_to_i32(bytes.len() - styled_until),
            STYLE_JSON_DEFAULT,
        );
    }
}

fn set_json_style(
    editor: &StyledTextCtrl,
    styled_until: &mut usize,
    start: usize,
    end: usize,
    style: i32,
) {
    if start > *styled_until {
        editor.set_styling(byte_len_to_i32(start - *styled_until), STYLE_JSON_DEFAULT);
        *styled_until = start;
    }
    if end > start {
        editor.set_styling(byte_len_to_i32(end - start), style);
        *styled_until = end;
    }
}

fn apply_json_stc_folding(editor: &StyledTextCtrl, content: &str) {
    let mut depth = 0i32;
    for (line_index, line) in content.lines().enumerate() {
        let line_no = line_index as i32;
        let line_depth = if line_starts_json_close(line) {
            depth.saturating_sub(1)
        } else {
            depth
        };
        let header = line_has_json_fold_header(line);
        let mut level = FOLD_BASE + line_depth.max(0);
        if header {
            level |= FOLD_HEADER;
        }
        editor.set_fold_level(line_no, level);
        if header {
            editor.set_fold_expanded(line_no, true);
        }
        depth = json_depth_after_line(line, depth);
    }
}

fn toggle_editor_fold_at_position(editor: &StyledTextCtrl, position: i32) {
    let line = editor.line_from_position(position);
    toggle_editor_fold_at_line(editor, line);
}

fn toggle_editor_fold_at_line(editor: &StyledTextCtrl, line: i32) {
    if editor.get_fold_level(line) & FOLD_HEADER == 0 {
        return;
    }
    editor.toggle_fold(line);
}

fn line_starts_json_close(line: &str) -> bool {
    line.trim_start()
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'}' | b']'))
}

fn line_has_json_fold_header(line: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut last_structural = None;
    for byte in line.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'}' | b'[' | b']' => last_structural = Some(byte),
            _ => {}
        }
    }
    matches!(last_structural, Some(b'{' | b'['))
}

fn json_depth_after_line(line: &str, current_depth: i32) -> i32 {
    let mut depth = current_depth;
    let mut in_string = false;
    let mut escaped = false;
    for byte in line.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn byte_len_to_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn summary_text(log: &RequestLogItem) -> String {
    format!(
        "#{}   model={}   channel={}   protocol={}   stream={}   status={}   req_size={}   tokens={}   ttft={}   latency={}   created={}",
        log.id,
        log.model_id,
        log.channel,
        log.provider_type,
        if log.stream { "true" } else { "false" },
        log.status,
        format_bytes(log.upstream_request_body_bytes),
        format_tokens(log),
        format_duration(log.ttft_ms),
        format_duration(log.latency_ms),
        log.created_at
    )
}

fn format_tokens(log: &RequestLogItem) -> String {
    match (log.total_tokens, log.input_tokens, log.output_tokens) {
        (Some(total), Some(input), Some(output)) => format!("{total}/{input}/{output}"),
        (Some(total), _, _) => total.to_string(),
        _ => "-".to_string(),
    }
}

fn format_duration(ms: Option<i64>) -> String {
    ms.map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_bytes(bytes: Option<i64>) -> String {
    let Some(bytes) = bytes else {
        return "-".to_string();
    };
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
