use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use wxdragon::prelude::*;
use wxdragon::widgets::dataview::DataViewItemAttr;

use super::text::GuiText;
use super::theme::{self, Theme};

#[derive(Clone, Copy)]
pub(super) struct StatusPanel {
    pub(super) panel: Panel,
    pub(super) icon: StaticBitmap,
    pub(super) marker: StaticText,
    pub(super) title: StaticText,
    pub(super) state: StaticText,
    pub(super) detail: StaticText,
    pub(super) extra: BoxSizer,
    pub(super) icon_kind: StatusIconKind,
}

#[derive(Clone, Copy)]
pub(super) struct ImStatusPanel {
    pub(super) panel: Panel,
    pub(super) feishu: ImChannelRow,
    pub(super) telegram: ImChannelRow,
    pub(super) wechat: ImChannelRow,
}

#[derive(Clone, Copy)]
pub(super) struct ImChannelRow {
    pub(super) icon: StaticBitmap,
    pub(super) marker: StaticText,
    pub(super) name: StaticText,
    pub(super) state: StaticText,
    pub(super) detail: StaticText,
    pub(super) kind: ImChannelKind,
}

#[derive(Clone, Copy)]
pub(super) enum ImChannelKind {
    Feishu,
    Telegram,
    Wechat,
}

#[derive(Clone, Copy)]
pub(super) enum StatusIconKind {
    Service,
    Codex,
    VsCodeCodex,
    CodexCli,
}

#[derive(Clone, Copy)]
pub(super) enum ProviderLogoKind {
    OpenAi,
    DeepSeek,
    Anthropic,
    Zhipu,
}

#[derive(Clone, Copy)]
pub(super) enum LucideIconKind {
    Router,
    MessagesSquare,
    ScrollText,
}

pub(super) fn table_cell_attr(row: usize) -> Option<DataViewItemAttr> {
    let t = theme::theme();
    Some(
        DataViewItemAttr::new()
            .with_text_colour(
                t.ink_primary.r,
                t.ink_primary.g,
                t.ink_primary.b,
                t.ink_primary.a,
            )
            .with_bg_colour(
                table_row_colour(row).r,
                table_row_colour(row).g,
                table_row_colour(row).b,
                table_row_colour(row).a,
            ),
    )
}

pub(super) fn apply_dataview_theme(list: &DataViewCtrl) {
    let t = theme::theme();
    list.set_background_color(t.table_row);
    let _ = list.set_alternate_row_colour(&t.table_row_alt);
}

pub(super) fn dataview_table_style(vertical_rules: bool) -> DataViewStyle {
    let base = DataViewStyle::Single;
    if theme::theme().is_dark {
        base
    } else if vertical_rules {
        base | DataViewStyle::RowLines
            | DataViewStyle::HorizontalRules
            | DataViewStyle::VerticalRules
    } else {
        base | DataViewStyle::RowLines | DataViewStyle::HorizontalRules
    }
}

pub(super) fn apply_listctrl_theme(list: &ListCtrl) {
    let t = theme::theme();
    list.set_background_color(t.table_row);
    list.set_foreground_color(t.ink_primary);
}

pub(super) fn listctrl_report_style() -> ListCtrlStyle {
    if theme::theme().is_dark {
        ListCtrlStyle::Report
    } else {
        ListCtrlStyle::Report | ListCtrlStyle::HRules | ListCtrlStyle::VRules
    }
}

pub(super) fn apply_listctrl_row_theme(list: &ListCtrl, row: i64) {
    let t = theme::theme();
    list.set_item_background_colour(row, &table_row_colour(row as usize));
    list.set_item_text_colour(row, &t.ink_primary);
}

pub(super) fn apply_notebook_theme(notebook: &Notebook) {
    let t = theme::theme();
    notebook.set_background_color(t.bg_card_alt);
    notebook.set_foreground_color(t.ink_primary);
}

