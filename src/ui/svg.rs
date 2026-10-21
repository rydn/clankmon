//! Render a ratatui buffer to SVG, pixel-exact on any viewer.
//!
//! Terminal screenshots usually break in browsers: block, braille and box
//! glyphs come from whatever fonts the viewer has, leaving gaps between rows
//! and misaligned columns. Here those glyphs are drawn as vector shapes on the
//! cell grid; only ordinary text uses fonts, in runs stretched to their exact
//! cell width. Animated output layers row diffs over the first frame with
//! discrete SMIL timing, which GitHub renders inside `<img>`.

use std::fmt::Write as _;

use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier};

#[derive(Debug, Clone)]
pub struct SvgOptions {
    pub cell_w: f64,
    pub cell_h: f64,
    pub font_size: f64,
    /// Draw a window frame with this title.
    pub chrome: Option<String>,
    /// Colours used for `Color::Reset`.
    pub fg: Color,
    pub bg: Color,
    /// Accent colours for the window frame (border glow, title).
    pub accent: Color,
    pub accent2: Color,
    /// Faint CRT scanlines over the terminal area.
    pub scanlines: bool,
    /// Prefix for element ids, so several renders can be nested in one SVG.
    pub id: String,
}

impl Default for SvgOptions {
    fn default() -> Self {
        SvgOptions {
            cell_w: 8.4,
            cell_h: 17.0,
            font_size: 13.6,
            chrome: None,
            fg: Color::Rgb(0xc8, 0xcc, 0xe0),
            bg: Color::Rgb(0x07, 0x08, 0x0d),
            accent: Color::Rgb(0xff, 0x2b, 0xd6),
            accent2: Color::Rgb(0x00, 0xe5, 0xff),
            scanlines: true,
            id: "c".into(),
        }
    }
}

const FONT: &str = "'JetBrains Mono','SF Mono','Cascadia Mono',Menlo,Consolas,'DejaVu Sans Mono',monospace";
const PAD: f64 = 14.0;
const BAR: f64 = 30.0;

/// Pixel geometry of the terminal area inside the frame.
struct Geo {
    cw: f64,
    ch: f64,
    x0: f64,
    y0: f64,
    width: f64,
    height: f64,
}

impl Geo {
    fn new(w: u16, h: u16, o: &SvgOptions) -> Geo {
        let (x0, y0) = if o.chrome.is_some() { (PAD, PAD + BAR) } else { (0.0, 0.0) };
        let tw = w as f64 * o.cell_w;
        let th = h as f64 * o.cell_h;
        let (width, height) = if o.chrome.is_some() { (tw + 2.0 * PAD, th + BAR + 2.0 * PAD) } else { (tw, th) };
        Geo { cw: o.cell_w, ch: o.cell_h, x0, y0, width, height }
    }
    fn x(&self, col: u16) -> f64 {
        self.x0 + col as f64 * self.cw
    }
    fn y(&self, row: u16) -> f64 {
        self.y0 + row as f64 * self.ch
    }
}

/// `#rrggbb` for a colour, resolving `Reset` and indexed colours.
pub fn hex(c: Color, default: Color) -> String {
    let (r, g, b) = rgb(c, default);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn rgb(c: Color, default: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => match default {
            Color::Reset => (0, 0, 0),
            d => rgb(d, Color::Black),
        },
        Color::Indexed(i) => indexed(i),
        Color::Black => indexed(0),
        Color::Red => indexed(1),
        Color::Green => indexed(2),
        Color::Yellow => indexed(3),
        Color::Blue => indexed(4),
        Color::Magenta => indexed(5),
        Color::Cyan => indexed(6),
        Color::Gray => indexed(7),
        Color::DarkGray => indexed(8),
        Color::LightRed => indexed(9),
        Color::LightGreen => indexed(10),
        Color::LightYellow => indexed(11),
        Color::LightBlue => indexed(12),
        Color::LightMagenta => indexed(13),
        Color::LightCyan => indexed(14),
        Color::White => indexed(15),
    }
}

