//! The 11 palettes of the reference widget (5 classics + 6 skins), plus the
//! small set of per-theme paint parameters the OSD renderer honours.

use eframe::egui::Color32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarStyle {
    Rounded,
    /// Monospace block glyphs (`█░`), for the terminal skin.
    Ascii,
    /// Sharp rectangles, for brutalist/receipt.
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelCase {
    Normal,
    Upper,
    Lower,
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color32,
    pub bar: Color32,
    pub bar_track: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_dim: Color32,
    pub text_link: Color32,
    pub separator: Color32,
    pub warn: Color32,
    pub crit: Color32,
    pub error: Color32,
    pub live: Color32,
    pub corner_radius: u8,
    pub border_width: f32,
    pub bar_style: BarStyle,
    pub label_case: LabelCase,
    pub title_prefix: &'static str,
    pub monospace: bool,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// bg, bar, track, primary, secondary, dim, link, separator, warn, crit, error, live
type Palette = [u32; 12];

const fn theme(name: &'static str, p: Palette) -> Theme {
    Theme {
        name,
        bg: hex(p[0]),
        bar: hex(p[1]),
        bar_track: hex(p[2]),
        text_primary: hex(p[3]),
        text_secondary: hex(p[4]),
        text_dim: hex(p[5]),
        text_link: hex(p[6]),
        separator: hex(p[7]),
        warn: hex(p[8]),
        crit: hex(p[9]),
        error: hex(p[10]),
        live: hex(p[11]),
        corner_radius: 12,
        border_width: 0.0,
        bar_style: BarStyle::Rounded,
        label_case: LabelCase::Normal,
        title_prefix: "",
        monospace: false,
    }
}

#[rustfmt::skip]
pub const THEMES: &[Theme] = &[
    theme("default", [0x1a1a2e, 0x5B9BD5, 0x333340, 0xe0e0e8, 0x8a8a9a, 0x555568, 0x6BA4D9, 0x2a2a38, 0xf59e0b, 0xdc2626, 0xef4444, 0x4ade80]),
    theme("catppuccin-mocha", [0x1e1e2e, 0x89b4fa, 0x313244, 0xcdd6f4, 0xa6adc8, 0x6c7086, 0x74c7ec, 0x45475a, 0xfab387, 0xf38ba8, 0xeba0ac, 0xa6e3a1]),
    theme("dracula", [0x282a36, 0xbd93f9, 0x44475a, 0xf8f8f2, 0xbfbfbf, 0x6272a4, 0x8be9fd, 0x44475a, 0xffb86c, 0xff5555, 0xff79c6, 0x50fa7b]),
    theme("nord", [0x2e3440, 0x88c0d0, 0x3b4252, 0xeceff4, 0xd8dee9, 0x4c566a, 0x81a1c1, 0x434c5e, 0xebcb8b, 0xbf616a, 0xd08770, 0xa3be8c]),
    theme("gruvbox-dark", [0x282828, 0x83a598, 0x3c3836, 0xebdbb2, 0xbdae93, 0x928374, 0x8ec07c, 0x504945, 0xfabd2f, 0xfb4934, 0xfe8019, 0xb8bb26]),
    Theme {
        corner_radius: 4, bar_style: BarStyle::Ascii, label_case: LabelCase::Lower,
        title_prefix: "┌─ ", monospace: true,
        ..theme("terminal", [0x0a0f0a, 0x5fd787, 0x2e4238, 0xd7e3d7, 0x7a9889, 0x668c75, 0x87d7d7, 0x1d2a22, 0xd7c85f, 0xff6b6b, 0xff6b6b, 0x5fd787])
    },
    Theme {
        corner_radius: 6, label_case: LabelCase::Upper,
        ..theme("dashboard", [0x0f1114, 0x6ea8fe, 0x2b2f3a, 0xe4e6ec, 0x7c808c, 0x565968, 0x6ea8fe, 0x232631, 0xf0b46a, 0xe76a6a, 0xe76a6a, 0x5fd7a5])
    },
    Theme {
        monospace: true,
        ..theme("hud", [0x0c0a08, 0xf5a524, 0x322820, 0xf1e8da, 0x8a7d6a, 0x8a7d6a, 0xa3d468, 0x221c15, 0xf5a524, 0xe5484d, 0xe5484d, 0xa3d468])
    },
    Theme {
        corner_radius: 0, border_width: 1.0, bar_style: BarStyle::Block, label_case: LabelCase::Upper, monospace: true,
        ..theme("receipt", [0xf3efe5, 0x16110a, 0xd8cfb9, 0x16110a, 0x43382a, 0x8a7d68, 0xb4331c, 0xcfc6b2, 0xb4331c, 0xb4331c, 0xb4331c, 0x3a6b3a])
    },
    Theme {
        corner_radius: 4,
        ..theme("strip", [0x0e1012, 0x6be3b6, 0x22262e, 0xe6e8ec, 0x7e8490, 0x5a606c, 0x6be3b6, 0x23272f, 0xe8b15b, 0xe66466, 0xe66466, 0x6be3b6])
    },
    Theme {
        corner_radius: 0, border_width: 2.0, bar_style: BarStyle::Block, label_case: LabelCase::Upper,
        ..theme("brutalist", [0xeeece7, 0xd81f26, 0x1f1f1f, 0x0a0a0a, 0x575757, 0x8b8b8b, 0xd81f26, 0x0a0a0a, 0xd81f26, 0xd81f26, 0xd81f26, 0xd81f26])
    },
];

pub fn get(name: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.name == name).unwrap_or(&THEMES[0])
}

impl Theme {
    /// Bar fill colour by utilisation, same thresholds as the reference OSD.
    pub fn bar_color(&self, pct: f64) -> Color32 {
        if pct < 0.6 {
            self.bar
        } else if pct < 0.85 {
            self.warn
        } else {
            self.crit
        }
    }

    pub fn label(&self, s: &str) -> String {
        match self.label_case {
            LabelCase::Normal => s.to_string(),
            LabelCase::Upper => s.to_uppercase(),
            LabelCase::Lower => s.to_lowercase(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eleven_unique_themes_and_fallback() {
        assert_eq!(THEMES.len(), 11);
        let mut names: Vec<_> = THEMES.iter().map(|t| t.name).collect();
        names.dedup();
        assert_eq!(names.len(), 11);
        assert_eq!(get("nope").name, "default");
        assert_eq!(get("nord").bg, Color32::from_rgb(0x2e, 0x34, 0x40));
    }
}
