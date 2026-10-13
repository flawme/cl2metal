//! Built-in themes: terminal palette + UI chrome colors.

use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

#[derive(Clone, Debug)]
pub struct UiTheme {
    pub bar_bg: [u8; 3],
    pub tab_active_bg: [u8; 3],
    pub tab_inactive_fg: [u8; 3],
    pub tab_hover_bg: [u8; 3],
    pub accent: [u8; 3],
    pub border: [u8; 3],
    pub panel_bg: [u8; 3],
    pub text: [u8; 3],
    pub text_dim: [u8; 3],
    pub danger: [u8; 3],
    pub success: [u8; 3],
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub bg: [u8; 3],
    pub fg: [u8; 3],
    pub cursor: [u8; 3],
    /// Selection background (alpha applied at render time).
    pub selection: [u8; 3],
    pub selection_alpha: f32,
    /// Dim color multiplier applied to Dim-flagged cells.
    pub dim_factor: f32,
    /// 16 ANSI colors: normal 0..8, bright 8..16.
    pub ansi: [[u8; 3]; 16],
    pub ui: UiTheme,
    /// Opacity of the chrome (tab bar, panels, modals). Lowered when a
    /// background image is set so the image shows through.
    pub ui_alpha: f32,
}

impl Theme {
    /// Resolve a grid color to RGB.
    pub fn resolve(&self, color: Color) -> [u8; 3] {
        match color {
            Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
            Color::Named(named) => self.named(named),
            Color::Indexed(i) => self.indexed(i),
        }
    }

    fn named(&self, named: NamedColor) -> [u8; 3] {
        use NamedColor::*;
        match named {
            Foreground => self.fg,
            Background => self.bg,
            Cursor => self.cursor,
            BrightForeground => self.ansi[15],
            DimForeground => dim(self.fg, self.dim_factor),
            DimBlack => dim(self.ansi[0], self.dim_factor),
            DimRed => dim(self.ansi[1], self.dim_factor),
            DimGreen => dim(self.ansi[2], self.dim_factor),
            DimYellow => dim(self.ansi[3], self.dim_factor),
            DimBlue => dim(self.ansi[4], self.dim_factor),
            DimMagenta => dim(self.ansi[5], self.dim_factor),
            DimCyan => dim(self.ansi[6], self.dim_factor),
            DimWhite => dim(self.ansi[7], self.dim_factor),
            // Black..BrightWhite map to 0..16.
            other => self.ansi[other as usize],
        }
    }

    /// xterm-256 indexed color.
    pub fn indexed(&self, i: u8) -> [u8; 3] {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let n = i - 16;
                let (r, g, b) = (n / 36, (n / 6) % 6, n % 6);
                let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                [level(r), level(g), level(b)]
            }
            232..=255 => {
                let v = 8 + 10 * (i - 232);
                [v, v, v]
            }
        }
    }

    pub fn rgb(hex: u32) -> [u8; 3] {
        [
            ((hex >> 16) & 0xff) as u8,
            ((hex >> 8) & 0xff) as u8,
            (hex & 0xff) as u8,
        ]
    }
}

fn dim(c: [u8; 3], f: f32) -> [u8; 3] {
    [
        (c[0] as f32 * f) as u8,
        (c[1] as f32 * f) as u8,
        (c[2] as f32 * f) as u8,
    ]
}

