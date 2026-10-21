//! Custom widgets drawn straight into the buffer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use super::theme::{Theme, lerp, shade};

const EIGHTHS_V: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const EIGHTHS_H: [&str; 9] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// A bordered panel with a `◢ TITLE ◣` header and an optional right-aligned note.
/// Returns the inner area.
pub fn panel(buf: &mut Buffer, area: Rect, t: &Theme, title: &str, note: Option<Line<'_>>, focused: bool) -> Rect {
    if area.width < 4 || area.height < 2 {
        return Rect::default();
    }
    let border = if focused { t.border_hi } else { t.border };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .style(Style::new().bg(t.bg))
        .title(Line::from(vec![
            Span::styled("◢", Style::new().fg(t.primary)),
            Span::styled(format!(" {title} "), Style::new().fg(t.bright).add_modifier(Modifier::BOLD)),
            Span::styled("◣", Style::new().fg(t.primary)),
        ]));
    if let Some(n) = note {
        block = block.title(n.right_aligned());
    }
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

/// A segmented neon gauge: `▰▰▰▰▱▱▱` with a gradient fill and an optional pace tick.
pub fn neon_gauge(buf: &mut Buffer, area: Rect, pct: f64, pace: Option<f64>, t: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let w = area.width as usize;
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * w as f64).round() as usize;
    let pace_idx = pace.map(|p| ((p.clamp(0.0, 100.0) / 100.0) * w as f64).floor() as usize).map(|i| i.min(w - 1));
    for i in 0..w {
        let x = area.x + i as u16;
        let Some(cell) = buf.cell_mut((x, area.y)) else { continue };
        let pos = i as f64 / (w.max(2) - 1) as f64;
        if i < filled {
            let c = if pct >= 100.0 { t.danger } else { t.gauge_at(pos) };
            cell.set_symbol("▰").set_fg(c).set_bg(t.bg);
        } else if Some(i) == pace_idx {
            cell.set_symbol("┃").set_fg(t.muted).set_bg(t.bg);
        } else {
            cell.set_symbol("▱").set_fg(t.grid).set_bg(t.bg);
        }
        if Some(i) == pace_idx && i < filled {
            cell.set_fg(t.bright).set_symbol("▮");
        }
    }
}

/// Inline bar for table cells: `▰▰▰▱▱`.
pub fn mini_bar(pct: f64, width: usize, t: &Theme) -> Vec<Span<'static>> {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let c = t.level(pct);
    vec![
        Span::styled("▰".repeat(filled), Style::new().fg(c)),
        Span::styled("▱".repeat(width.saturating_sub(filled)), Style::new().fg(t.grid)),
    ]
}

/// Horizontal bar with eighth-cell resolution.
pub fn hbar(value: f64, max: f64, width: usize, color: Color) -> Span<'static> {
    if max <= 0.0 || width == 0 {
        return Span::raw(" ".repeat(width));
    }
    let eighths = ((value / max).clamp(0.0, 1.0) * width as f64 * 8.0).round() as usize;
    let full = eighths / 8;
    let rem = eighths % 8;
    let mut s = "█".repeat(full);
    if rem > 0 && full < width {
        s.push_str(EIGHTHS_H[rem]);
    }
    let used = full + usize::from(rem > 0 && full < width);
    s.push_str(&" ".repeat(width.saturating_sub(used)));
    Span::styled(s, Style::new().fg(color))
}

/// Braille area chart. Each cell holds 2 samples horizontally and 4 dots vertically.
/// `values` are oldest first; the newest sample lands at the right edge.
pub fn braille_area(buf: &mut Buffer, area: Rect, values: &[f64], max: f64, color: impl Fn(f64) -> Color, bg: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols = area.width as usize * 2;
    let rows = area.height as usize * 4;
    let max = if max > 0.0 { max } else { 1.0 };
    // Right-align: pad the left with "no data".
    let start = values.len().saturating_sub(cols);
    let vals = &values[start..];
    let offset = cols - vals.len();
    for cy in 0..area.height as usize {
        for cx in 0..area.width as usize {
            let mut bits: u32 = 0;
            let mut top_val: f64 = 0.0;
            for sub in 0..2 {
                let dot_x = cx * 2 + sub;
                if dot_x < offset {
                    continue;
                }
                let v = vals[dot_x - offset];
                top_val = top_val.max(v);
                let h = ((v / max).clamp(0.0, 1.0) * rows as f64).round() as usize;
                // Dots in this cell, bottom-up.
                let cell_bottom = (area.height as usize - 1 - cy) * 4;
                for d in 0..4 {
                    if cell_bottom + d < h {
                        let row_in_cell = 3 - d;
                        bits |= dot_bit(sub, row_in_cell);
                    }
                }
            }
            let x = area.x + cx as u16;
            let y = area.y + cy as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                if bits == 0 {
                    cell.set_symbol(" ").set_bg(bg);
                } else {
                    let ch = char::from_u32(0x2800 + bits).unwrap_or(' ');
                    let mut tmp = [0u8; 4];
                    cell.set_symbol(ch.encode_utf8(&mut tmp)).set_fg(color(top_val / max)).set_bg(bg);
                }
            }
        }
    }
}

