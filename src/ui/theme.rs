//! Colour themes. True colour by default, with a 256-colour fallback applied to
//! the whole frame buffer so effects degrade too.

use ratatui::buffer::Buffer;
use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub panel: Color,
    pub grid: Color,
    pub border: Color,
    pub border_hi: Color,
    pub text: Color,
    pub bright: Color,
    pub muted: Color,
    pub dim: Color,
    pub primary: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub ok: Color,
    pub warn: Color,
    pub danger: Color,
    pub l_input: Color,
    pub l_cache_read: Color,
    pub l_cache_write: Color,
    pub l_output: Color,
    pub l_thinking: Color,
    /// Gauge gradient stops, low to high.
    pub gauge: [Color; 3],
}

pub const NAMES: [&str; 4] = ["neon", "synthwave", "matrix", "mono"];

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    pub fn by_name(name: &str) -> Theme {
        match name {
            "synthwave" => Theme::synthwave(),
            "matrix" => Theme::matrix(),
            "mono" => Theme::mono(),
            _ => Theme::neon(),
        }
    }

    pub fn next(&self) -> Theme {
        let i = NAMES.iter().position(|n| *n == self.name).unwrap_or(0);
        Theme::by_name(NAMES[(i + 1) % NAMES.len()])
    }

    pub fn neon() -> Theme {
        Theme {
            name: "neon",
            bg: rgb(0x07080d),
            panel: rgb(0x0b0d16),
            grid: rgb(0x1a1d2b),
            border: rgb(0x2a2f45),
            border_hi: rgb(0x00e5ff),
            text: rgb(0xc8cce0),
            bright: rgb(0xffffff),
            muted: rgb(0x7c819a),
            dim: rgb(0x454a63),
            primary: rgb(0xff2bd6),
            secondary: rgb(0x00e5ff),
            tertiary: rgb(0xf4ff5a),
            ok: rgb(0x39ff88),
            warn: rgb(0xff9e2c),
            danger: rgb(0xff3b3b),
            l_input: rgb(0x00e5ff),
            l_cache_read: rgb(0x0f5f73),
            l_cache_write: rgb(0xff2bd6),
            l_output: rgb(0xf4ff5a),
            l_thinking: rgb(0xff9e2c),
            gauge: [rgb(0x00e5ff), rgb(0xf4ff5a), rgb(0xff2bd6)],
        }
    }

    pub fn synthwave() -> Theme {
        Theme {
            name: "synthwave",
            bg: rgb(0x110a1f),
            panel: rgb(0x170f2a),
            grid: rgb(0x2a1a47),
            border: rgb(0x46306e),
            border_hi: rgb(0xff6ad5),
            text: rgb(0xe3d7ff),
            bright: rgb(0xffffff),
            muted: rgb(0x9a86c4),
            dim: rgb(0x5a4880),
            primary: rgb(0xff6ad5),
            secondary: rgb(0x26f7fd),
            tertiary: rgb(0xffd319),
            ok: rgb(0x72f1b8),
            warn: rgb(0xff8b39),
            danger: rgb(0xfe4450),
            l_input: rgb(0x26f7fd),
            l_cache_read: rgb(0x3b2f7a),
            l_cache_write: rgb(0xff6ad5),
            l_output: rgb(0xffd319),
            l_thinking: rgb(0xff8b39),
            gauge: [rgb(0x26f7fd), rgb(0xffd319), rgb(0xfe4450)],
        }
    }

    pub fn matrix() -> Theme {
        Theme {
            name: "matrix",
            bg: rgb(0x000700),
            panel: rgb(0x000c02),
            grid: rgb(0x032b0c),
            border: rgb(0x0a4a1a),
            border_hi: rgb(0x00ff41),
            text: rgb(0x9cffb0),
            bright: rgb(0xe0ffe6),
            muted: rgb(0x2f8f46),
            dim: rgb(0x14522a),
            primary: rgb(0x00ff41),
            secondary: rgb(0x7dff9b),
            tertiary: rgb(0xd1ff6a),
            ok: rgb(0x00ff41),
            warn: rgb(0xd1ff6a),
            danger: rgb(0xff4141),
            l_input: rgb(0x7dff9b),
            l_cache_read: rgb(0x0b5a22),
            l_cache_write: rgb(0x00ff41),
            l_output: rgb(0xd1ff6a),
            l_thinking: rgb(0x9cff00),
            gauge: [rgb(0x0b8f2e), rgb(0x00ff41), rgb(0xd1ff6a)],
        }
    }

    pub fn mono() -> Theme {
        Theme {
            name: "mono",
            bg: Color::Reset,
            panel: Color::Reset,
            grid: Color::DarkGray,
            border: Color::DarkGray,
            border_hi: Color::White,
            text: Color::Gray,
            bright: Color::White,
            muted: Color::DarkGray,
            dim: Color::DarkGray,
            primary: Color::White,
            secondary: Color::Gray,
            tertiary: Color::White,
            ok: Color::Gray,
            warn: Color::White,
            danger: Color::White,
            l_input: Color::White,
            l_cache_read: Color::DarkGray,
            l_cache_write: Color::Gray,
            l_output: Color::White,
            l_thinking: Color::Gray,
            gauge: [Color::Gray, Color::White, Color::White],
        }
    }

    /// Colour for a percentage: calm, caution, warning, danger.
    pub fn level(&self, pct: f64) -> Color {
        if pct >= 90.0 {
            self.danger
        } else if pct >= 80.0 {
            self.warn
        } else if pct >= 50.0 {
            self.tertiary
        } else {
            self.secondary
        }
    }

    /// Gauge gradient colour at position t in 0..=1.
    pub fn gauge_at(&self, t: f64) -> Color {
        let t = t.clamp(0.0, 1.0);
        if t < 0.5 {
            lerp(self.gauge[0], self.gauge[1], t * 2.0)
        } else {
            lerp(self.gauge[1], self.gauge[2], (t - 0.5) * 2.0)
        }
    }
}