pub(super) fn apply_textctrl_theme(input: &TextCtrl) {
    let t = theme::theme();
    input.set_background_color(t.bg_muted);
    input.set_foreground_color(t.ink_primary);
}

fn table_row_colour(row: usize) -> Colour {
    let t = theme::theme();
    if row % 2 == 0 {
        t.table_row
    } else {
        t.table_row_alt
    }
}

fn flat_bordered_panel<W: WxWidget>(parent: &W, background: Colour) -> Panel {
    let panel = Panel::builder(parent)
        .with_style(PanelStyle::BorderNone)
        .build();
    panel.set_background_color(background);

    let panel_for_paint = panel;
    panel.on_paint(move |_| {
        let dc = PaintDC::new(&panel_for_paint);
        let size = panel_for_paint.get_client_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        dc.set_background(background);
        dc.clear();
        dc.set_pen(theme::theme().border, 1, PenStyle::Solid);
        dc.set_brush(background, BrushStyle::Transparent);
        dc.draw_rectangle(0, 0, width.saturating_sub(1), height.saturating_sub(1));
    });

    panel
}

/// Build a flat "card" section: a `bg_card` surface with a bold title header and
/// a content sizer for the caller's children. Replaces the dated etched
/// `StaticBox` group frames.
///
/// Returns `(card_panel, content_sizer)`. Parent children to `card_panel` and add
/// their layout to `content_sizer`; add `card_panel` itself to the page sizer.
pub(super) fn card_section<W: WxWidget>(parent: &W, title: &str) -> (Panel, BoxSizer) {
    let t = theme::theme();
    let card = flat_bordered_panel(parent, t.bg_card);

    let outer = BoxSizer::builder(Orientation::Vertical).build();
    let header = StaticText::builder(&card).with_label(title).build();
    header.set_foreground_color(t.ink_primary);
    header.set_font(&theme::font(theme::TextRole::Title));
    outer.add(
        &header,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        theme::SPACE_MD,
    );

    let content = BoxSizer::builder(Orientation::Vertical).build();
    outer.add_sizer(
        &content,
        1,
        SizerFlag::Expand | SizerFlag::All,
        theme::SPACE_XS,
    );
    card.set_sizer(outer, true);
    (card, content)
}

pub(super) fn status_panel<W: WxWidget>(
    parent: &W,
    title: &str,
    icon_kind: StatusIconKind,
    text: GuiText,
) -> StatusPanel {
    build_status_panel(parent, title, icon_kind, text, false)
}

pub(super) fn centered_status_panel<W: WxWidget>(
    parent: &W,
    title: &str,
    icon_kind: StatusIconKind,
    text: GuiText,
) -> StatusPanel {
    build_status_panel(parent, title, icon_kind, text, true)
}