fn dot_bit(col: usize, row: usize) -> u32 {
    match (col, row) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        _ => 0x80,
    }
}

/// A bmon-style stacked bar graph.
pub struct BarGraph<'a> {
    /// One entry per column, oldest first; each holds layer values bottom-up.
    pub columns: &'a [Vec<f64>],
    pub colors: &'a [Color],
    /// Value at the top of the area.
    pub max: f64,
    pub log: bool,
    /// Tint the last column: it is the live one, still filling up.
    pub live_last: bool,
    /// Per column: draw a time tick on the floor.
    pub ticks: &'a [bool],
}

/// Draw a [`BarGraph`]. Columns are right-aligned; the newest is at the right edge.
///
/// Eighth-block resolution everywhere: where two layers meet inside one cell,
/// the lower layer is the glyph's foreground and the upper layer its
/// background, so thin layers stay visible on top of thick ones.
pub fn bar_graph(buf: &mut Buffer, area: Rect, g: &BarGraph<'_>, t: &Theme) {
    if area.width == 0 || area.height == 0 || g.colors.is_empty() {
        return;
    }
    let h = area.height as usize;
    let h8 = h * 8;
    let scale = |v: f64| -> f64 { if g.log { (v.max(0.0) + 1.0).ln() } else { v.max(0.0) } };
    let smax = scale(g.max).max(1e-9);
    let w = area.width as usize;
    let start = g.columns.len().saturating_sub(w);
    let cols = &g.columns[start..];
    let ticks = &g.ticks[g.ticks.len().saturating_sub(cols.len())..];
    let offset = w - cols.len();
    // The half-way gridline, placed exactly at 50 %: mid-cell when the height is
    // odd, on a cell's top edge when it is even.
    let (mid_row, mid_sym) = if h % 2 == 1 { (h / 2, "┈") } else { (h / 2 - 1, "▔") };
    let mid = (h >= 3).then_some(mid_row);
    let live_bg = lerp(t.bg, t.secondary, 0.10);

    for x in 0..w {
        let ci = x.checked_sub(offset);
        let is_live = g.live_last && ci == Some(cols.len().wrapping_sub(1));
        let bg = if is_live { live_bg } else { t.bg };
        let tick = ci.is_some_and(|i| ticks.get(i).copied().unwrap_or(false));
        let gx = area.x + x as u16;
        // Background: faint horizontal grid at the top and middle, and a
        // vertical dotted line at each time tick. Nothing that looks like data.
        for row in 0..h {
            let y = area.y + (h - 1 - row) as u16;
            let Some(cell) = buf.cell_mut((gx, y)) else { continue };
            let sym = if tick {
                "┊"
            } else if Some(row) == mid {
                mid_sym
            } else {
                " "
            };
            cell.set_symbol(sym).set_fg(t.grid).set_bg(bg);
        }
        let Some(ci) = ci else { continue };
        let layers = &cols[ci];
        let total: f64 = layers.iter().map(|v| v.max(0.0)).sum();
        if total <= 0.0 {
            continue;
        }
        let stack = (((scale(total) / smax).clamp(0.0, 1.0) * h8 as f64).round() as usize).max(1);
        let mut bounds = Vec::with_capacity(layers.len());
        let mut acc = 0.0;
        for v in layers {
            acc += v.max(0.0);
            bounds.push(((acc / total) * stack as f64).round() as usize);
        }
        let overflow = scale(total) > smax * 1.0001;
        let color = |i: usize| g.colors[i.min(g.colors.len() - 1)];
        let layer_at = |e: usize| bounds.iter().position(|&b| b > e).unwrap_or(layers.len() - 1);
        for row in 0..h {
            let lo = row * 8;
            if lo >= stack {
                break;
            }
            let top = stack.min(lo + 8);
            let li = layer_at(lo);
            let lower_end = bounds[li].min(top);
            let (sym, fg, cbg) = if lower_end >= top {
                // One layer fills this cell up to the top of the stack.
                (EIGHTHS_V[top - lo], color(li), bg)
            } else if top == lo + 8 {
                // A full cell split between two layers.
                (EIGHTHS_V[lower_end - lo], color(li), color(layer_at(lower_end)))
            } else {
                // The stack ends inside this cell: use the bigger layer's colour.
                let upper = top - lower_end;
                let c = if lower_end - lo >= upper { color(li) } else { color(layer_at(lower_end)) };
                (EIGHTHS_V[top - lo], c, bg)
            };
            let y = area.y + (h - 1 - row) as u16;
            if let Some(cell) = buf.cell_mut((gx, y)) {
                cell.set_symbol(sym).set_fg(fg).set_bg(cbg);
            }
        }
        // Clipped by the axis ceiling: cap the column so it reads as "more".
        if overflow && let Some(cell) = buf.cell_mut((gx, area.y)) {
            cell.set_symbol("▲").set_fg(t.danger).set_bg(bg);
        }
    }
}

