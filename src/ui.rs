//! egui chrome: tab bar, command palette, settings, and SSH prompts.

use crate::agent::server::{ConsentAnswer, ConsentKind, ConsentRequest};
use crate::config::{AuthMethod, BackgroundFit, Config, CursorShape, SshProfile};
use crate::term::SshRequest;
use crate::theme::Theme;
use std::sync::mpsc::Sender;

/// Height of the compact application chrome. This leaves room for the tabs
/// and controls without stealing meaningful space from the terminal.
pub const BAR_H: f32 = 44.0;

/// Modal overlay state.
///
/// The settings variant is much larger than the others, but only one overlay is
/// ever alive at a time, so boxing it would only add indirection.
#[allow(clippy::large_enum_variant)]
pub enum Overlay {
    None,
    Palette {
        query: String,
        selected: usize,
    },
    Settings {
        draft: Config,
        font: String,
        error: Option<String>,
        /// Trusted host keys, loaded when the modal opens.
        known_hosts: Vec<crate::config::known_hosts::Entry>,
        /// Config as it was when the modal opened, so Cancel/Esc can revert
        /// the live preview.
        original: Config,
        original_font: String,
    },
    /// Ad-hoc `user@host[:port]` connection.
    Connect {
        text: String,
        error: Option<String>,
    },
    /// Ask for an image file to look at.
    ViewImage {
        text: String,
        error: Option<String>,
    },
    /// An image being looked at, already decoded onto the GPU.
    Image {
        path: std::path::PathBuf,
        texture: egui::TextureHandle,
    },
    /// Work is still running where a close was asked for: confirm first.
    CloseConfirm {
        scope: CloseScope,
        jobs: Vec<String>,
    },
    /// Rename a tab.
    Rename {
        tab: usize,
        text: String,
    },
    /// A multi-line paste that would run immediately: confirm first.
    PasteConfirm {
        text: String,
    },
    /// Edit an SSH profile. `original` is the name the profile had when the
    /// editor opened (`None` for a new host): it is what makes a rename edit
    /// the profile in place and lets the editor delete it.
    Profile {
        profile: SshProfile,
        original: Option<String>,
        error: Option<String>,
    },
    /// Edit the editor's keybindings.
    Keybindings {
        /// The bindings being edited: command name -> sequence.
        bindings: std::collections::BTreeMap<String, String>,
        /// The command whose binding is being recorded, if any.
        recording: Option<String>,
        error: Option<String>,
    },
    /// A newer release is out: offer to install it.
    Update {
        version: String,
    },
}

/// What a close confirmation is about to close.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CloseScope {
    Pane {
        tab: usize,
        pane: usize,
    },
    Tab(usize),
    /// The whole window.
    App,
}

/// Where an SSH question came from. Panes resolve their profile through
/// the tab; broker sessions carry the profile name with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptSource {
    Tab(usize),
    Agent,
}

/// An interactive SSH question awaiting the user's answer.
pub struct SshPrompt {
    pub source: PromptSource,
    pub kind: PromptKind,
    pub reply: Sender<Result<String, ()>>,
}

pub enum PromptKind {
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
    },
    Password {
        prompt: String,
        remember: bool,
        profile: Option<String>,
    },
    Keyboard {
        prompt: String,
    },
    Passphrase {
        prompt: String,
        remember: bool,
        profile: Option<String>,
    },
}

/// Apply the theme to egui's global style (chrome only).
pub fn apply_theme(ctx: &egui::Context, theme: &Theme) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let mut visuals = if theme.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let ui = &theme.ui;
    let alpha = theme.ui_alpha.clamp(0.0, 1.0);
    visuals.panel_fill = with_alpha(color32(ui.panel_bg), alpha);
    visuals.window_fill = with_alpha(color32(ui.panel_bg), alpha);
    visuals.extreme_bg_color = with_alpha(color32(ui.bar_bg), alpha);
    visuals.faint_bg_color = with_alpha(color32(ui.tab_hover_bg), alpha);
    visuals.selection.bg_fill = color32(ui.accent);
    visuals.selection.stroke = egui::Stroke::new(1.0, color32(ui.accent));
    visuals.window_stroke = egui::Stroke::new(1.0, color32(ui.border));
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    visuals.menu_corner_radius = egui::CornerRadius::same(8);
    visuals.window_shadow = soft_shadow(12, 32, 170);
    visuals.popup_shadow = soft_shadow(10, 26, 150);
    // Website-style hand cursor over everything clickable.
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, color32(ui.border));
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0, color32(ui.border));
    widgets.inactive.weak_bg_fill = with_alpha(color32(ui.tab_hover_bg), alpha);
    widgets.inactive.fg_stroke = egui::Stroke::new(1.0, color32(ui.text));
    widgets.hovered.bg_stroke = egui::Stroke::new(1.0, color32(ui.accent));
    widgets.hovered.weak_bg_fill = with_alpha(
        mix(color32(ui.tab_hover_bg), color32(ui.accent), 0.20),
        alpha,
    );
    widgets.hovered.fg_stroke = egui::Stroke::new(1.0, color32(ui.text));
    widgets.active.bg_stroke = egui::Stroke::new(1.0, color32(ui.accent));
    widgets.active.weak_bg_fill = with_alpha(
        mix(color32(ui.tab_hover_bg), color32(ui.accent), 0.34),
        alpha,
    );
    widgets.active.fg_stroke = egui::Stroke::new(1.0, color32(ui.text));
    for w in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(6);
    }

    style.visuals = visuals;
    style.animation_time = 0.14;
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.interact_size.y = 26.0;
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

pub fn color32(c: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(c[0], c[1], c[2])
}

/// A soft drop shadow, tuned for the dark chrome.
pub fn soft_shadow(offset_y: i8, blur: u8, alpha: u8) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, offset_y],
        blur,
        spread: 0,
        color: egui::Color32::from_black_alpha(alpha),
    }
}