/// Slightly-decorated RGB into vte Rgb.
pub fn to_vte(c: [u8; 3]) -> Rgb {
    Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

pub fn builtin(id: &str) -> Theme {
    match id {
        "graphite" => graphite(),
        "paper" => paper(),
        _ => midnight(),
    }
}

/// Apply `[colors]` overrides from the config on top of a built-in theme.
///
/// Keys are theme field names (`accent`, `bg`, `text`, `ansi4`, …) and values
/// are `#rrggbb` strings; unknown keys are ignored.
pub fn apply_overrides(theme: &mut Theme, colors: &std::collections::BTreeMap<String, String>) {
    for (key, value) in colors {
        let Some(rgb) = parse_hex(value) else {
            log::warn!("ignoring colour override {key} = {value:?}");
            continue;
        };
        match key.as_str() {
            "bg" => theme.bg = rgb,
            "fg" => theme.fg = rgb,
            "cursor" => theme.cursor = rgb,
            "selection" => theme.selection = rgb,
            "bar_bg" => theme.ui.bar_bg = rgb,
            "tab_active_bg" => theme.ui.tab_active_bg = rgb,
            "tab_inactive_fg" => theme.ui.tab_inactive_fg = rgb,
            "tab_hover_bg" => theme.ui.tab_hover_bg = rgb,
            "accent" => theme.ui.accent = rgb,
            "border" => theme.ui.border = rgb,
            "panel_bg" => theme.ui.panel_bg = rgb,
            "text" => theme.ui.text = rgb,
            "text_dim" => theme.ui.text_dim = rgb,
            "danger" => theme.ui.danger = rgb,
            "success" => theme.ui.success = rgb,
            other => {
                if let Some(index) = other
                    .strip_prefix("ansi")
                    .and_then(|n| n.parse::<usize>().ok())
                    && index < 16
                {
                    theme.ansi[index] = rgb;
                }
            }
        }
    }
}

fn parse_hex(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(Theme::rgb)
}

pub fn all_ids() -> Vec<&'static str> {
    vec!["midnight", "graphite", "paper"]
}

fn midnight() -> Theme {
    let h = Theme::rgb;
    Theme {
        id: "midnight",
        name: "Midnight",
        dark: true,
        bg: h(0x10141c),
        fg: h(0xd7dce5),
        cursor: h(0x7aa2f7),
        selection: h(0x3b4261),
        selection_alpha: 0.55,
        dim_factor: 0.65,
        ansi: [
            h(0x1a2030), // black
            h(0xf7768e), // red
            h(0x9ece6a), // green
            h(0xe0af68), // yellow
            h(0x7aa2f7), // blue
            h(0xbb9af7), // magenta
            h(0x7dcfff), // cyan
            h(0xc0caf5), // white
            h(0x414868), // bright black
            h(0xff899d), // bright red
            h(0xb9f27c), // bright green
            h(0xf5c877), // bright yellow
            h(0x9db9ff), // bright blue
            h(0xd0a9ff), // bright magenta
            h(0x9be4ff), // bright cyan
            h(0xe2e8f5), // bright white
        ],
        ui: UiTheme {
            bar_bg: h(0x0a0e15),
            tab_active_bg: h(0x10141c),
            tab_inactive_fg: h(0x6b7385),
            tab_hover_bg: h(0x161b26),
            accent: h(0x7aa2f7),
            border: h(0x1c2230),
            panel_bg: h(0x0d111a),
            text: h(0xd7dce5),
            text_dim: h(0x6b7385),
            danger: h(0xf7768e),
            success: h(0x9ece6a),
        },
        ui_alpha: 1.0,
    }
}

fn graphite() -> Theme {
    let h = Theme::rgb;
    Theme {
        id: "graphite",
        name: "Graphite",
        dark: true,
        bg: h(0x17191c),
        fg: h(0xd2d5da),
        cursor: h(0xe6e6e6),
        selection: h(0x3a3f46),
        selection_alpha: 0.6,
        dim_factor: 0.6,
        ansi: [
            h(0x24272c),
            h(0xe06c75),
            h(0x98c379),
            h(0xe5c07b),
            h(0x61afef),
            h(0xc678dd),
            h(0x56b6c2),
            h(0xabb2bf),
            h(0x5c6370),
            h(0xef7a85),
            h(0xa9d98a),
            h(0xebc58d),
            h(0x7fc0ff),
            h(0xd890e8),
            h(0x6ad0de),
            h(0xd8dee9),
        ],
        ui: UiTheme {
            bar_bg: h(0x111315),
            tab_active_bg: h(0x17191c),
            tab_inactive_fg: h(0x6d747e),
            tab_hover_bg: h(0x1e2126),
            accent: h(0x61afef),
            border: h(0x24272c),
            panel_bg: h(0x131518),
            text: h(0xd2d5da),
            text_dim: h(0x6d747e),
            danger: h(0xe06c75),
            success: h(0x98c379),
        },
        ui_alpha: 1.0,
    }
}