fn build_status_panel<W: WxWidget>(
    parent: &W,
    title: &str,
    icon_kind: StatusIconKind,
    text: GuiText,
    center_content: bool,
) -> StatusPanel {
    let t = theme::theme();
    let panel = flat_bordered_panel(parent, t.bg_card);
    // Height fits three lines (title + state + detail) with headroom; the cards
    // split the row evenly via proportion in the parent column.
    panel.set_min_size(Size::new(230, 66));

    let row = BoxSizer::builder(Orientation::Horizontal).build();
    if center_content {
        row.add_stretch_spacer(1);
    } else {
        row.add_spacer(18);
    }
    let icon = StaticBitmap::builder(&panel)
        .with_bitmap(Some(status_icon_bitmap(icon_kind, 34)))
        .with_scale_mode(Some(ScaleMode::None))
        .with_size(Size::new(34, 34))
        .build();
    icon.set_min_size(Size::new(34, 34));
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
    marker.set_foreground_color(t.ink_muted);
    title_row.add(&marker, 0, SizerFlag::Right, 5);
    let title_label = StaticText::builder(&panel).with_label(title).build();
    title_label.set_foreground_color(t.ink_secondary);
    title_row.add(&title_label, 0, SizerFlag::Bottom, 0);
    text_col.add_sizer(&title_row, 0, SizerFlag::Bottom, 4);

    let state = StaticText::builder(&panel)
        .with_label(text.detecting())
        .build();
    state.set_foreground_color(t.ink_primary);
    text_col.add(&state, 0, SizerFlag::Bottom, 4);

    let detail = StaticText::builder(&panel).with_label("").build();
    detail.set_foreground_color(t.ink_muted);
    detail.wrap(250);
    text_col.add(&detail, 0, SizerFlag::Expand, 0);
    let extra = BoxSizer::builder(Orientation::Vertical).build();
    text_col.add_sizer(&extra, 0, SizerFlag::Expand, 0);
    text_col.add_stretch_spacer(1);

    row.add_sizer(&text_col, 1, SizerFlag::Expand, 0);
    if center_content {
        row.add_stretch_spacer(1);
    } else {
        row.add_spacer(18);
    }
    let outer = BoxSizer::builder(Orientation::Vertical).build();
    outer.add_sizer(&row, 1, SizerFlag::Expand | SizerFlag::All, 2);
    panel.set_sizer(outer, true);
    StatusPanel {
        panel,
        icon,
        marker,
        title: title_label,
        state,
        detail,
        extra,
        icon_kind,
    }
}

pub(super) fn im_status_panel<W: WxWidget>(parent: &W, text: GuiText) -> ImStatusPanel {
    let panel = Panel::builder(parent).build();
    panel.set_background_color(theme::theme().bg_app);
    panel.set_min_size(Size::new(260, 190));

    let sizer = BoxSizer::builder(Orientation::Vertical).build();
    let feishu = im_channel_row(
        &panel,
        &sizer,
        ImChannelKind::Feishu,
        text.feishu_label(),
        text,
    );
    let telegram = im_channel_row(&panel, &sizer, ImChannelKind::Telegram, "Telegram", text);
    let wechat = im_channel_row(
        &panel,
        &sizer,
        ImChannelKind::Wechat,
        text.wechat_label(),
        text,
    );

    panel.set_sizer(sizer, true);
    ImStatusPanel {
        panel,
        feishu,
        telegram,
        wechat,
    }
}

pub(super) fn im_channel_row(
    parent: &Panel,
    parent_sizer: &BoxSizer,
    kind: ImChannelKind,
    name: &str,
    text: GuiText,
) -> ImChannelRow {
    let t = theme::theme();
    let row_panel = flat_bordered_panel(parent, t.bg_card);
    row_panel.set_min_size(Size::new(250, 58));
    let row = BoxSizer::builder(Orientation::Horizontal).build();

    let icon = StaticBitmap::builder(&row_panel)
        .with_bitmap(Some(im_channel_icon_bitmap(kind, false, 24)))
        .with_scale_mode(Some(ScaleMode::AspectFit))
        .with_size(Size::new(24, 24))
        .build();
    icon.set_min_size(Size::new(24, 24));
    row.add_spacer(14);
    row.add(
        &icon,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        12,
    );

    let text_col = BoxSizer::builder(Orientation::Vertical).build();
    let title_row = BoxSizer::builder(Orientation::Horizontal).build();
    let marker = StaticText::builder(&row_panel).with_label("●").build();
    marker.set_foreground_color(t.ink_muted);
    title_row.add(&marker, 0, SizerFlag::Right, 5);

    let name_label = StaticText::builder(&row_panel).with_label(name).build();
    name_label.set_foreground_color(t.ink_secondary);
    title_row.add(&name_label, 0, SizerFlag::Right, 8);

    let state = StaticText::builder(&row_panel)
        .with_label(text.detecting())
        .build();
    state.set_foreground_color(t.ink_muted);
    title_row.add(&state, 0, SizerFlag::Right, 0);
    text_col.add_sizer(&title_row, 0, SizerFlag::Bottom, 2);

    let detail = StaticText::builder(&row_panel).with_label("").build();
    detail.set_foreground_color(t.ink_muted);
    detail.wrap(220);
    text_col.add(&detail, 0, SizerFlag::Expand, 0);

    row.add_sizer(&text_col, 1, SizerFlag::Expand, 0);
    row.add_spacer(12);
    let outer = BoxSizer::builder(Orientation::Vertical).build();
    outer.add_sizer(&row, 1, SizerFlag::Expand | SizerFlag::All, 3);
    row_panel.set_sizer(outer, true);
    parent_sizer.add(&row_panel, 1, SizerFlag::Expand, 0);

    ImChannelRow {
        icon,
        marker,
        name: name_label,
        state,
        detail,
        kind,
    }
}