/// The xterm 256-colour palette.
fn indexed(i: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let n = i - 16;
            let lv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (lv(n / 36), lv((n / 6) % 6), lv(n % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            (g, g, g)
        }
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

/// A rectangle or line segment in cell fractions: (x, y, w, h) or (x1, y1, x2, y2).
type Quad = (f64, f64, f64, f64);

/// Rectangles for block elements, with a fill opacity (for shades).
fn block_rects(c: char) -> Option<(&'static [Quad], f64)> {
    const FULL: &[Quad] = &[(0.0, 0.0, 1.0, 1.0)];
    let r: &'static [Quad] = match c {
        '▀' => &[(0.0, 0.0, 1.0, 0.5)],
        '▁' => &[(0.0, 0.875, 1.0, 0.125)],
        '▂' => &[(0.0, 0.75, 1.0, 0.25)],
        '▃' => &[(0.0, 0.625, 1.0, 0.375)],
        '▄' => &[(0.0, 0.5, 1.0, 0.5)],
        '▅' => &[(0.0, 0.375, 1.0, 0.625)],
        '▆' => &[(0.0, 0.25, 1.0, 0.75)],
        '▇' => &[(0.0, 0.125, 1.0, 0.875)],
        '█' => FULL,
        '▉' => &[(0.0, 0.0, 0.875, 1.0)],
        '▊' => &[(0.0, 0.0, 0.75, 1.0)],
        '▋' => &[(0.0, 0.0, 0.625, 1.0)],
        '▌' => &[(0.0, 0.0, 0.5, 1.0)],
        '▍' => &[(0.0, 0.0, 0.375, 1.0)],
        '▎' => &[(0.0, 0.0, 0.25, 1.0)],
        '▏' => &[(0.0, 0.0, 0.125, 1.0)],
        '▐' => &[(0.5, 0.0, 0.5, 1.0)],
        '▔' => &[(0.0, 0.0, 1.0, 0.125)],
        '▕' => &[(0.875, 0.0, 0.125, 1.0)],
        '▖' => &[(0.0, 0.5, 0.5, 0.5)],
        '▗' => &[(0.5, 0.5, 0.5, 0.5)],
        '▘' => &[(0.0, 0.0, 0.5, 0.5)],
        '▝' => &[(0.5, 0.0, 0.5, 0.5)],
        '▙' => &[(0.0, 0.0, 0.5, 1.0), (0.5, 0.5, 0.5, 0.5)],
        '▚' => &[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 0.5, 0.5)],
        '▛' => &[(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 0.5)],
        '▜' => &[(0.0, 0.0, 1.0, 0.5), (0.5, 0.5, 0.5, 0.5)],
        '▞' => &[(0.5, 0.0, 0.5, 0.5), (0.0, 0.5, 0.5, 0.5)],
        '▟' => &[(0.5, 0.0, 0.5, 1.0), (0.0, 0.5, 0.5, 0.5)],
        '░' => return Some((FULL, 0.25)),
        '▒' => return Some((FULL, 0.5)),
        '▓' => return Some((FULL, 0.75)),
        _ => return None,
    };
    Some((r, 1.0))
}