/// Linear blend of two RGB colours; non-RGB colours snap at the midpoint.
pub fn lerp(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

/// Multiply an RGB colour's brightness.
pub fn shade(c: Color, f: f64) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let m = |x: u8| (x as f64 * f).clamp(0.0, 255.0) as u8;
            Color::Rgb(m(r), m(g), m(b))
        }
        other => other,
    }
}

/// Does the terminal advertise 24-bit colour?
pub fn truecolor_supported() -> bool {
    std::env::var("COLORTERM").map(|v| v.contains("truecolor") || v.contains("24bit")).unwrap_or(false)
        || std::env::var("TERM_PROGRAM")
            .map(|p| matches!(p.as_str(), "iTerm.app" | "WezTerm" | "ghostty" | "vscode" | "WarpTerminal"))
            .unwrap_or(false)
}

pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Nearest xterm-256 index for an RGB colour.
pub fn to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let gray = r == g && g == b;
    if gray {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return 232 + ((r as u16 - 8) * 24 / 247) as u8;
    }
    let q = |x: u8| -> u8 {
        if x < 48 {
            0
        } else if x < 115 {
            1
        } else {
            ((x as u16 - 35) / 40) as u8
        }
    };
    16 + 36 * q(r) + 6 * q(g) + q(b)
}

/// Rewrite every RGB colour in the buffer to its 256-colour equivalent.
pub fn downsample(buf: &mut Buffer) {
    for cell in buf.content.iter_mut() {
        if let Color::Rgb(r, g, b) = cell.fg {
            cell.fg = Color::Indexed(to_ansi256(r, g, b));
        }
        if let Color::Rgb(r, g, b) = cell.bg {
            cell.bg = Color::Indexed(to_ansi256(r, g, b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_and_levels() {
        assert_eq!(lerp(Color::Rgb(0, 0, 0), Color::Rgb(200, 100, 50), 0.5), Color::Rgb(100, 50, 25));
        let t = Theme::neon();
        assert_eq!(t.level(95.0), t.danger);
        assert_eq!(t.level(10.0), t.secondary);
        assert_eq!(t.next().name, "synthwave");
        assert_eq!(Theme::by_name("mono").next().name, "neon");
    }

    #[test]
    fn ansi256_mapping() {
        assert_eq!(to_ansi256(0, 0, 0), 16);
        assert_eq!(to_ansi256(255, 255, 255), 231);
        assert_eq!(to_ansi256(255, 0, 0), 196);
        assert_eq!(to_ansi256(0, 229, 255), 45); // #00d7ff
    }
}