pub(super) fn topology_connector<W: WxWidget>(parent: &W) -> StaticBitmap {
    let bitmap = topology_connector_bitmap(72, 190);
    let connector = StaticBitmap::builder(parent)
        .with_bitmap(Some(bitmap))
        .with_scale_mode(Some(ScaleMode::None))
        .with_size(Size::new(72, 190))
        .build();
    connector.set_min_size(Size::new(72, 190));
    connector
}

pub(super) fn topology_splitter<W: WxWidget>(parent: &W) -> StaticBitmap {
    let bitmap = topology_splitter_bitmap(72, 190);
    let splitter = StaticBitmap::builder(parent)
        .with_bitmap(Some(bitmap))
        .with_scale_mode(Some(ScaleMode::None))
        .with_size(Size::new(72, 190))
        .build();
    splitter.set_min_size(Size::new(72, 190));
    splitter
}

/// Alpha-composite `fg` over the opaque `bg` and return an opaque RGBA tuple.
/// Used so self-drawn topology bitmaps contain no translucent pixels.
fn blend_over(fg: Colour, bg: Colour, alpha: u8) -> [u8; 4] {
    let a = alpha as u16;
    let inv = 255 - a;
    let mix = |f: u8, b: u8| (((f as u16 * a) + (b as u16 * inv)) / 255) as u8;
    [mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b), 255]
}

pub(super) fn topology_connector_bitmap(width: usize, height: usize) -> Bitmap {
    // Composite the semi-transparent line colour onto the (opaque) card
    // background so the produced bitmap is fully opaque. A bitmap with
    // translucent or transparent pixels makes Windows StaticBitmap skip
    // erasing the parent background on redraw, layering successive frames
    // into a "ghost" of the vertical trunk line.
    let t = theme::theme();
    let mut canvas = IconCanvas::new_with_size(width, height, Theme::rgba(t.bg_card, 255));
    let colour = blend_over(t.divider, t.bg_card, 210);
    let trunk_x = 30usize;
    let top_y = 33usize;
    let mid_y = height / 2;
    let bottom_y = height.saturating_sub(33);
    canvas.draw_line(0, top_y, trunk_x, top_y, 2, colour);
    canvas.draw_line(0, bottom_y, trunk_x, bottom_y, 2, colour);
    canvas.draw_line(trunk_x, top_y, trunk_x, bottom_y, 2, colour);
    canvas.draw_line(trunk_x, mid_y, width.saturating_sub(1), mid_y, 2, colour);
    Bitmap::from_rgba(&canvas.rgba, width as u32, height as u32).expect("topology connector bitmap")
}