/// Linear blend between two colors (`t` clamped to 0..=1).
pub fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Same color with a different alpha.
pub fn with_alpha(c: egui::Color32, a: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        c.r(),
        c.g(),
        c.b(),
        (a.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Multiply an RGB triple by `f` (for pressed states).
pub fn shade(c: [u8; 3], f: f32) -> egui::Color32 {
    let s = |v: u8| (v as f32 * f).clamp(0.0, 255.0).round() as u8;
    egui::Color32::from_rgb(s(c[0]), s(c[1]), s(c[2]))
}

/// Readable text color on top of `bg`.
pub fn on_color(bg: [u8; 3]) -> egui::Color32 {
    let l = 0.2126 * bg[0] as f32 + 0.7152 * bg[1] as f32 + 0.0722 * bg[2] as f32;
    if l > 145.0 {
        egui::Color32::from_rgb(0x0d, 0x11, 0x1a)
    } else {
        egui::Color32::from_rgb(0xf5, 0xf7, 0xfa)
    }
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// Small, hand-drawn chrome icons. Using strokes instead of a glyph avoids
/// depending on whichever emoji or symbol font happens to be installed.
#[derive(Clone, Copy)]
pub enum Icon {
    Add,
    Settings,
    /// A page with a folded corner: the editor.
    Editor,
    /// A terminal prompt: the SSH workspace.
    Ssh,
}

/// Square icon button: rounded hover background, hand cursor, tooltip.
pub fn icon_button(ui: &mut egui::Ui, theme: &Theme, icon: Icon, tooltip: &str) -> egui::Response {
    icon_button_tinted(ui, theme, icon, tooltip, false)
}

/// An icon button that stays lit while its mode is on.
pub fn icon_button_tinted(
    ui: &mut egui::Ui,
    theme: &Theme,
    icon: Icon,
    tooltip: &str,
    active: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let painter = ui.painter_at(rect);
    if active {
        painter.rect_filled(
            rect.shrink(1.0),
            8.0,
            with_alpha(color32(theme.ui.accent), 0.22),
        );
    } else if hover > 0.0 {
        painter.rect_filled(
            rect.shrink(1.0),
            8.0,
            with_alpha(color32(theme.ui.tab_hover_bg), 0.96 * hover),
        );
    }
    let fg = if active {
        color32(theme.ui.accent)
    } else {
        mix(
            color32(theme.ui.tab_inactive_fg),
            color32(theme.ui.text),
            hover,
        )
    };
    paint_icon(&painter, rect.center(), icon, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

fn paint_icon(painter: &egui::Painter, center: egui::Pos2, icon: Icon, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.7, color);
    match icon {
        Icon::Add => {
            painter.line_segment(
                [center - egui::vec2(6.0, 0.0), center + egui::vec2(6.0, 0.0)],
                stroke,
            );
            painter.line_segment(
                [center - egui::vec2(0.0, 6.0), center + egui::vec2(0.0, 6.0)],
                stroke,
            );
        }
        // A restrained tune icon is clearer than a platform-dependent gear
        // glyph at this size.
        Icon::Settings => {
            for (y, knob_x) in [(-5.0, 3.0), (0.0, -3.0), (5.0, 1.0)] {
                painter.line_segment(
                    [center + egui::vec2(-6.5, y), center + egui::vec2(6.5, y)],
                    stroke,
                );
                painter.circle_filled(center + egui::vec2(knob_x, y), 2.15, color);
            }
        }
        // A page with a folded corner.
        Icon::Editor => {
            let left = center.x - 5.5;
            let right = center.x + 5.5;
            let top = center.y - 6.5;
            let bottom = center.y + 6.5;
            let fold = 4.0;
            painter.line_segment(
                [egui::pos2(left, top), egui::pos2(right - fold, top)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(right - fold, top), egui::pos2(right, top + fold)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(right, top + fold), egui::pos2(right, bottom)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(right, bottom), egui::pos2(left, bottom)],
                stroke,
            );
            painter.line_segment([egui::pos2(left, bottom), egui::pos2(left, top)], stroke);
            // Two text lines.
            for y in [0.0, 4.0] {
                painter.line_segment(
                    [
                        egui::pos2(left + 2.5, center.y - 1.0 + y),
                        egui::pos2(right - 2.5, center.y - 1.0 + y),
                    ],
                    egui::Stroke::new(1.2, color),
                );
            }
        }
        // A prompt chevron and a cursor.
        Icon::Ssh => {
            painter.line_segment(
                [
                    center + egui::vec2(-5.5, -4.5),
                    center + egui::vec2(-1.5, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-1.5, 0.0),
                    center + egui::vec2(-5.5, 4.5),
                ],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(0.5, 4.5), center + egui::vec2(6.0, 4.5)],
                stroke,
            );
        }
    }
}

/// Filled accent button for the primary action of a dialog.
pub fn primary_button(ui: &mut egui::Ui, theme: &Theme, text: &str, min_w: f32) -> egui::Response {
    let fg = on_color(theme.ui.accent);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        fg,
    );
    let size = egui::vec2((galley.size().x + 30.0).max(min_w), 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let fill = if resp.is_pointer_button_down_on() {
        shade(theme.ui.accent, 0.82)
    } else {
        mix(color32(theme.ui.accent), egui::Color32::WHITE, 0.12 * hover)
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, fill);
    painter.galley(rect.center() - galley.size() * 0.5, galley, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Outlined button for the secondary action of a dialog.
pub fn secondary_button(
    ui: &mut egui::Ui,
    theme: &Theme,
    text: &str,
    min_w: f32,
) -> egui::Response {
    let fg = color32(theme.ui.text);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        fg,
    );
    let size = egui::vec2((galley.size().x + 30.0).max(min_w), 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        6.0,
        with_alpha(color32(theme.ui.tab_hover_bg), 0.55 + 0.45 * hover),
    );
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(
            1.0,
            mix(color32(theme.ui.border), color32(theme.ui.accent), hover),
        ),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() * 0.5, galley, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Small-caps section header with a hairline rule underneath.
pub fn section_header(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(text)
            .size(10.5)
            .strong()
            .color(color32(theme.ui.accent)),
    );
    ui.add_space(4.0);
    hairline(ui, theme);
    ui.add_space(6.0);
}

/// A 1px rule spanning the available width.
pub fn hairline(ui: &mut egui::Ui, theme: &Theme) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, color32(theme.ui.border));
}

// ---------------------------------------------------------------------------
// Modal frame helper
// ---------------------------------------------------------------------------

fn modal_frame(ui: &egui::Ui, theme: &Theme, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Area::new(egui::Id::new(format!("modal:{title}")))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -20.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(with_alpha(color32(theme.ui.panel_bg), theme.ui_alpha))
                .stroke(egui::Stroke::new(1.0, color32(theme.ui.border)))
                .corner_radius(10.0)
                .shadow(soft_shadow(14, 36, 180))
                .inner_margin(egui::Margin::same(18))
                .show(ui, |ui| {
                    ui.set_min_width(440.0);
                    ui.set_max_width(560.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(title)
                                .size(15.0)
                                .strong()
                                .color(color32(theme.ui.text)),
                        );
                    });
                    ui.add_space(8.0);
                    hairline(ui, theme);
                    ui.add_space(10.0);
                    add(ui);
                });
        });
}

// ---------------------------------------------------------------------------
// Command palette
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum PaletteAction {
    NewTerminal,
    Connect(usize), // profile index
    ConnectAdhoc,
    CloseTab,
    Settings,
    Quit,
    /// System info panel (`:info`).
    SystemInfo,
    /// Re-open the most recently closed tab.
    ReopenTab,
    /// Split the focused pane.
    SplitRight,
    SplitDown,
    /// Close the focused pane.
    ClosePane,
    /// Even out the panes: equal columns, equal rows, or a grid.
    EvenColumns,
    EvenRows,
    EvenGrid,
    /// Easter eggs, listed only when the query names them.
    Party,
    Matrix,
    Answer,
    /// Switch to the editor.
    OpenEditor,
    /// Switch to the terminal.
    OpenTerminal,
    /// Turn the SSH workspace sidebar on or off.
    ToggleSshMode,
    /// Open the SFTP browser for a connected host.
    Sftp,
    /// Edit the editor's keybindings.
    Keybindings,
    /// Look at an image file.
    ViewImage,
}

pub fn palette_items(config: &Config) -> Vec<(String, PaletteAction)> {
    let mut items = vec![
        ("New terminal".to_owned(), PaletteAction::NewTerminal),
        ("Connect to host…".to_owned(), PaletteAction::ConnectAdhoc),
        ("Close tab".to_owned(), PaletteAction::CloseTab),
        ("Reopen closed tab".to_owned(), PaletteAction::ReopenTab),
        ("Split right".to_owned(), PaletteAction::SplitRight),
        ("Split down".to_owned(), PaletteAction::SplitDown),
        ("Close pane".to_owned(), PaletteAction::ClosePane),
        ("Even out: columns".to_owned(), PaletteAction::EvenColumns),
        ("Even out: rows".to_owned(), PaletteAction::EvenRows),
        ("Even out: grid".to_owned(), PaletteAction::EvenGrid),
        ("System info".to_owned(), PaletteAction::SystemInfo),
        ("Editor: open".to_owned(), PaletteAction::OpenEditor),
        ("Terminal: open".to_owned(), PaletteAction::OpenTerminal),
        ("SSH mode: toggle".to_owned(), PaletteAction::ToggleSshMode),
        ("SFTP: browse files".to_owned(), PaletteAction::Sftp),
        ("Keybindings…".to_owned(), PaletteAction::Keybindings),
        ("Image: view…".to_owned(), PaletteAction::ViewImage),
        ("Settings…".to_owned(), PaletteAction::Settings),
        ("Quit".to_owned(), PaletteAction::Quit),
    ];
    for (i, p) in config.profiles.iter().enumerate() {
        items.push((
            format!("SSH: {} ({}@{}:{})", p.name, p.user, p.host, p.port),
            PaletteAction::Connect(i),
        ));
    }
    items
}