/// Line segments for box drawing, in cell fractions: (x1, y1, x2, y2).
/// Returns segments, stroke weight (1 light, 2 heavy) and an optional dash count.
fn box_lines(c: char) -> Option<(&'static [Quad], f64, Option<u32>)> {
    const H: Quad = (0.0, 0.5, 1.0, 0.5);
    const V: Quad = (0.5, 0.0, 0.5, 1.0);
    const L: Quad = (0.0, 0.5, 0.5, 0.5);
    const R: Quad = (0.5, 0.5, 1.0, 0.5);
    const U: Quad = (0.5, 0.0, 0.5, 0.5);
    const D: Quad = (0.5, 0.5, 0.5, 1.0);
    Some(match c {
        '─' => (&[H], 1.0, None),
        '│' => (&[V], 1.0, None),
        '━' => (&[H], 2.0, None),
        '┃' => (&[V], 2.0, None),
        '┈' => (&[H], 1.0, Some(4)),
        '┊' => (&[V], 1.0, Some(4)),
        '╌' => (&[H], 1.0, Some(2)),
        '┤' => (&[V, L], 1.0, None),
        '├' => (&[V, R], 1.0, None),
        '┬' => (&[H, D], 1.0, None),
        '┴' => (&[H, U], 1.0, None),
        '┼' => (&[H, V], 1.0, None),
        '┌' => (&[R, D], 1.0, None),
        '┐' => (&[L, D], 1.0, None),
        '└' => (&[R, U], 1.0, None),
        '┘' => (&[L, U], 1.0, None),
        '┏' => (&[R, D], 2.0, None),
        '┓' => (&[L, D], 2.0, None),
        '┗' => (&[R, U], 2.0, None),
        '┛' => (&[L, U], 2.0, None),
        _ => return None,
    })
}

/// Rounded corners: (start, control, end) in cell fractions.
fn arc(c: char) -> Option<[(f64, f64); 3]> {
    Some(match c {
        '╭' => [(1.0, 0.5), (0.5, 0.5), (0.5, 1.0)],
        '╮' => [(0.0, 0.5), (0.5, 0.5), (0.5, 1.0)],
        '╰' => [(1.0, 0.5), (0.5, 0.5), (0.5, 0.0)],
        '╯' => [(0.0, 0.5), (0.5, 0.5), (0.5, 0.0)],
        _ => return None,
    })
}

/// Can consecutive copies of this glyph merge into one shape?
fn mergeable(c: char) -> bool {
    matches!(c, '█' | '─' | '━' | '┈' | '▔' | '▁' | '▀' | '▄')
}

fn is_vector(c: char) -> bool {
    block_rects(c).is_some()
        || box_lines(c).is_some()
        || arc(c).is_some()
        || ('\u{2800}'..='\u{28ff}').contains(&c)
        || matches!(c, '▰' | '▱' | '▮' | '◢' | '◣')
}