pub(super) fn topology_splitter_bitmap(width: usize, height: usize) -> Bitmap {
    let t = theme::theme();
    let mut canvas = IconCanvas::new_with_size(width, height, Theme::rgba(t.bg_card, 255));
    let colour = blend_over(t.divider, t.bg_card, 210);
    let trunk_x = 34usize;
    let top_y = 31usize;
    let mid_y = height / 2;
    let bottom_y = height.saturating_sub(31);
    canvas.draw_line(0, mid_y, trunk_x, mid_y, 2, colour);
    canvas.draw_line(trunk_x, top_y, trunk_x, bottom_y, 2, colour);
    canvas.draw_line(trunk_x, top_y, width.saturating_sub(1), top_y, 2, colour);
    canvas.draw_line(trunk_x, mid_y, width.saturating_sub(1), mid_y, 2, colour);
    canvas.draw_line(
        trunk_x,
        bottom_y,
        width.saturating_sub(1),
        bottom_y,
        2,
        colour,
    );
    Bitmap::from_rgba(&canvas.rgba, width as u32, height as u32).expect("topology splitter bitmap")
}

pub(super) fn status_icon_bitmap(kind: StatusIconKind, size: usize) -> Bitmap {
    match kind {
        StatusIconKind::Codex => {
            return svg_brand_bitmap(
                "codex.svg",
                include_bytes!("../../packaging/brand/codex.svg"),
                size,
            );
        }
        StatusIconKind::VsCodeCodex => {
            return svg_brand_bitmap(
                "vscode-logo.svg",
                include_bytes!("../../packaging/brand/vscode-logo.svg"),
                size,
            );
        }
        StatusIconKind::CodexCli => {
            return svg_brand_bitmap(
                "codex-cli.svg",
                include_bytes!("../../packaging/brand/codex-cli.svg"),
                size,
            );
        }
        StatusIconKind::Service => {
            return svg_brand_bitmap(
                "service-server.svg",
                include_bytes!("../../packaging/brand/service-server.svg"),
                size,
            );
        }
    }
}

pub(super) fn disabled_status_icon_bitmap(kind: StatusIconKind, size: usize) -> Bitmap {
    match kind {
        StatusIconKind::Codex => {
            return disabled_svg_brand_bitmap(
                "codex.svg",
                include_bytes!("../../packaging/brand/codex.svg"),
                size,
            );
        }
        StatusIconKind::VsCodeCodex => {
            return disabled_svg_brand_bitmap(
                "vscode-logo.svg",
                include_bytes!("../../packaging/brand/vscode-logo.svg"),
                size,
            );
        }
        StatusIconKind::CodexCli => {
            return disabled_svg_brand_bitmap(
                "codex-cli.svg",
                include_bytes!("../../packaging/brand/codex-cli.svg"),
                size,
            );
        }
        StatusIconKind::Service => {
            return disabled_svg_brand_bitmap(
                "service-server.svg",
                include_bytes!("../../packaging/brand/service-server.svg"),
                size,
            );
        }
    }
}

pub(super) fn app_icon_bitmap(size: usize) -> Bitmap {
    png_brand_bitmap(
        "dolphin-rounded-256.png",
        include_bytes!("../../packaging/icons/dolphin-rounded-256.png"),
        size,
    )
}

pub(super) fn im_channel_icon_bitmap(kind: ImChannelKind, disabled: bool, size: usize) -> Bitmap {
    match kind {
        ImChannelKind::Feishu => {
            if disabled {
                disabled_png_brand_bitmap(
                    "feishu-logo.png",
                    include_bytes!("../../packaging/brand/feishu-logo.png"),
                    size,
                )
            } else {
                png_brand_bitmap(
                    "feishu-logo.png",
                    include_bytes!("../../packaging/brand/feishu-logo.png"),
                    size,
                )
            }
        }
        ImChannelKind::Telegram => {
            if disabled {
                disabled_svg_brand_bitmap(
                    "telegram-logo.svg",
                    include_bytes!("../../packaging/brand/telegram-logo.svg"),
                    size,
                )
            } else {
                svg_brand_bitmap(
                    "telegram-logo.svg",
                    include_bytes!("../../packaging/brand/telegram-logo.svg"),
                    size,
                )
            }
        }
        ImChannelKind::Wechat => {
            if disabled {
                disabled_svg_brand_bitmap(
                    "wechat-logo.svg",
                    include_bytes!("../../packaging/brand/wechat-logo.svg"),
                    size,
                )
            } else {
                svg_brand_bitmap(
                    "wechat-logo.svg",
                    include_bytes!("../../packaging/brand/wechat-logo.svg"),
                    size,
                )
            }
        }
    }
}