/// Palette entries matching a query, best match first.
///
/// Matching is fuzzy (subsequence) and case-insensitive; recently used entries
/// are boosted so an empty query shows them first.
pub fn palette_filtered(
    config: &Config,
    query: &str,
    recents: &[String],
) -> Vec<(String, PaletteAction)> {
    let mut items = palette_items(config);
    let needle = query.trim().to_lowercase();
    // Easter eggs: hidden until the query names them.
    for egg in crate::app::eggs::hidden_eggs(&needle) {
        items.push((egg.label().to_owned(), egg.into()));
    }
    let mut scored: Vec<(i32, usize, (String, PaletteAction))> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let score = fuzzy_score(&item.0, &needle)?;
            let recent_boost = recents
                .iter()
                .position(|r| r == &item.0)
                .map_or(0, |pos| 1000 - pos as i32);
            Some((score + recent_boost, i, item))
        })
        .collect();
    // Stable by score, then by original order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// Fuzzy subsequence score: every needle character must appear in order.
///
/// Higher is better; consecutive and word-start matches score extra.
fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    let mut score = 0;
    let mut cursor = 0;
    let mut last: Option<usize> = None;
    for want in needle.chars() {
        let mut found = None;
        while cursor < hay.len() {
            if hay[cursor] == want {
                found = Some(cursor);
                cursor += 1;
                break;
            }
            cursor += 1;
        }
        let index = found?;
        if last == Some(index.wrapping_sub(1)) {
            score += 8; // consecutive run
        }
        if index == 0 || matches!(hay[index - 1], ' ' | ':' | '-' | '_') {
            score += 4; // start of a word
        }
        score += 1;
        last = Some(index);
    }
    Some(score)
}

/// Draw the palette. Returns the chosen action, if any.
pub fn show_palette(
    ui: &mut egui::Ui,
    theme: &Theme,
    config: &Config,
    query: &mut String,
    selected: &mut usize,
    recents: &[String],
) -> Option<PaletteAction> {
    let mut action = None;
    let filtered = palette_filtered(config, query, recents);
    if *selected >= filtered.len() {
        *selected = filtered.len().saturating_sub(1);
    }

    egui::Area::new(egui::Id::new("palette"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(with_alpha(color32(theme.ui.panel_bg), theme.ui_alpha))
                .stroke(egui::Stroke::new(1.0, color32(theme.ui.border)))
                .corner_radius(10.0)
                .shadow(soft_shadow(14, 36, 180))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_min_width(480.0);
                    ui.set_max_width(480.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(query)
                            .hint_text("Type a command or ssh profile…")
                            .desired_width(460.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    resp.request_focus();
                    ui.add_space(6.0);
                    if filtered.is_empty() {
                        ui.label(
                            egui::RichText::new("No matches").color(color32(theme.ui.text_dim)),
                        );
                    }
                    for (i, (label, _)) in filtered.iter().enumerate() {
                        let selected_row = i == *selected;
                        let (rect, row) =
                            ui.allocate_exact_size(egui::vec2(460.0, 26.0), egui::Sense::click());
                        let lit = ui.ctx().animate_bool_with_time(
                            row.id,
                            row.hovered() || selected_row,
                            0.08,
                        );
                        let painter = ui.painter_at(rect);
                        if lit > 0.0 {
                            painter.rect_filled(
                                rect,
                                6.0,
                                with_alpha(color32(theme.ui.accent), 0.16 * lit),
                            );
                            // Accent bar on the leading edge.
                            let bar = egui::Rect::from_min_size(
                                egui::pos2(rect.min.x + 2.0, rect.min.y + 5.0),
                                egui::vec2(2.0, rect.height() - 10.0),
                            );
                            painter.rect_filled(
                                bar,
                                1.0,
                                with_alpha(color32(theme.ui.accent), lit),
                            );
                        }
                        painter.text(
                            egui::pos2(rect.min.x + 12.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::new(13.0, egui::FontFamily::Monospace),
                            if selected_row {
                                color32(theme.ui.text)
                            } else {
                                color32(theme.ui.text_dim)
                            },
                        );
                        if row.clicked() {
                            action = Some(filtered[i].1.clone());
                        }
                        if row.hovered() {
                            *selected = i;
                        }
                        row.on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                    ui.add_space(6.0);
                    hairline(ui, theme);
                    ui.add_space(5.0);
                    ui.label(
                        egui::RichText::new("Up/Down navigate    Enter run    Esc close")
                            .size(10.5)
                            .color(color32(theme.ui.text_dim)),
                    );
                });
        });
    action
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Width of the label column in a settings row.
const SETTING_LABEL_W: f32 = 104.0;

/// A settings row: a dim label in a fixed-width column, then the control.
fn setting_row(ui: &mut egui::Ui, theme: &Theme, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(SETTING_LABEL_W, 22.0), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(rect.min.x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
            color32(theme.ui.text_dim),
        );
        add(ui);
    });
    ui.add_space(3.0);
}

/// A checkbox aligned with the control column of [`setting_row`].
fn check_row(ui: &mut egui::Ui, theme: &Theme, checked: &mut bool, label: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.add_space(SETTING_LABEL_W);
        ui.checkbox(
            checked,
            egui::RichText::new(label)
                .size(12.5)
                .color(color32(theme.ui.text)),
        )
    })
    .inner
}

