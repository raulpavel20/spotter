//! Colors for the diff view: row tints, changed-word tints, and the
//! conversion of syntax-theme colors for the terminal at hand.
//!
//! Nothing here queries the terminal; detection uses environment
//! variables only (`COLORTERM`, `COLORFGBG`) plus `spotter.theme`.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub mode: ColorMode,
    pub background: Background,
    pub minus: Color,
    pub minus_emph: Color,
    pub plus: Color,
    pub plus_emph: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Palette::new(ColorMode::TrueColor, Background::Dark)
    }
}

impl Palette {
    pub fn new(mode: ColorMode, background: Background) -> Palette {
        let mut p = Palette {
            mode,
            background,
            minus: Color::Reset,
            minus_emph: Color::Reset,
            plus: Color::Reset,
            plus_emph: Color::Reset,
        };
        // Roughly delta's defaults.
        let (minus, minus_emph, plus, plus_emph) = match background {
            Background::Dark => (
                (0x3f, 0x00, 0x01),
                (0x90, 0x10, 0x11),
                (0x00, 0x28, 0x00),
                (0x00, 0x60, 0x00),
            ),
            Background::Light => (
                (0xff, 0xe0, 0xe0),
                (0xff, 0xc0, 0xc0),
                (0xd0, 0xff, 0xd0),
                (0xa0, 0xef, 0xa0),
            ),
        };
        if mode == ColorMode::Ansi256 {
            // Nearest-color reduction would turn the dark tints gray, so
            // use hand-picked palette entries that keep their hue.
            let (m, me, pl, pe) = match background {
                Background::Dark => (52, 88, 22, 28),
                Background::Light => (224, 217, 194, 157),
            };
            p.minus = Color::Indexed(m);
            p.minus_emph = Color::Indexed(me);
            p.plus = Color::Indexed(pl);
            p.plus_emph = Color::Indexed(pe);
            return p;
        }
        p.minus = p.rgb(minus);
        p.minus_emph = p.rgb(minus_emph);
        p.plus = p.rgb(plus);
        p.plus_emph = p.rgb(plus_emph);
        p
    }

    /// From `spotter.theme` (`auto`, `dark`, `light`) and the environment.
    pub fn detect(
        theme: Option<&str>,
        colorterm: Option<&str>,
        colorfgbg: Option<&str>,
    ) -> Palette {
        let mode = match colorterm.map(str::to_ascii_lowercase).as_deref() {
            Some("truecolor" | "24bit") => ColorMode::TrueColor,
            _ => ColorMode::Ansi256,
        };
        let background = match theme.map(str::to_ascii_lowercase).as_deref() {
            Some("light") => Background::Light,
            Some("dark") => Background::Dark,
            _ => colorfgbg
                .and_then(background_from_colorfgbg)
                .unwrap_or(Background::Dark),
        };
        Palette::new(mode, background)
    }

    pub fn rgb(&self, (r, g, b): (u8, u8, u8)) -> Color {
        match self.mode {
            ColorMode::TrueColor => Color::Rgb(r, g, b),
            ColorMode::Ansi256 => Color::Indexed(nearest_256(r, g, b)),
        }
    }

    /// A syntax-theme color. Following bat's convention, alpha 0 means
    /// "ANSI palette index in the red channel" and alpha 1 means "the
    /// terminal's default color" (`None`).
    pub fn theme_color(&self, r: u8, g: u8, b: u8, a: u8) -> Option<Color> {
        match a {
            0 => Some(ansi(r)),
            1 => None,
            _ => Some(self.rgb((r, g, b))),
        }
    }
}

fn ansi(i: u8) -> Color {
    match i {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        i => Color::Indexed(i),
    }
}

/// `COLORFGBG` is `fg;bg` (sometimes `fg;default;bg`); a light background
/// is palette entry 7 or 9–15.
pub fn background_from_colorfgbg(s: &str) -> Option<Background> {
    let bg: u8 = s.rsplit(';').next()?.trim().parse().ok()?;
    Some(if bg == 7 || (9..=15).contains(&bg) {
        Background::Light
    } else {
        Background::Dark
    })
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn cube_index(v: u8) -> usize {
    match v {
        0..=47 => 0,
        48..=114 => 1,
        v => ((v as usize - 35) / 40).min(5),
    }
}

fn dist((r1, g1, b1): (u8, u8, u8), (r2, g2, b2): (u8, u8, u8)) -> u32 {
    let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2) as u32;
    d(r1, r2) + d(g1, g2) + d(b1, b2)
}

/// The closest xterm-256 color (6×6×6 cube or grayscale ramp).
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let (ri, gi, bi) = (cube_index(r), cube_index(g), cube_index(b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gi = ((avg.saturating_sub(8)) / 10).min(23) as u8;
    let gray_v = 8 + 10 * gi;
    let gray = (gray_v, gray_v, gray_v);
    if dist((r, g, b), gray) < dist((r, g, b), cube) {
        232 + gi
    } else {
        cube_idx as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_background_and_mode() {
        assert_eq!(background_from_colorfgbg("15;0"), Some(Background::Dark));
        assert_eq!(background_from_colorfgbg("0;15"), Some(Background::Light));
        assert_eq!(
            background_from_colorfgbg("0;default;7"),
            Some(Background::Light)
        );
        assert_eq!(background_from_colorfgbg("junk"), None);
        let p = Palette::detect(None, Some("truecolor"), Some("0;15"));
        assert_eq!(
            (p.mode, p.background),
            (ColorMode::TrueColor, Background::Light)
        );
        let p = Palette::detect(Some("dark"), None, Some("0;15"));
        assert_eq!(
            (p.mode, p.background),
            (ColorMode::Ansi256, Background::Dark)
        );
        assert_eq!(p.minus, Color::Indexed(52));
    }

    #[test]
    fn reduces_to_256_colors() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(128, 128, 128), 244);
        // Very dark colors land on the grayscale ramp…
        assert_eq!(nearest_256(0x00, 0x28, 0x00), 232);
        // …which is why the 256-color diff tints are picked by hand.
        assert_eq!(
            Palette::new(ColorMode::Ansi256, Background::Dark).plus,
            Color::Indexed(22)
        );
    }

    #[test]
    fn decodes_theme_colors() {
        let p = Palette::default();
        assert_eq!(p.theme_color(2, 0, 0, 0), Some(Color::Green));
        assert_eq!(p.theme_color(0, 0, 0, 1), None);
        assert_eq!(p.theme_color(1, 2, 3, 0xff), Some(Color::Rgb(1, 2, 3)));
    }
}