pub(super) fn provider_logo_bitmap(kind: ProviderLogoKind, size: i32) -> Bitmap {
    let (file_name, bytes) = match kind {
        ProviderLogoKind::OpenAi => (
            "openai.svg",
            include_bytes!("../../packaging/brand/providers/openai.svg").as_slice(),
        ),
        ProviderLogoKind::DeepSeek => (
            "deepseek.svg",
            include_bytes!("../../packaging/brand/providers/deepseek.svg").as_slice(),
        ),
        ProviderLogoKind::Anthropic => (
            "anthropic.svg",
            include_bytes!("../../packaging/brand/providers/anthropic.svg").as_slice(),
        ),
        ProviderLogoKind::Zhipu => (
            "zhipu.svg",
            include_bytes!("../../packaging/brand/providers/zhipu.svg").as_slice(),
        ),
    };
    let bitmap = BitmapBundle::from_svg_data(bytes, Size::new(size, size))
        .and_then(|bundle| bundle.get_bitmap(Size::new(size, size)))
        .unwrap_or_else(|| panic!("failed to load provider logo {file_name}"));
    if theme::theme().is_dark {
        recolor_dark_provider_logo(&bitmap, file_name)
    } else {
        bitmap
    }
}

pub(super) fn lucide_icon_bitmap(kind: LucideIconKind, size: usize) -> Bitmap {
    let (file_name, bytes) = match kind {
        LucideIconKind::Router => (
            "router.svg",
            include_bytes!("../../packaging/brand/lucide/router.svg").as_slice(),
        ),
        LucideIconKind::MessagesSquare => (
            "messages-square.svg",
            include_bytes!("../../packaging/brand/lucide/messages-square.svg").as_slice(),
        ),
        LucideIconKind::ScrollText => (
            "scroll-text.svg",
            include_bytes!("../../packaging/brand/lucide/scroll-text.svg").as_slice(),
        ),
    };
    let t = theme::theme();
    let color = if t.is_dark {
        t.ink_primary
    } else {
        t.ink_secondary
    };
    let stroke = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
    let svg = std::str::from_utf8(bytes)
        .unwrap_or_else(|err| panic!("failed to parse lucide svg {file_name}: {err}"))
        .replace("stroke=\"currentColor\"", &format!("stroke=\"{stroke}\""));
    let size = size as i32;
    BitmapBundle::from_svg_data(svg.as_bytes(), Size::new(size, size))
        .and_then(|bundle| bundle.get_bitmap(Size::new(size, size)))
        .unwrap_or_else(|| panic!("failed to load lucide icon {file_name}"))
}

pub(super) fn svg_brand_bitmap(file_name: &str, bytes: &[u8], size: usize) -> Bitmap {
    let size = size as i32;
    BitmapBundle::from_svg_data(bytes, Size::new(size, size))
        .and_then(|bundle| bundle.get_bitmap(Size::new(size, size)))
        .unwrap_or_else(|| panic!("failed to load brand svg {file_name}"))
}

pub(super) fn disabled_svg_brand_bitmap(file_name: &str, bytes: &[u8], size: usize) -> Bitmap {
    let bitmap = svg_brand_bitmap(file_name, bytes, size);
    disabled_bitmap(&bitmap, file_name)
}

pub(super) fn png_brand_bitmap(file_name: &str, bytes: &[u8], size: usize) -> Bitmap {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .unwrap_or_else(|err| panic!("failed to load brand image {file_name}: {err}"))
        .resize(size as u32, size as u32, FilterType::Lanczos3)
        .into_rgba8();
    let (width, height) = image.dimensions();
    Bitmap::from_rgba(image.as_raw(), width, height)
        .unwrap_or_else(|| panic!("failed to create bitmap from {file_name}"))
}