/// Draw the settings modal.
///
/// Edits are applied to the running app as they happen (live preview); Save
/// persists them to `config.toml`, Cancel/Esc reverts them.
///
/// Returns `(close, saved_config_and_font, known_hosts_line_to_forget)`.
pub fn show_settings(
    ui: &mut egui::Ui,
    theme: &Theme,
    draft: &mut Config,
    font: &mut String,
    error: &mut Option<String>,
    fonts: &[String],
    known_hosts: &[crate::config::known_hosts::Entry],
) -> (bool, Option<(Config, String)>, Option<usize>) {
    let mut close = false;
    let mut saved: Option<(Config, String)> = None;
    let mut forget: Option<usize> = None;
    modal_frame(ui, theme, "Settings", |ui| {
        ui.set_min_width(500.0);
        ui.label(
            egui::RichText::new("Changes apply immediately — Save writes them to config.toml.")
                .size(11.0)
                .color(color32(theme.ui.text_dim)),
        );
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .max_height(430.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                section_header(ui, theme, "APPEARANCE");
                setting_row(ui, theme, "Theme", |ui| {
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(crate::theme::builtin(&draft.theme).name)
                        .show_ui(ui, |ui| {
                            for id in crate::theme::all_ids() {
                                let name = crate::theme::builtin(id).name;
                                ui.selectable_value(&mut draft.theme, id.to_owned(), name);
                            }
                        });
                });
                setting_row(ui, theme, "Font", |ui| {
                    egui::ComboBox::from_id_salt("font")
                        .selected_text(font.clone())
                        .show_ui(ui, |ui| {
                            for f in fonts {
                                ui.selectable_value(font, f.clone(), f);
                            }
                        });
                });
                setting_row(ui, theme, "Font size", |ui| {
                    ui.add(
                        egui::Slider::new(&mut draft.font_size, 8.0..=32.0)
                            .fixed_decimals(0)
                            .suffix(" pt"),
                    );
                });
                setting_row(ui, theme, "Opacity", |ui| {
                    ui.add(
                        egui::Slider::new(&mut draft.opacity, 0.5..=1.0)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    );
                });

                ui.add_space(8.0);
                section_header(ui, theme, "BACKGROUND");
                setting_row(ui, theme, "Image", |ui| {
                    let mut path = draft.background_image.clone().unwrap_or_default();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut path)
                            .hint_text("~/Pictures/wallpaper.png")
                            .desired_width(300.0),
                    );
                    if resp.changed() {
                        draft.background_image = if path.trim().is_empty() {
                            None
                        } else {
                            Some(path)
                        };
                    }
                });
                if let Some(path) = draft.background_image.as_deref()
                    && !path.trim().is_empty()
                    && !std::path::Path::new(&crate::app::expand_tilde(path)).is_file()
                {
                    ui.horizontal(|ui| {
                        ui.add_space(SETTING_LABEL_W);
                        ui.label(
                            egui::RichText::new("No such file — the current image stays")
                                .size(11.0)
                                .color(color32(theme.ui.danger)),
                        );
                    });
                    ui.add_space(3.0);
                }
                let has_image = draft.background_image.is_some();
                setting_row(ui, theme, "Image opacity", |ui| {
                    ui.add_enabled(
                        has_image,
                        egui::Slider::new(&mut draft.background_image_opacity, 0.0..=1.0)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    );
                });
                setting_row(ui, theme, "Image fit", |ui| {
                    ui.add_enabled_ui(has_image, |ui| {
                        egui::ComboBox::from_id_salt("background_fit")
                            .selected_text(draft.background_fit.label())
                            .show_ui(ui, |ui| {
                                for fit in [
                                    BackgroundFit::Cover,
                                    BackgroundFit::Contain,
                                    BackgroundFit::Stretch,
                                ] {
                                    ui.selectable_value(&mut draft.background_fit, fit, fit.label());
                                }
                            });
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(SETTING_LABEL_W);
                    ui.label(
                        egui::RichText::new("Tip: drop an image file onto the window to set it.")
                            .size(11.0)
                            .color(color32(theme.ui.text_dim)),
                    );
                });

                ui.add_space(8.0);
                section_header(ui, theme, "CURSOR");
                setting_row(ui, theme, "Shape", |ui| {
                    egui::ComboBox::from_id_salt("cursor_shape")
                        .selected_text(draft.cursor_shape.label())
                        .show_ui(ui, |ui| {
                            for shape in [
                                CursorShape::Block,
                                CursorShape::Beam,
                                CursorShape::Underline,
                            ] {
                                ui.selectable_value(&mut draft.cursor_shape, shape, shape.label());
                            }
                        });
                });
                check_row(ui, theme, &mut draft.cursor_blink, "Blink when idle");
                check_row(ui, theme, &mut draft.visual_bell, "Flash the terminal on bell");

                ui.add_space(8.0);
                section_header(ui, theme, "TERMINAL");
                setting_row(ui, theme, "Shell", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.shell)
                            .hint_text("$SHELL")
                            .desired_width(300.0),
                    );
                });
                setting_row(ui, theme, "Scrollback", |ui| {
                    let mut sb = draft.scrollback as f32;
                    ui.add(
                        egui::Slider::new(&mut sb, 1000.0..=100_000.0)
                            .logarithmic(true)
                            .custom_formatter(|v, _| format!("{} lines", v as usize)),
                    );
                    draft.scrollback = sb as usize;
                });
                check_row(
                    ui,
                    theme,
                    &mut draft.copy_on_select,
                    "Copy a selection to the clipboard on select",
                );
                check_row(
                    ui,
                    theme,
                    &mut draft.desktop_notifications,
                    "Notify when a long command finishes unfocused",
                );

                ui.add_space(8.0);
                section_header(ui, theme, "SESSIONS");
                check_row(ui, theme, &mut draft.log_sessions, "Log session output to disk");
                check_row(
                    ui,
                    theme,
                    &mut draft.auto_reconnect,
                    "Reconnect dropped SSH sessions",
                );

                ui.add_space(8.0);
                section_header(ui, theme, "SSH PROFILES");
                ui.horizontal(|ui| {
                    ui.add_space(SETTING_LABEL_W);
                    if ui.small_button("+ Add profile").clicked() {
                        draft.profiles.push(SshProfile {
                            name: format!("profile-{}", draft.profiles.len() + 1),
                            host: String::new(),
                            port: 22,
                            user: String::new(),
                            auth: AuthMethod::Password,
                            key_path: None,
                            proxy_jump: None,
                            forwards: Vec::new(),
                            remote_forwards: Vec::new(),
                            socks_forwards: Vec::new(),
                            agent_forwarding: false,
                            group: None,
                        });
                    }
                    if ui
                        .small_button("Import ~/.ssh/config")
                        .on_hover_text("Add profiles from your OpenSSH config")
                        .clicked()
                    {
                        for p in crate::config::import_ssh_config() {
                            if !draft.profiles.iter().any(|e| e.name == p.name) {
                                draft.profiles.push(p);
                            }
                        }
                    }
                });
                ui.add_space(6.0);

                if draft.profiles.is_empty() {
                    ui.label(
                        egui::RichText::new("No profiles yet — add one to connect quickly.")
                            .color(color32(theme.ui.text_dim)),
                    );
                }
                let mut remove: Option<usize> = None;
                for (i, p) in draft.profiles.iter_mut().enumerate() {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            egui::Grid::new(format!("profile_{i}"))
                                .num_columns(4)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("Name");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut p.name)
                                            .desired_width(140.0),
                                    );
                                    ui.label("Host");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut p.host)
                                            .desired_width(180.0),
                                    );
                                    ui.end_row();

                                    ui.label("User");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut p.user)
                                            .desired_width(140.0),
                                    );
                                    ui.label("Port");
                                    ui.add(
                                        egui::DragValue::new(&mut p.port)
                                            .range(1..=65535)
                                            .clamp_existing_to_range(true),
                                    );
                                    ui.end_row();

                                    ui.label("Auth");
                                    let auth_text = match p.auth {
                                        AuthMethod::Password => "password",
                                        AuthMethod::Key => "key file",
                                        AuthMethod::Agent => "ssh-agent",
                                    };
                                    egui::ComboBox::from_id_salt(format!("auth_{i}"))
                                        .selected_text(auth_text)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut p.auth,
                                                AuthMethod::Password,
                                                "password",
                                            );
                                            ui.selectable_value(
                                                &mut p.auth,
                                                AuthMethod::Key,
                                                "key file",
                                            );
                                            ui.selectable_value(
                                                &mut p.auth,
                                                AuthMethod::Agent,
                                                "ssh-agent",
                                            );
                                        });
                                    ui.label("Key path");
                                    let mut kp = p.key_path.clone().unwrap_or_default();
                                    ui.add_enabled(
                                        p.auth == AuthMethod::Key,
                                        egui::TextEdit::singleline(&mut kp)
                                            .hint_text("~/.ssh/id_ed25519")
                                            .desired_width(260.0),
                                    );
                                    p.key_path = if kp.is_empty() { None } else { Some(kp) };
                                    ui.end_row();

                                    ui.label("Jump host");
                                    let mut jump = p.proxy_jump.clone().unwrap_or_default();
                                    ui.add(
                                        egui::TextEdit::singleline(&mut jump)
                                            .hint_text("user@bastion:22")
                                            .desired_width(140.0),
                                    );
                                    p.proxy_jump =
                                        if jump.trim().is_empty() { None } else { Some(jump) };
                                    ui.label("Forwards");
                                    let mut fwd = p.forwards.join(", ");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut fwd)
                                            .hint_text("8080:localhost:5432")
                                            .desired_width(180.0),
                                    );
                                    p.forwards = fwd
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    ui.end_row();

                                    ui.label("Remote (-R)");
                                    let mut rfwd = p.remote_forwards.join(", ");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut rfwd)
                                            .hint_text("9000:localhost:3000")
                                            .desired_width(140.0),
                                    );
                                    p.remote_forwards = rfwd
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    ui.label("SOCKS (-D)");
                                    let mut sfwd = p.socks_forwards.join(", ");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut sfwd)
                                            .hint_text("1080")
                                            .desired_width(180.0),
                                    );
                                    p.socks_forwards = sfwd
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    ui.end_row();

                                    ui.label("Agent (-A)");
                                    ui.checkbox(&mut p.agent_forwarding, "forward ssh-agent");
                                    ui.label("");
                                    ui.label("");
                                    ui.end_row();
                                });
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(
                                        "Passwords are stored in the system keyring, never on disk.",
                                    )
                                    .weak()
                                    .size(11.0)
                                    .color(color32(theme.ui.text_dim)),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Remove").clicked() {
                                            remove = Some(i);
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(4.0);
                }
                if let Some(i) = remove {
                    // The keyring entries go with it when the settings are
                    // saved (on cancel the profile stays, so its secrets must
                    // stay too).
                    draft.profiles.remove(i);
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(SETTING_LABEL_W);
                    ui.add_enabled(
                        crate::config::secrets::available(),
                        egui::Checkbox::new(
                            &mut draft.remember_secrets,
                            "Store SSH secrets in the system keyring",
                        ),
                    );
                });

                ui.add_space(8.0);
                section_header(ui, theme, "SECURITY");
                check_row(
                    ui,
                    theme,
                    &mut draft.allow_osc52_read,
                    "Allow programs to read the clipboard (OSC 52)",
                );
                check_row(
                    ui,
                    theme,
                    &mut draft.agent_api.enabled,
                    "Let local coding agents use SSH profiles (MCP)",
                );
                check_row(
                    ui,
                    theme,
                    &mut draft.agent_api.confirm_commands,
                    "Approve agent commands before they run (read-only ones excepted)",
                );
                check_row(
                    ui,
                    theme,
                    &mut draft.agent_api.read_only,
                    "Read-only mode: refuse anything outside the safe command set",
                );

                ui.add_space(8.0);
                section_header(ui, theme, "KNOWN HOSTS");
                if known_hosts.is_empty() {
                    ui.label(
                        egui::RichText::new("No trusted host keys yet.")
                            .color(color32(theme.ui.text_dim)),
                    );
                }
                for entry in known_hosts.iter() {
                    ui.horizontal(|ui| {
                        let label = if entry.hashed {
                            "(hashed host)".to_owned()
                        } else {
                            entry.hosts.clone()
                        };
                        ui.label(
                            egui::RichText::new(label)
                                .monospace()
                                .size(11.5)
                                .color(color32(theme.ui.text)),
                        );
                        ui.label(
                            egui::RichText::new(entry.key_type.as_str())
                                .size(11.0)
                                .color(color32(theme.ui.text_dim)),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .small_button("Forget")
                                    .on_hover_text("Remove this key from ~/.ssh/known_hosts")
                                    .clicked()
                                {
                                    forget = Some(entry.line);
                                }
                            },
                        );
                    });
                }
            });

        ui.add_space(10.0);
        if let Some(err) = error {
            ui.colored_label(color32(theme.ui.danger), err);
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            if primary_button(ui, theme, "Save", 88.0).clicked() {
                // The font family lives in its own field while the modal is
                // open; fold it back in before writing the file.
                draft.font = font.clone();
                match draft.save() {
                    Ok(()) => {
                        saved = Some((draft.clone(), font.clone()));
                        close = true;
                    }
                    Err(e) => *error = Some(format!("Failed to save: {e:#}")),
                }
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                close = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("Esc reverts")
                        .size(10.5)
                        .color(color32(theme.ui.text_dim)),
                );
            });
        });
    });
    (close, saved, forget)
}