fn paper() -> Theme {
    let h = Theme::rgb;
    Theme {
        id: "paper",
        name: "Paper",
        dark: false,
        bg: h(0xfafafa),
        fg: h(0x24292e),
        cursor: h(0x2ea8ee),
        selection: h(0xbbd9f2),
        selection_alpha: 0.7,
        dim_factor: 0.7,
        ansi: [
            h(0x24292e),
            h(0xcb2431),
            h(0x148535),
            h(0xb08800),
            h(0x005cc5),
            h(0x8250df),
            h(0x0a7c86),
            h(0xd1d5da),
            h(0x6a737d),
            h(0xd73a49),
            h(0x22863a),
            h(0xe36209),
            h(0x005cc5),
            h(0x8250df),
            h(0x0a7c86),
            h(0x24292e),
        ],
        ui: UiTheme {
            bar_bg: h(0xeef0f2),
            tab_active_bg: h(0xfafafa),
            tab_inactive_fg: h(0x8c959f),
            tab_hover_bg: h(0xe6e8eb),
            accent: h(0x005cc5),
            border: h(0xd8dee4),
            panel_bg: h(0xf3f4f6),
            text: h(0x24292e),
            text_dim: h(0x8c959f),
            danger: h(0xcb2431),
            success: h(0x148535),
        },
        ui_alpha: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_colors_cover_the_xterm_ranges() {
        let theme = builtin("midnight");
        // 0..16 map straight to the ANSI palette.
        assert_eq!(theme.indexed(0), theme.ansi[0]);
        assert_eq!(theme.indexed(15), theme.ansi[15]);
        // 6x6x6 colour cube: 196 is pure red.
        assert_eq!(theme.indexed(196), [255, 0, 0]);
        assert_eq!(theme.indexed(16), [0, 0, 0]);
        assert_eq!(theme.indexed(231), [255, 255, 255]);
        // Greyscale ramp.
        assert_eq!(theme.indexed(232), [8, 8, 8]);
        assert_eq!(theme.indexed(255), [238, 238, 238]);
    }

    #[test]
    fn named_colors_resolve_to_the_palette() {
        let theme = builtin("midnight");
        assert_eq!(theme.resolve(Color::Named(NamedColor::Red)), theme.ansi[1]);
        assert_eq!(
            theme.resolve(Color::Named(NamedColor::Foreground)),
            theme.fg
        );
        assert_eq!(
            theme.resolve(Color::Named(NamedColor::Background)),
            theme.bg
        );
        assert_eq!(
            theme.resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 })),
            [1, 2, 3]
        );
        // Dim variants are scaled down.
        let dim = theme.resolve(Color::Named(NamedColor::DimRed));
        assert!(dim[0] < theme.ansi[1][0]);
    }

    #[test]
    fn builtin_themes_are_addressable_by_id() {
        for id in all_ids() {
            let theme = builtin(id);
            assert_eq!(theme.id, id);
        }
        // Unknown ids fall back to the default theme.
        assert_eq!(builtin("nope").id, "midnight");
    }

    #[test]
    fn colour_overrides_apply_to_theme_and_palette() {
        let mut theme = builtin("midnight");
        let mut colors = std::collections::BTreeMap::new();
        colors.insert("accent".to_owned(), "#ff8800".to_owned());
        colors.insert("bg".to_owned(), "000000".to_owned());
        colors.insert("ansi4".to_owned(), "#123456".to_owned());
        colors.insert("nonsense".to_owned(), "not-a-colour".to_owned());
        colors.insert("ansi99".to_owned(), "#ffffff".to_owned());
        apply_overrides(&mut theme, &colors);
        assert_eq!(theme.ui.accent, [0xff, 0x88, 0x00]);
        assert_eq!(theme.bg, [0, 0, 0]);
        assert_eq!(theme.ansi[4], [0x12, 0x34, 0x56]);
        // Bad values are ignored, not fatal.
        assert_eq!(theme.ui.text, builtin("midnight").ui.text);
    }

    #[test]
    fn hex_helper_splits_channels() {
        assert_eq!(Theme::rgb(0x7aa2f7), [0x7a, 0xa2, 0xf7]);
    }
}