fn recolor_dark_provider_logo(bitmap: &Bitmap, file_name: &str) -> Bitmap {
    let width = bitmap.get_width() as u32;
    let height = bitmap.get_height() as u32;
    let rgba = bitmap
        .get_rgba_data()
        .unwrap_or_else(|| panic!("failed to read provider logo data from {file_name}"));
    let mut image = RgbaImage::from_raw(width, height, rgba)
        .unwrap_or_else(|| panic!("failed to decode provider logo data from {file_name}"));
    let tint = theme::theme().ink_primary;

    for pixel in image.pixels_mut() {
        let alpha = pixel[3];
        if alpha == 0 {
            continue;
        }
        let luminance = (pixel[0] as u16 * 30 + pixel[1] as u16 * 59 + pixel[2] as u16 * 11) / 100;
        if luminance < 96 {
            *pixel = Rgba([tint.r, tint.g, tint.b, alpha]);
        }
    }

    Bitmap::from_rgba(image.as_raw(), width, height)
        .unwrap_or_else(|| panic!("failed to recolor provider logo {file_name}"))
}

pub(super) fn disabled_png_brand_bitmap(file_name: &str, bytes: &[u8], size: usize) -> Bitmap {
    let mut image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .unwrap_or_else(|err| panic!("failed to load brand image {file_name}: {err}"))
        .resize(size as u32, size as u32, FilterType::Lanczos3)
        .into_rgba8();
    for pixel in image.pixels_mut() {
        soften_disabled_pixel(&mut pixel.0);
    }
    let (width, height) = image.dimensions();
    Bitmap::from_rgba(image.as_raw(), width, height)
        .unwrap_or_else(|| panic!("failed to create disabled bitmap from {file_name}"))
}

fn disabled_bitmap(bitmap: &Bitmap, file_name: &str) -> Bitmap {
    let width = bitmap.get_width() as u32;
    let height = bitmap.get_height() as u32;
    let mut rgba = bitmap
        .get_rgba_data()
        .unwrap_or_else(|| panic!("failed to read bitmap data from {file_name}"));
    for pixel in rgba.chunks_exact_mut(4) {
        soften_disabled_pixel(pixel);
    }
    Bitmap::from_rgba(&rgba, width, height)
        .unwrap_or_else(|| panic!("failed to create disabled bitmap from {file_name}"))
}

fn soften_disabled_pixel(pixel: &mut [u8]) {
    let alpha = pixel[3];
    if alpha == 0 {
        return;
    }
    let gray = ((pixel[0] as u16 * 30 + pixel[1] as u16 * 59 + pixel[2] as u16 * 11) / 100) as u8;
    let soft = (gray as u16 + 180) / 2;
    pixel[0] = soft as u8;
    pixel[1] = soft as u8;
    pixel[2] = soft as u8;
    pixel[3] = ((alpha as u16 * 50) / 100) as u8;
}

pub(super) struct IconCanvas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl IconCanvas {
    fn new_with_size(width: usize, height: usize, background: [u8; 4]) -> Self {
        let mut rgba = vec![0; width * height * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&background);
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
        for yy in y..(y + height).min(self.height) {
            for xx in x..(x + width).min(self.width) {
                self.set_pixel(xx, yy, color);
            }
        }
    }

    fn draw_line(
        &mut self,
        x1: usize,
        y1: usize,
        x2: usize,
        y2: usize,
        thickness: usize,
        color: [u8; 4],
    ) {
        if y1 == y2 {
            let start = x1.min(x2);
            let end = x1.max(x2);
            let y = y1.saturating_sub(thickness / 2);
            self.fill_rect(start, y, end - start + 1, thickness, color);
        } else if x1 == x2 {
            let start = y1.min(y2);
            let end = y1.max(y2);
            let x = x1.saturating_sub(thickness / 2);
            self.fill_rect(x, start, thickness, end - start + 1, color);
        }
    }