// ---------------------------------------------------------------------------
// Ad-hoc connect
// ---------------------------------------------------------------------------

/// Draw the `user@host[:port]` connect dialog.
///
/// Returns `(close, submitted_spec)`.
pub fn show_connect(
    ui: &mut egui::Ui,
    theme: &Theme,
    text: &mut String,
    error: &mut Option<String>,
) -> (bool, Option<String>) {
    let mut close = false;
    let mut submit = None;
    modal_frame(ui, theme, "Connect to host", |ui| {
        ui.label(
            egui::RichText::new("user@host[:port] — password or ssh-agent, no profile needed")
                .size(11.5)
                .color(color32(theme.ui.text_dim)),
        );
        ui.add_space(6.0);
        let resp = ui.add(
            egui::TextEdit::singleline(text)
                .hint_text("root@example.com:22")
                .desired_width(400.0)
                .font(egui::TextStyle::Monospace),
        );
        resp.request_focus();
        if let Some(err) = error {
            ui.add_space(6.0);
            ui.colored_label(color32(theme.ui.danger), err.as_str());
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if primary_button(ui, theme, "Connect", 120.0).clicked() || enter {
                submit = Some(text.clone());
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                close = true;
            }
        });
    });
    (close, submit)
}

// ---------------------------------------------------------------------------
// Image viewer
// ---------------------------------------------------------------------------

/// Draw the image-path dialog.
///
/// Returns `(close, submitted_path)`.
pub fn show_view_image(
    ui: &mut egui::Ui,
    theme: &Theme,
    text: &mut String,
    error: &mut Option<String>,
) -> (bool, Option<String>) {
    let mut close = false;
    let mut submit = None;
    modal_frame(ui, theme, "View an image", |ui| {
        ui.label(
            egui::RichText::new(
                "a picture to look at — png, jpg, webp, gif, bmp…; click the picture to close it",
            )
            .size(11.5)
            .color(color32(theme.ui.text_dim)),
        );
        ui.add_space(6.0);
        let resp = ui.add(
            egui::TextEdit::singleline(text)
                .hint_text("~/pictures/wall.png")
                .desired_width(400.0)
                .font(egui::TextStyle::Monospace),
        );
        resp.request_focus();
        if let Some(err) = error {
            ui.add_space(6.0);
            ui.colored_label(color32(theme.ui.danger), err.as_str());
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if primary_button(ui, theme, "View", 120.0).clicked() || enter {
                submit = Some(text.clone());
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                close = true;
            }
        });
    });
    (close, submit)
}

/// Draw a decoded image over a dimmed screen. A click anywhere closes it.
pub fn show_image(
    ui: &mut egui::Ui,
    theme: &Theme,
    path: &std::path::Path,
    texture: &egui::TextureHandle,
) -> bool {
    let screen = ui.ctx().content_rect();
    let painter = ui.painter();
    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(215));
    // Fit into 80% of the screen, keeping the picture's own aspect.
    let size = texture.size_vec2().max(egui::vec2(1.0, 1.0));
    let bounds = egui::vec2(screen.width() * 0.8, screen.height() * 0.8);
    let scale = (bounds.x / size.x).min(bounds.y / size.y);
    let rect =
        egui::Rect::from_center_size(screen.center(), egui::vec2(size.x * scale, size.y * scale));
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(texture.id(), rect, uv, egui::Color32::WHITE);
    // The path it came from, along the bottom.
    painter.text(
        egui::pos2(screen.center().x, screen.bottom() - 14.0),
        egui::Align2::CENTER_CENTER,
        path.display().to_string(),
        egui::FontId::monospace(12.0),
        with_alpha(color32(theme.ui.text), 0.8),
    );
    ui.interact(
        screen,
        egui::Id::new("cl2metal-image-view"),
        egui::Sense::click(),
    )
    .clicked()
}

// ---------------------------------------------------------------------------
// Close confirmation
// ---------------------------------------------------------------------------