/// Draw `n` copies of vector glyph `c` starting at pixel (x, y).
fn vector(out: &mut String, c: char, n: u16, x: f64, y: f64, g: &Geo, fill: &str) {
    let (cw, ch) = (g.cw, g.ch);
    let w = cw * n as f64;
    let sw = (cw * 0.13).max(1.0);
    if let Some((rects, opacity)) = block_rects(c) {
        for (rx, ry, rw, rh) in rects {
            // A run of a full-width glyph is one long rect; partial ones repeat per cell.
            let reps = if *rw >= 1.0 { 1 } else { n };
            for i in 0..reps {
                let cx = x + i as f64 * cw;
                let width = if *rw >= 1.0 { w } else { rw * cw };
                let _ = write!(
                    out,
                    r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{fill}"{}/>"#,
                    cx + rx * cw,
                    y + ry * ch,
                    width + 0.04,
                    rh * ch + 0.04,
                    if opacity < 1.0 { format!(r#" fill-opacity="{opacity}""#) } else { String::new() }
                );
            }
        }
        return;
    }
    if let Some((lines, weight, dashes)) = box_lines(c) {
        let horizontal_run = lines.len() == 1 && lines[0].1 == lines[0].3;
        for (x1, y1, x2, y2) in lines {
            let (ax, bx) =
                if horizontal_run { (x + x1 * cw, x + (n as f64 - 1.0 + x2) * cw) } else { (x + x1 * cw, x + x2 * cw) };
            let dash = dashes.map(|k| {
                let len = if y1 == y2 { cw } else { ch } / (2 * k) as f64;
                format!(r#" stroke-dasharray="{len:.2} {len:.2}" stroke-dashoffset="{:.1}""#, -len / 2.0)
            });
            let reps = if horizontal_run { 1 } else { n };
            for i in 0..reps {
                let off = i as f64 * cw;
                let _ = write!(
                    out,
                    r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{fill}" stroke-width="{:.1}"{}/>"#,
                    ax + off,
                    y + y1 * ch,
                    bx + off,
                    y + y2 * ch,
                    sw * weight,
                    dash.clone().unwrap_or_default()
                );
            }
        }
        return;
    }
    for i in 0..n {
        let cx = x + i as f64 * cw;
        if let Some([(sx, sy), (qx, qy), (ex, ey)]) = arc(c) {
            let _ = write!(
                out,
                r#"<path d="M{:.1} {:.1}Q{:.1} {:.1} {:.1} {:.1}" fill="none" stroke="{fill}" stroke-width="{sw:.2}"/>"#,
                cx + sx * cw,
                y + sy * ch,
                cx + qx * cw,
                y + qy * ch,
                cx + ex * cw,
                y + ey * ch
            );
        } else if ('\u{2800}'..='\u{28ff}').contains(&c) {
            let bits = c as u32 - 0x2800;
            const DOTS: [(u32, f64, f64); 8] = [
                (0x01, 0., 0.),
                (0x02, 0., 1.),
                (0x04, 0., 2.),
                (0x40, 0., 3.),
                (0x08, 1., 0.),
                (0x10, 1., 1.),
                (0x20, 1., 2.),
                (0x80, 1., 3.),
            ];
            let (dw, dh) = (cw * 0.26, ch * 0.17);
            let mut d = String::new();
            for (bit, col, row) in DOTS {
                if bits & bit != 0 {
                    let _ = write!(
                        d,
                        "M{:.1} {:.1}h{dw:.1}v{dh:.1}h-{dw:.1}z",
                        cx + cw * (0.18 + 0.4 * col),
                        y + ch * (0.06 + 0.235 * row)
                    );
                }
            }
            let _ = write!(out, r#"<path d="{d}" fill="{fill}"/>"#);
        } else {
            let poly = |pts: &[(f64, f64)]| -> String {
                pts.iter()
                    .map(|(px, py)| format!("{:.1},{:.1}", cx + px * cw, y + py * ch))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            match c {
                '▰' => {
                    let _ = write!(
                        out,
                        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="1.4" fill="{fill}"/>"#,
                        cx + cw * 0.1,
                        y + ch * 0.24,
                        cw * 0.8,
                        ch * 0.52
                    );
                }
                '▱' => {
                    let _ = write!(
                        out,
                        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="1.4" fill="none" stroke="{fill}" stroke-width="1"/>"#,
                        cx + cw * 0.14,
                        y + ch * 0.27,
                        cw * 0.72,
                        ch * 0.46
                    );
                }
                '▮' => {
                    let _ = write!(
                        out,
                        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="1" fill="{fill}"/>"#,
                        cx + cw * 0.28,
                        y + ch * 0.14,
                        cw * 0.44,
                        ch * 0.72
                    );
                }
                '◢' => {
                    let _ = write!(
                        out,
                        r#"<polygon points="{}" fill="{fill}"/>"#,
                        poly(&[(0.9, 0.2), (0.9, 0.8), (0.1, 0.8)])
                    );
                }
                '◣' => {
                    let _ = write!(
                        out,
                        r#"<polygon points="{}" fill="{fill}"/>"#,
                        poly(&[(0.1, 0.2), (0.9, 0.8), (0.1, 0.8)])
                    );
                }
                _ => {}
            }
        }
    }
}

fn cell_char(cell: &Cell) -> char {
    cell.symbol().chars().next().unwrap_or(' ')
}

/// Draw one buffer row: background runs, vector glyphs, then text runs.
/// With `cover`, first paint the whole row in the canvas colour (for overlays).
fn row_svg(out: &mut String, buf: &Buffer, row: u16, g: &Geo, o: &SvgOptions, cover: bool) {
    row_span_svg(out, buf, row, (0, buf.area.width), g, o, cover);
}

/// Like [`row_svg`] for the columns `lo..hi` only.
fn row_span_svg(out: &mut String, buf: &Buffer, row: u16, (lo, w): (u16, u16), g: &Geo, o: &SvgOptions, cover: bool) {
    let y = g.y(row);
    let canvas = hex(o.bg, o.bg);
    if cover {
        let _ = write!(
            out,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{canvas}"/>"#,
            g.x(lo),
            y,
            (w - lo) as f64 * g.cw,
            g.ch + 0.04
        );
    }
    // Backgrounds.
    let mut col = lo;
    while col < w {
        let bg = hex(buf[(col, row)].bg, o.bg);
        let start = col;
        while col < w && hex(buf[(col, row)].bg, o.bg) == bg {
            col += 1;
        }
        if bg != canvas {
            let _ = write!(
                out,
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{bg}"/>"#,
                g.x(start),
                y,
                (col - start) as f64 * g.cw + 0.04,
                g.ch + 0.04
            );
        }
    }
    // Vector glyphs, merged into runs where possible.
    let mut col = lo;
    while col < w {
        let cell = &buf[(col, row)];
        let c = cell_char(cell);
        if !is_vector(c) {
            col += 1;
            continue;
        }
        let fg = hex(cell.fg, o.fg);
        let start = col;
        col += 1;
        if mergeable(c) {
            while col < w && cell_char(&buf[(col, row)]) == c && hex(buf[(col, row)].fg, o.fg) == fg {
                col += 1;
            }
        }
        vector(out, c, col - start, g.x(start), y, g, &fg);
    }
    // Text runs: same colour and weight, no gap of two or more spaces, each
    // stretched to its exact cell width. Single spaces become no-break spaces:
    // some renderers collapse ordinary ones even under xml:space="preserve".
    let mut col = lo;
    while col < w {
        let cell = &buf[(col, row)];
        let c = cell_char(cell);
        if c == ' ' || is_vector(c) {
            col += 1;
            continue;
        }
        let fg = hex(cell.fg, o.fg);
        let bold = cell.modifier.contains(Modifier::BOLD);
        let dim = cell.modifier.contains(Modifier::DIM);
        let start = col;
        let mut text = String::new();
        while col < w {
            let cc = &buf[(col, row)];
            let ch = cell_char(cc);
            if is_vector(ch) {
                break;
            }
            if ch == ' ' {
                let next_is_text = col + 1 < w && {
                    let n = &buf[(col + 1, row)];
                    let nc = cell_char(n);
                    nc != ' ' && !is_vector(nc) && hex(n.fg, o.fg) == fg && n.modifier.contains(Modifier::BOLD) == bold
                };
                if !next_is_text {
                    break;
                }
                text.push('\u{a0}');
            } else {
                if hex(cc.fg, o.fg) != fg || cc.modifier.contains(Modifier::BOLD) != bold {
                    break;
                }
                text.push_str(cc.symbol());
            }
            col += 1;
        }
        let used = col - start;
        let _ = write!(
            out,
            r#"<text x="{:.1}" y="{:.1}" fill="{fg}" textLength="{:.1}" lengthAdjust="spacingAndGlyphs"{}{}>{}</text>"#,
            g.x(start),
            y + g.ch * 0.76,
            used as f64 * g.cw,
            if bold { r#" font-weight="700""# } else { "" },
            if dim { r#" opacity="0.6""# } else { "" },
            esc(&text)
        );
    }
}

fn header(out: &mut String, g: &Geo, o: &SvgOptions, cols: u16, rows: u16) {
    let canvas = hex(o.bg, o.bg);
    let a1 = hex(o.accent, o.accent);
    let a2 = hex(o.accent2, o.accent2);
    let id = &o.id;
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}" font-family="{FONT}" font-size="{fs}" xml:space="preserve">"#,
        w = g.width,
        h = g.height,
        fs = o.font_size
    );
    let _ = write!(
        out,
        r##"<defs><linearGradient id="{id}edge" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{a1}"/><stop offset="1" stop-color="{a2}"/></linearGradient><pattern id="{id}scan" width="4" height="3" patternUnits="userSpaceOnUse"><rect width="4" height="1" fill="#000" fill-opacity="0.22"/></pattern></defs>"##
    );
    let tw = cols as f64 * g.cw;
    let th = rows as f64 * g.ch;
    match &o.chrome {
        Some(title) => {
            let _ = write!(
                out,
                r#"<rect x="1" y="1" width="{:.0}" height="{:.0}" rx="12" fill="{canvas}" stroke="url(#{id}edge)" stroke-width="1.5"/>"#,
                g.width - 2.0,
                g.height - 2.0
            );
            for (i, c) in [&a1, "#f4ff5a", &a2].iter().enumerate() {
                let _ = write!(
                    out,
                    r#"<circle cx="{:.0}" cy="{:.0}" r="5.5" fill="{c}"/>"#,
                    PAD + 8.0 + i as f64 * 18.0,
                    PAD + 8.0
                );
            }
            let _ = write!(
                out,
                r##"<text x="{:.1}" y="{:.1}" fill="#7c819a" text-anchor="middle" font-size="12.5">{}</text>"##,
                g.width / 2.0,
                PAD + 12.5,
                esc(title)
            );
            let _ = write!(
                out,
                r##"<line x1="{PAD}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#1a1d2b"/>"##,
                PAD + BAR - 8.0,
                g.width - PAD,
                PAD + BAR - 8.0
            );
        }
        None => {
            let _ = write!(out, r#"<rect width="{:.1}" height="{:.1}" fill="{canvas}"/>"#, tw, th);
        }
    }
}

fn footer(out: &mut String, g: &Geo, o: &SvgOptions, cols: u16, rows: u16) {
    let id = &o.id;
    if o.scanlines {
        let _ = write!(
            out,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="url(#{id}scan)" pointer-events="none"/>"#,
            g.x0,
            g.y0,
            cols as f64 * g.cw,
            rows as f64 * g.ch
        );
    }
    out.push_str("</svg>\n");
}

/// Column spans of `row` that differ between two frames, merged across small
/// gaps (a whole-row span when most of it changed).
fn changed_spans(prev: &Buffer, cur: &Buffer, row: u16) -> Vec<(u16, u16)> {
    const GAP: u16 = 6;
    let w = cur.area.width;
    let mut spans: Vec<(u16, u16)> = Vec::new();
    for c in 0..w {
        if cur[(c, row)] != prev[(c, row)] {
            match spans.last_mut() {
                Some(last) if c <= last.1 + GAP => last.1 = c + 1,
                _ => spans.push((c, c + 1)),
            }
        }
    }
    let covered: u16 = spans.iter().map(|s| s.1 - s.0).sum();
    if covered * 10 > w * 6 { vec![(0, w)] } else { spans }
}

/// A static SVG of one frame.
pub fn render(buf: &Buffer, o: &SvgOptions) -> String {
    let (w, h) = (buf.area.width, buf.area.height);
    let g = Geo::new(w, h, o);
    let mut out = String::with_capacity(w as usize * h as usize * 40);
    header(&mut out, &g, o, w, h);
    for row in 0..h {
        row_svg(&mut out, buf, row, &g, o, false);
    }
    footer(&mut out, &g, o, w, h);
    out
}

/// An animated SVG: the first frame in full, then for each later frame only the
/// rows that changed, revealed at its time and hidden again when the loop restarts.
pub fn render_animated(frames: &[Buffer], frame_ms: u64, o: &SvgOptions) -> String {
    let Some(first) = frames.first() else {
        return String::new();
    };
    let (w, h) = (first.area.width, first.area.height);
    let g = Geo::new(w, h, o);
    let n = frames.len();
    let dur = n as f64 * frame_ms as f64 / 1000.0;
    let mut out = String::new();
    header(&mut out, &g, o, w, h);
    for row in 0..h {
        row_svg(&mut out, first, row, &g, o, false);
    }
    for k in 1..n {
        let (prev, cur) = (&frames[k - 1], &frames[k]);
        let changed: Vec<u16> = (0..h).filter(|&r| (0..w).any(|c| cur[(c, r)] != prev[(c, r)])).collect();
        if changed.is_empty() {
            continue;
        }
        let _ = write!(
            out,
            r#"<g visibility="hidden"><animate attributeName="visibility" values="hidden;visible" keyTimes="0;{:.4}" dur="{dur:.2}s" calcMode="discrete" repeatCount="indefinite"/>"#,
            k as f64 / n as f64
        );
        for r in changed {
            for span in changed_spans(prev, cur, r) {
                row_span_svg(&mut out, cur, r, span, &g, o, true);
            }
        }
        out.push_str("</g>");
    }
    footer(&mut out, &g, o, w, h);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    fn buf(lines: &[&str]) -> Buffer {
        let w = lines[0].chars().count() as u16;
        let mut b = Buffer::empty(Rect::new(0, 0, w, lines.len() as u16));
        for (y, l) in lines.iter().enumerate() {
            b.set_string(0, y as u16, l, Style::new().fg(Color::Rgb(255, 43, 214)));
        }
        b
    }

    #[test]
    fn text_blocks_and_lines_become_the_right_elements() {
        let svg = render(&buf(&["ab █▄ ─┈ ⣿ ▰▱ x&y"]), &SvgOptions::default());
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains(">ab</text>"));
        let words = render(&buf(&["tool use    1.0s"]), &SvgOptions::default());
        assert!(words.contains(">tool\u{a0}use</text>"), "single spaces are kept as no-break spaces");
        assert!(words.contains(">1.0s</text>"), "a wide gap splits the run");
        assert!(svg.contains(">x&amp;y</text>"), "text is escaped");
        assert!(svg.contains(r#"stroke-dasharray"#), "dashed line for ┈");
        // Minus canvas, scanline pattern and scanline overlay.
        assert_eq!(svg.matches("<rect").count() - 3, 1 + 1 + 2, "full + half block, 2 gauge cells");
        assert_eq!(svg.matches("h2.2v2.9h-2.2z").count(), 8, "8 braille dots in one path");
        assert!(svg.contains("#ff2bd6"));
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn merges_runs_of_full_blocks() {
        let svg = render(&buf(&["████"]), &SvgOptions { scanlines: false, ..Default::default() });
        // Canvas + scanline pattern + one merged rect.
        assert_eq!(svg.matches("<rect").count(), 3);
    }

    #[test]
    fn animation_only_emits_changed_rows() {
        let a = buf(&["hello", "world"]);
        let b = buf(&["hello", "there"]);
        let svg = render_animated(&[a.clone(), b, a], 500, &SvgOptions::default());
        assert_eq!(svg.matches("<animate").count(), 2);
        assert_eq!(svg.matches(">there</text>").count(), 1);
        assert_eq!(svg.matches(">hello</text>").count(), 1, "unchanged row drawn once");
        assert!(svg.contains(r#"dur="1.50s""#));
    }

    #[test]
    fn changed_spans_merge_small_gaps() {
        let a = buf(&["0123456789012345678901234567890123456789"]);
        let b = buf(&["0X2345678901234567890X23456789012345678Y"]);
        assert_eq!(changed_spans(&a, &b, 0), vec![(1, 2), (21, 22), (39, 40)]);
        let c = buf(&["0XX345678901234567890123456789012345678X"]);
        assert_eq!(changed_spans(&a, &c, 0), vec![(1, 3), (39, 40)]);
    }

    #[test]
    fn chrome_adds_a_window_frame() {
        let svg = render(&buf(&["x"]), &SvgOptions { chrome: Some("clankmon".into()), ..Default::default() });
        assert!(svg.contains(">clankmon</text>"));
        assert_eq!(svg.matches("<circle").count(), 3);
    }

    #[test]
    fn palette() {
        assert_eq!(hex(Color::Indexed(196), Color::Reset), "#ff0000");
        assert_eq!(hex(Color::Reset, Color::Rgb(7, 8, 13)), "#07080d");
        assert_eq!(hex(Color::Indexed(244), Color::Reset), "#808080");
    }
}