    fn set_pixel(&mut self, x: usize, y: usize, color: [u8; 4]) {
        let offset = (y * self.width + x) * 4;
        self.rgba[offset..offset + 4].copy_from_slice(&color);
    }
}

pub(super) fn text_field_row<W: WxWidget>(
    parent: &W,
    sizer: &FlexGridSizer,
    label: &str,
    value: &str,
) -> TextCtrl {
    let label_widget = StaticText::builder(parent).with_label(label).build();
    label_widget.set_foreground_color(theme::theme().ink_secondary);
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
    apply_textctrl_theme(&input);
    input.set_min_size(Size::new(420, 30));
    sizer.add(&input, 1, SizerFlag::Expand, 0);
    input
}

#[derive(Clone, Copy)]
pub(super) enum StateTone {
    Ok,
    Warn,
    Error,
    Muted,
}

impl StateTone {
    fn colour(self) -> Colour {
        let t = theme::theme();
        match self {
            StateTone::Ok => t.ok,
            StateTone::Warn => t.warn,
            StateTone::Error => t.error,
            StateTone::Muted => t.ink_muted,
        }
    }
}

pub(super) fn set_status_panel(panel: &StatusPanel, state: &str, detail: &str, tone: StateTone) {
    let t = theme::theme();
    let title_colour = t.ink_secondary;
    let tone_colour = tone.colour();
    if panel.state.get_label() == state
        && panel.detail.get_label() == detail
        && panel.marker.get_foreground_color() == tone_colour
        && panel.state.get_foreground_color() == tone_colour
    {
        return;
    }

    panel.panel.set_background_color(t.bg_card);
    if panel.title.get_foreground_color() != title_colour {
        panel
            .icon
            .set_bitmap(&status_icon_bitmap(panel.icon_kind, 34));
    }
    panel.title.set_foreground_color(title_colour);
    panel.marker.set_foreground_color(tone_colour);
    panel.state.set_label(state);
    panel.state.set_foreground_color(tone_colour);
    panel.detail.set_label(detail);
    panel.detail.set_foreground_color(t.ink_muted);
    panel.detail.wrap(220);
    panel.panel.layout();
    panel.panel.refresh(true, None);
    panel.panel.update();
}

pub(super) fn set_im_channel_row(row: &ImChannelRow, state: &str, detail: &str, tone: StateTone) {
    if row.state.get_label() == state && row.detail.get_label() == detail {
        return;
    }

    let t = theme::theme();
    let muted = matches!(tone, StateTone::Muted);
    let name_colour = if muted { t.ink_muted } else { t.ink_secondary };
    row.icon
        .set_bitmap(&im_channel_icon_bitmap(row.kind, muted, 24));
    row.name.set_foreground_color(name_colour);
    row.marker.set_foreground_color(tone.colour());
    row.state.set_label(state);
    row.state.set_foreground_color(tone.colour());
    row.detail.set_label(detail);
    row.detail.set_foreground_color(t.ink_muted);
    row.detail.wrap(220);
}

pub(super) fn set_disabled_status_panel(panel: &StatusPanel, state: &str, detail: &str) {
    if panel.state.get_label() == state && panel.detail.get_label() == detail {
        return;
    }

    let muted = theme::theme().ink_muted;
    panel.panel.set_background_color(theme::theme().bg_muted);
    if panel.title.get_foreground_color() != muted {
        panel
            .icon
            .set_bitmap(&disabled_status_icon_bitmap(panel.icon_kind, 34));
    }
    panel.title.set_foreground_color(muted);
    panel.marker.set_foreground_color(muted);
    panel.state.set_label(state);
    panel.state.set_foreground_color(muted);
    panel.detail.set_label(detail);
    panel.detail.set_foreground_color(muted);
    panel.detail.wrap(190);
}