/// Draw the "work is still running — close anyway?" question.
///
/// Returns `(cancel, close_anyway)`. The names come from the pane's living
/// children (VS Code's `confirmOnKill` model), so an idle shell never
/// raises it and `npm` or an editor always does.
pub fn show_close_confirm(
    ui: &mut egui::Ui,
    theme: &Theme,
    jobs: &[String],
    scope: &CloseScope,
) -> (bool, bool) {
    let mut cancel = false;
    let mut close = false;
    let what = match scope {
        CloseScope::Pane { .. } => "this pane",
        CloseScope::Tab(_) => "this tab",
        CloseScope::App => "the window",
    };
    modal_frame(ui, theme, "Close anyway?", |ui| {
        ui.label(
            egui::RichText::new(format!("{} is still running in {what}.", jobs.join(", ")))
                .size(12.5)
                .color(color32(theme.ui.text)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Closing kills it.")
                .size(11.5)
                .color(color32(theme.ui.text_dim)),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if primary_button(ui, theme, "Close anyway", 140.0).clicked() || enter {
                close = true;
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                cancel = true;
            }
        });
    });
    (cancel, close)
}

/// Offer to install a newer release.
///
/// Returns `(later, update)`: `later` closes the prompt for this launch,
/// `update` hands the installer to the user's terminal.
pub fn show_update(ui: &mut egui::Ui, theme: &Theme, version: &str) -> (bool, bool) {
    let mut later = false;
    let mut update = false;
    modal_frame(ui, theme, "Update available", |ui| {
        ui.label(
            egui::RichText::new(format!(
                "cl2metal {version} is out; this build is {}.",
                env!("CARGO_PKG_VERSION")
            ))
            .size(12.5)
            .color(color32(theme.ui.text)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Update now? Your terminal opens and runs the installer.")
                .size(11.5)
                .color(color32(theme.ui.text_dim)),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // No Enter shortcut here: the prompt appears on its own, and an
            // Enter already in flight from the terminal must not install.
            if primary_button(ui, theme, "Update now", 140.0).clicked() {
                update = true;
            }
            if secondary_button(ui, theme, "Later", 88.0).clicked() {
                later = true;
            }
        });
    });
    (later, update)
}

// ---------------------------------------------------------------------------
// SSH profile editor
// ---------------------------------------------------------------------------

/// The label for an authentication method.
pub fn auth_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::Agent => "ssh-agent",
        AuthMethod::Key => "private key",
        AuthMethod::Password => "password",
    }
}

/// Edit a `Vec<String>` as one comma-separated line.
fn edit_list(ui: &mut egui::Ui, list: &mut Vec<String>, hint: &str) {
    let mut text = list.join(", ");
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width(320.0)
            .font(egui::TextStyle::Monospace),
    );
    if response.changed() {
        *list = text
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
    }
}

/// Edit an `Option<String>` as one line.
fn edit_optional(ui: &mut egui::Ui, value: &mut Option<String>, hint: &str) {
    let mut text = value.clone().unwrap_or_default();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width(320.0)
            .font(egui::TextStyle::Monospace),
    );
    if response.changed() {
        *value = if text.trim().is_empty() {
            None
        } else {
            Some(text.clone())
        };
    }
}

/// Draw the SSH profile editor. Returns `(close, save, remove)`.
///
/// `original` names the profile being edited, when there is one: the editor
/// offers to delete it, and a rename edits it in place instead of adding a
/// second host. Passwords are never edited here: they live in the system
/// keyring, and the profile only records *that* a password is used.
pub fn show_profile(
    ui: &mut egui::Ui,
    theme: &Theme,
    profile: &mut SshProfile,
    original: Option<&str>,
    error: &mut Option<String>,
) -> (bool, bool, bool) {
    let mut close = false;
    let mut save = false;
    let mut remove = false;
    let title = if original.is_none() {
        "New host"
    } else {
        "Edit host"
    };
    modal_frame(ui, theme, title, |ui| {
        egui::Grid::new("profile-grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut profile.name)
                        .desired_width(320.0)
                        .font(egui::TextStyle::Monospace),
                );
                ui.end_row();

                ui.label("Host");
                ui.add(
                    egui::TextEdit::singleline(&mut profile.host)
                        .hint_text("example.com")
                        .desired_width(320.0)
                        .font(egui::TextStyle::Monospace),
                );
                ui.end_row();

                ui.label("Port");
                ui.add(egui::DragValue::new(&mut profile.port).range(1..=65535));
                ui.end_row();

                ui.label("User");
                ui.add(
                    egui::TextEdit::singleline(&mut profile.user)
                        .desired_width(320.0)
                        .font(egui::TextStyle::Monospace),
                );
                ui.end_row();

                ui.label("Group");
                edit_optional(ui, &mut profile.group, "optional, groups the sidebar");
                ui.end_row();

                ui.label("Auth");
                egui::ComboBox::from_id_salt("profile-auth")
                    .selected_text(auth_label(profile.auth))
                    .show_ui(ui, |ui| {
                        for method in [AuthMethod::Agent, AuthMethod::Key, AuthMethod::Password] {
                            ui.selectable_value(&mut profile.auth, method, auth_label(method));
                        }
                    });
                ui.end_row();

                if profile.auth == AuthMethod::Key {
                    ui.label("Key file");
                    edit_optional(ui, &mut profile.key_path, "~/.ssh/id_ed25519");
                    ui.end_row();
                }

                ui.label("Jump host");
                edit_optional(ui, &mut profile.proxy_jump, "user@bastion[:port]");
                ui.end_row();

                ui.label("Local -L");
                edit_list(ui, &mut profile.forwards, "8080:localhost:80");
                ui.end_row();

                ui.label("Remote -R");
                edit_list(ui, &mut profile.remote_forwards, "9000:localhost:3000");
                ui.end_row();

                ui.label("SOCKS -D");
                edit_list(ui, &mut profile.socks_forwards, "1080");
                ui.end_row();

                ui.label("Agent -A");
                ui.checkbox(&mut profile.agent_forwarding, "forward ssh-agent");
                ui.end_row();
            });

        if profile.auth == AuthMethod::Password {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("the password is asked for on connect and kept in the keyring")
                    .size(11.0)
                    .color(color32(theme.ui.text_dim)),
            );
        }
        if let Some(err) = error {
            ui.add_space(6.0);
            ui.colored_label(color32(theme.ui.danger), err.as_str());
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if primary_button(ui, theme, "Save", 110.0).clicked() {
                save = true;
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                close = true;
            }
            if original.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = egui::RichText::new("Remove host").color(color32(theme.ui.danger));
                    if ui.button(label).clicked() {
                        remove = true;
                    }
                });
            }
        });
    });
    (close, save, remove)
}

// ---------------------------------------------------------------------------
// Keybinding editor
// ---------------------------------------------------------------------------