/// Write a styled line, clipped to the area width.
pub fn line(buf: &mut Buffer, area: Rect, y_off: u16, l: &Line<'_>) {
    if y_off >= area.height {
        return;
    }
    buf.set_line(area.x, area.y + y_off, l, area.width);
}

/// Apply a left-to-right gradient to every non-blank cell in `area`.
pub fn gradient_fg(buf: &mut Buffer, area: Rect, from: Color, to: Color, scanlines: bool) {
    let w = area.width.max(2) as f64 - 1.0;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y))
                && cell.symbol() != " "
            {
                let mut c = lerp(from, to, (x - area.x) as f64 / w);
                if scanlines && (y - area.y) % 2 == 1 {
                    c = shade(c, 0.78);
                }
                cell.set_fg(c);
            }
        }
    }
}

/// Fill an area with the background colour.
pub fn clear(buf: &mut Buffer, area: Rect, bg: Color) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_bg(bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buf: &Buffer, y: u16, w: u16) -> String {
        (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    #[test]
    fn gauge_fill_and_pace() {
        let t = Theme::neon();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        neon_gauge(&mut buf, Rect::new(0, 0, 10, 1), 40.0, Some(70.0), &t);
        assert_eq!(row(&buf, 0, 10), "▰▰▰▰▱▱▱┃▱▱");
    }

    #[test]
    fn hbar_eighths() {
        assert_eq!(hbar(5.0, 10.0, 4, Color::Red).content, "██  ");
        assert_eq!(hbar(1.0, 16.0, 2, Color::Red).content, "▏ ");
        assert_eq!(hbar(0.0, 0.0, 3, Color::Red).content, "   ");
    }

    #[test]
    fn braille_full_and_empty() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        braille_area(&mut buf, Rect::new(0, 0, 2, 1), &[1.0, 1.0, 0.0, 0.5], 1.0, |_| Color::White, Color::Black);
        assert_eq!(buf[(0, 0)].symbol(), "⣿");
        // Left sample empty, right sample half height: bottom two dots of the right column.
        assert_eq!(buf[(1, 0)].symbol(), "⢠");
    }

    #[test]
    fn bar_graph_layers_and_live_column() {
        let t = Theme::neon();
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        let cols = vec![vec![1.0, 1.0], vec![0.5, 0.0], vec![0.0, 0.0]];
        let g = BarGraph {
            columns: &cols,
            colors: &[Color::Red, Color::Blue],
            max: 2.0,
            log: false,
            live_last: true,
            ticks: &[true, false, false],
        };
        bar_graph(&mut buf, Rect::new(0, 0, 3, 2), &g, &t);
        assert_eq!(buf[(0, 1)].symbol(), "█");
        assert_eq!(buf[(0, 1)].fg, Color::Red);
        assert_eq!(buf[(0, 0)].symbol(), "█");
        assert_eq!(buf[(0, 0)].fg, Color::Blue);
        assert_eq!(buf[(1, 1)].symbol(), "▄");
        assert_eq!(buf[(1, 0)].symbol(), " ", "no gridline below three rows");
        assert_eq!(buf[(2, 1)].symbol(), " ", "the floor stays empty so tiny bars read as data");
        assert_ne!(buf[(2, 1)].bg, t.bg, "live column is tinted");
    }

    #[test]
    fn bar_graph_caps_clipped_columns() {
        let t = Theme::neon();
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
        let cols = vec![vec![5.0], vec![1.0]];
        let g = BarGraph { columns: &cols, colors: &[Color::Red], max: 2.0, log: false, live_last: false, ticks: &[] };
        bar_graph(&mut buf, Rect::new(0, 0, 2, 2), &g, &t);
        assert_eq!(buf[(0, 0)].symbol(), "▲");
        assert_eq!(buf[(0, 0)].fg, t.danger);
        assert_eq!(buf[(1, 1)].symbol(), "█");
        assert_eq!(buf[(1, 0)].symbol(), " ");
    }

    #[test]
    fn bar_graph_midline_sits_at_half() {
        let t = Theme::neon();
        for (h, row, sym) in [(3u16, 1u16, "┈"), (4, 2, "▔")] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 1, h));
            let g =
                BarGraph { columns: &[], colors: &[Color::Red], max: 1.0, log: false, live_last: false, ticks: &[] };
            bar_graph(&mut buf, Rect::new(0, 0, 1, h), &g, &t);
            // `row` counts from the top here.
            assert_eq!(buf[(0, row)].symbol(), sym, "height {h}");
        }
    }

    #[test]
    fn bar_graph_splits_a_cell_between_layers() {
        let t = Theme::neon();
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        // 3/8 red then 5/8 blue in one full cell.
        let cols = vec![vec![3.0, 5.0]];
        let g = BarGraph {
            columns: &cols,
            colors: &[Color::Red, Color::Blue],
            max: 8.0,
            log: false,
            live_last: false,
            ticks: &[],
        };
        bar_graph(&mut buf, Rect::new(0, 0, 1, 1), &g, &t);
        assert_eq!(buf[(0, 0)].symbol(), "▃");
        assert_eq!(buf[(0, 0)].fg, Color::Red);
        assert_eq!(buf[(0, 0)].bg, Color::Blue);
    }
}