/// Draw the keybinding editor. Returns `(close, save, reset)`.
///
/// `recording` is the command waiting for a key sequence: the next key press
/// that is not Escape becomes its binding.
pub fn show_keybindings(
    ui: &mut egui::Ui,
    theme: &Theme,
    bindings: &mut std::collections::BTreeMap<String, String>,
    recording: &mut Option<String>,
    error: &mut Option<String>,
) -> (bool, bool, bool) {
    let mut close = false;
    let mut save = false;
    let mut reset = false;
    modal_frame(ui, theme, "Keybindings", |ui| {
        ui.label(
            egui::RichText::new("click a binding, then press the keys — Esc cancels")
                .size(11.5)
                .color(color32(theme.ui.text_dim)),
        );
        ui.add_space(6.0);
        let conflicts = crate::editor::keymap::conflicts(
            &bindings
                .iter()
                .map(|(c, s)| (c.clone(), s.clone()))
                .collect::<Vec<_>>(),
        );
        if !conflicts.is_empty() {
            for (sequence, commands) in &conflicts {
                ui.colored_label(
                    color32(theme.ui.danger),
                    format!("{sequence} is bound to {}", commands.join(" and ")),
                );
            }
            ui.add_space(4.0);
        }
        if let Some(err) = error {
            ui.colored_label(color32(theme.ui.danger), err.as_str());
            ui.add_space(4.0);
        }
        egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                egui::Grid::new("keybinding-grid")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        let mut clear: Option<String> = None;
                        for (command, sequence) in bindings.iter() {
                            ui.label(
                                egui::RichText::new(command.replace('_', " "))
                                    .size(12.0)
                                    .color(color32(theme.ui.text)),
                            );
                            let is_recording = recording.as_deref() == Some(command.as_str());
                            let label = if is_recording {
                                "press keys…".to_owned()
                            } else {
                                sequence.clone()
                            };
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(label).size(12.0).monospace().color(
                                            color32(if is_recording {
                                                theme.ui.accent
                                            } else {
                                                theme.ui.text_dim
                                            }),
                                        ),
                                    )
                                    .min_size(egui::vec2(160.0, 20.0)),
                                )
                                .clicked()
                            {
                                *recording = Some(command.clone());
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("clear")
                                            .size(11.0)
                                            .color(color32(theme.ui.text_dim)),
                                    )
                                    .min_size(egui::vec2(48.0, 20.0)),
                                )
                                .clicked()
                            {
                                clear = Some(command.clone());
                            }
                            ui.end_row();
                        }
                        if let Some(command) = clear {
                            bindings.remove(&command);
                        }
                    });
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if primary_button(ui, theme, "Save", 110.0).clicked() {
                save = true;
            }
            if secondary_button(ui, theme, "Reset to defaults", 150.0).clicked() {
                reset = true;
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                close = true;
            }
        });
    });
    (close, save, reset)
}

// ---------------------------------------------------------------------------
// Tab rename
// ---------------------------------------------------------------------------

/// Draw the tab rename dialog. Returns `(close, submitted_title)`.
pub fn show_rename(ui: &mut egui::Ui, theme: &Theme, text: &mut String) -> (bool, Option<String>) {
    let mut close = false;
    let mut submit = None;
    modal_frame(ui, theme, "Rename tab", |ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(text)
                .hint_text("tab title")
                .desired_width(400.0),
        );
        resp.request_focus();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if primary_button(ui, theme, "Rename", 110.0).clicked() || enter {
                submit = Some(text.clone());
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                close = true;
            }
        });
    });
    (close, submit)
}

/// Confirm a multi-line paste that would run its last line immediately.
///
/// Returns `(close, confirmed)`.
pub fn show_paste_confirm(ui: &mut egui::Ui, theme: &Theme, text: &str) -> (bool, bool) {
    let mut close = false;
    let mut confirmed = false;
    let lines = text.lines().count();
    let last: String = text
        .lines()
        .last()
        .unwrap_or_default()
        .chars()
        .take(64)
        .collect();
    modal_frame(ui, theme, "Paste", |ui| {
        ui.label(format!(
            "{lines} lines ending in a newline: the last one runs immediately."
        ));
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(last)
                .monospace()
                .color(color32(theme.ui.accent)),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if primary_button(ui, theme, "Paste", 110.0).clicked() || enter {
                confirmed = true;
            }
            if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                close = true;
            }
        });
    });
    (close, confirmed)
}

// ---------------------------------------------------------------------------
// Scrollback search
// ---------------------------------------------------------------------------
/// Draw the scrollback search bar over the terminal.
///
/// Returns `(close, next, previous)`.
pub fn show_search(
    ui: &mut egui::Ui,
    theme: &Theme,
    anchor: egui::Rect,
    query: &mut String,
    status: &str,
    is_error: bool,
) -> (bool, bool, bool) {
    let mut close = false;
    let mut next = false;
    let mut prev = false;
    egui::Area::new(egui::Id::new("search_bar"))
        .fixed_pos(egui::pos2(anchor.max.x - 372.0, anchor.min.y + 8.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(with_alpha(color32(theme.ui.panel_bg), theme.ui_alpha))
                .stroke(egui::Stroke::new(1.0, color32(theme.ui.border)))
                .corner_radius(8.0)
                .shadow(soft_shadow(10, 26, 150))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(query)
                                .hint_text("Search scrollback…")
                                .desired_width(170.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        resp.request_focus();
                        ui.label(egui::RichText::new(status).size(11.0).color(if is_error {
                            color32(theme.ui.danger)
                        } else {
                            color32(theme.ui.text_dim)
                        }));
                        if ui.small_button("Prev").clicked() {
                            prev = true;
                        }
                        if ui.small_button("Next").clicked() {
                            next = true;
                        }
                        if ui.small_button("×").on_hover_text("Close (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
        });
    (close, next, prev)
}

// ---------------------------------------------------------------------------
// SSH prompts
// ---------------------------------------------------------------------------

/// Draw an SSH question modal. Returns `Some(answer)` when submitted.
pub fn show_prompt(
    ui: &mut egui::Ui,
    theme: &Theme,
    kind: &mut PromptKind,
    text: &mut String,
    submit: &mut bool,
    cancel: &mut bool,
) {
    match kind {
        PromptKind::HostKey {
            host,
            port,
            key_type,
            fingerprint,
        } => {
            let title = "Unknown host key";
            modal_frame(ui, theme, title, |ui| {
                ui.label(format!(
                    "The authenticity of {}:{port} can't be established.",
                    host
                ));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("{key_type} key fingerprint"))
                        .weak()
                        .color(color32(theme.ui.text_dim)),
                );
                ui.add_space(2.0);
                ui.monospace(fingerprint);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Adding it to ~/.ssh/known_hosts. Verify the fingerprint through a \
                         trusted channel before continuing.",
                    )
                    .weak()
                    .size(11.5)
                    .color(color32(theme.ui.text_dim)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, theme, "Trust & connect", 130.0).clicked() {
                        *submit = true;
                    }
                    if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                        *cancel = true;
                    }
                });
            });
        }
        PromptKind::Password {
            prompt,
            remember,
            profile,
        } => {
            modal_frame(ui, theme, "Password required", |ui| {
                ui.label(prompt.as_str());
                ui.add_space(6.0);
                let masked = !text.is_empty();
                let resp = ui.add(
                    egui::TextEdit::singleline(text)
                        .password(masked)
                        .desired_width(400.0)
                        .font(egui::TextStyle::Monospace),
                );
                if resp.gained_focus() {
                    resp.request_focus();
                }
                if profile.is_some() {
                    ui.add_space(4.0);
                    ui.checkbox(remember, "Remember in system keyring for this profile");
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if primary_button(ui, theme, "Connect", 120.0).clicked()
                        || (enter && resp.has_focus())
                    {
                        *submit = true;
                    }
                    if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                        *cancel = true;
                    }
                });
            });
        }
        PromptKind::Keyboard { prompt } => {
            modal_frame(ui, theme, "Authentication required", |ui| {
                ui.label(prompt.as_str());
                ui.add_space(6.0);
                let masked = !text.is_empty();
                let resp = ui.add(
                    egui::TextEdit::singleline(text)
                        .password(masked)
                        .desired_width(400.0)
                        .font(egui::TextStyle::Monospace),
                );
                if resp.gained_focus() {
                    resp.request_focus();
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if primary_button(ui, theme, "Continue", 120.0).clicked()
                        || (enter && resp.has_focus())
                    {
                        *submit = true;
                    }
                    if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                        *cancel = true;
                    }
                });
            });
        }
        PromptKind::Passphrase {
            prompt,
            remember,
            profile,
        } => {
            modal_frame(ui, theme, "Key passphrase required", |ui| {
                ui.label(prompt.as_str());
                ui.add_space(6.0);
                let masked = !text.is_empty();
                let resp = ui.add(
                    egui::TextEdit::singleline(text)
                        .password(masked)
                        .desired_width(400.0)
                        .font(egui::TextStyle::Monospace),
                );
                if resp.gained_focus() {
                    resp.request_focus();
                }
                if profile.is_some() {
                    ui.add_space(4.0);
                    ui.checkbox(remember, "Remember in system keyring for this profile");
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if primary_button(ui, theme, "Unlock", 120.0).clicked()
                        || (enter && resp.has_focus())
                    {
                        *submit = true;
                    }
                    if secondary_button(ui, theme, "Cancel", 88.0).clicked() {
                        *cancel = true;
                    }
                });
            });
        }
    }
}

/// Map a [`SshRequest`] into overlay state.
pub fn prompt_from_request(
    source: PromptSource,
    req: SshRequest,
    reply: Sender<Result<String, ()>>,
    profile: Option<String>,
) -> SshPrompt {
    let kind = match req {
        SshRequest::HostKey {
            host,
            port,
            key_type,
            fingerprint,
        } => PromptKind::HostKey {
            host,
            port,
            key_type,
            fingerprint,
        },
        SshRequest::Password { prompt } => PromptKind::Password {
            prompt,
            remember: true,
            profile,
        },
        SshRequest::KeyboardInteractive { prompt } => PromptKind::Keyboard { prompt },
        SshRequest::Passphrase { prompt } => PromptKind::Passphrase {
            prompt,
            remember: true,
            profile,
        },
    };
    SshPrompt {
        source,
        kind,
        reply,
    }
}

/// Draw the agent consent modal: a program on this machine wants
/// something through this app — an SSH session, or with screening on,
/// one command. `remaining` is how long the broker keeps the question
/// open before it denies on the user's behalf.
pub fn show_consent(
    ui: &mut egui::Ui,
    theme: &Theme,
    request: &ConsentRequest,
    remaining: f64,
    decision: &mut Option<ConsentAnswer>,
) {
    match &request.kind {
        ConsentKind::Connect {
            profile,
            user_at_host,
        } => {
            modal_frame(ui, theme, "Agent SSH access", |ui| {
                ui.label(format!(
                    "{} on this machine is asking to open an SSH session through this app.",
                    request.requester
                ));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Profile")
                        .weak()
                        .color(color32(theme.ui.text_dim)),
                );
                ui.monospace(profile.as_str());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Connects as")
                        .weak()
                        .color(color32(theme.ui.text_dim)),
                );
                ui.monospace(user_at_host.as_str());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Allowing opens a session that can run any command as this user. \
                         Every command is written to the local agent audit log. \
                         Unanswered requests close in {} s.",
                        remaining as u32
                    ))
                    .weak()
                    .size(11.5)
                    .color(color32(theme.ui.text_dim)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, theme, "Allow", 110.0).clicked() {
                        *decision = Some(ConsentAnswer::Allow);
                    }
                    if secondary_button(ui, theme, "Deny", 88.0).clicked() {
                        *decision = Some(ConsentAnswer::Deny);
                    }
                });
            });
        }
        ConsentKind::Exec {
            command,
            user_at_host,
        } => {
            modal_frame(ui, theme, "Agent command approval", |ui| {
                ui.label(format!(
                    "{} on this machine wants to run a command over {}:",
                    request.requester, user_at_host
                ));
                ui.add_space(6.0);
                ui.monospace(display_command(command));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Read-only commands run without asking; everything else needs your \
                         approval. \"Allow all future\" stops asking for the rest of this \
                         session. Unanswered requests close in {} s.",
                        remaining as u32
                    ))
                    .weak()
                    .size(11.5)
                    .color(color32(theme.ui.text_dim)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, theme, "Allow", 88.0).clicked() {
                        *decision = Some(ConsentAnswer::Allow);
                    }
                    if secondary_button(ui, theme, "Don't allow", 120.0).clicked() {
                        *decision = Some(ConsentAnswer::Deny);
                    }
                    if secondary_button(ui, theme, "Allow all future", 140.0).clicked() {
                        *decision = Some(ConsentAnswer::AllowAll);
                    }
                });
            });
        }
        ConsentKind::File {
            action,
            local_path,
            remote_path,
            user_at_host,
        } => {
            modal_frame(ui, theme, "Agent file transfer approval", |ui| {
                ui.label(format!(
                    "{} on this machine wants to {} a file over {}:",
                    request.requester, action, user_at_host
                ));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Local")
                        .weak()
                        .color(color32(theme.ui.text_dim)),
                );
                ui.monospace(local_path.as_str());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Remote")
                        .weak()
                        .color(color32(theme.ui.text_dim)),
                );
                ui.monospace(remote_path.as_str());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Transfers use the session's SFTP channel and are capped at 64 MiB. \
                         \"Allow all future\" stops asking for the rest of this session. \
                         Unanswered requests close in {} s.",
                        remaining as u32
                    ))
                    .weak()
                    .size(11.5)
                    .color(color32(theme.ui.text_dim)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, theme, "Allow", 88.0).clicked() {
                        *decision = Some(ConsentAnswer::Allow);
                    }
                    if secondary_button(ui, theme, "Don't allow", 120.0).clicked() {
                        *decision = Some(ConsentAnswer::Deny);
                    }
                    if secondary_button(ui, theme, "Allow all future", 140.0).clicked() {
                        *decision = Some(ConsentAnswer::AllowAll);
                    }
                });
            });
        }
    }
}

/// The command as the modal draws it: control characters become their
/// escapes (nothing moves the cursor or clears the panel), and the
/// very long tail folds into an ellipsis so one modal stays one
/// screen.
fn display_command(command: &str) -> String {
    let mut text = String::new();
    for c in command.chars() {
        match c {
            '\n' => text.push('\n'),
            c if c.is_control() => text.push_str(&format!("\\u{:04x}", c as u32)),
            c => text.push(c),
        }
    }
    if text.chars().count() > 400 {
        let mut cut: String = text.chars().take(400).collect();
        cut.push_str(" …");
        cut
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_lists_builtins_and_profiles() {
        let mut config = Config::default();
        config.profiles.push(SshProfile {
            name: "prod".to_owned(),
            host: "prod.example.com".to_owned(),
            port: 22,
            user: "deploy".to_owned(),
            auth: AuthMethod::Password,
            key_path: None,
            proxy_jump: None,
            forwards: Vec::new(),

            remote_forwards: Vec::new(),
            socks_forwards: Vec::new(),
            agent_forwarding: false,
            group: None,
        });
        let items = palette_items(&config);
        assert_eq!(items.len(), 20, "19 builtins + 1 profile");
        assert!(
            items
                .iter()
                .any(|(label, _)| label.contains("prod.example.com"))
        );
    }

    #[test]
    fn palette_filter_is_case_insensitive_substring() {
        let config = Config::default();
        let all = palette_filtered(&config, "", &[]);
        assert_eq!(all.len(), 19);

        // "CLOSE" also matches "Reopen closed tab" and "Close pane"; the
        // exact prefix wins.
        let close = palette_filtered(&config, "CLOSE", &[]);
        assert_eq!(close.len(), 3);
        assert!(close[0].0.starts_with("Close"));

        let host = palette_filtered(&config, "connect to", &[]);
        assert_eq!(host.len(), 1);
        assert!(matches!(host[0].1, PaletteAction::ConnectAdhoc));

        let info = palette_filtered(&config, "system info", &[]);
        assert_eq!(info.len(), 1);
        assert!(matches!(info[0].1, PaletteAction::SystemInfo));

        let reopen = palette_filtered(&config, "reopen", &[]);
        assert_eq!(reopen.len(), 1);
        assert!(matches!(reopen[0].1, PaletteAction::ReopenTab));

        assert!(palette_filtered(&config, "zzz", &[]).is_empty());
    }
}
