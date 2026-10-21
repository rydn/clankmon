//! The terminal UI: app state, event loop and shared chrome.

pub mod fmt;
pub mod fx;
pub mod theme;
pub mod views;
pub mod widgets;

use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc::UnboundedReceiver;
use tui_big_text::{BigText, PixelSize};

use crate::model::{AppEvent, CollectorHealth, SessionState};
use crate::state::{FxCue, State, ToastLevel};
use crate::store::{Store, StoreMsg};
use fx::{Fx, FxKey};
use theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Sessions,
    Requests,
    History,
    Limits,
}

impl View {
    pub const ALL: [View; 5] = [View::Overview, View::Sessions, View::Requests, View::History, View::Limits];

    pub fn label(&self) -> &'static str {
        match self {
            View::Overview => "OVERVIEW",
            View::Sessions => "SESSIONS",
            View::Requests => "REQUESTS",
            View::History => "HISTORY",
            View::Limits => "LIMITS",
        }
    }

    fn index(&self) -> usize {
        View::ALL.iter().position(|v| v == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Activity,
    Cost,
    Tokens,
    Cpu,
    Name,
}

impl SortKey {
    fn next(self) -> SortKey {
        match self {
            SortKey::Activity => SortKey::Cost,
            SortKey::Cost => SortKey::Tokens,
            SortKey::Tokens => SortKey::Cpu,
            SortKey::Cpu => SortKey::Name,
            SortKey::Name => SortKey::Activity,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Activity => "activity",
            SortKey::Cost => "cost",
            SortKey::Tokens => "tokens",
            SortKey::Cpu => "cpu",
            SortKey::Name => "name",
        }
    }
}

/// Rects from the last frame, so effects can target widgets.
#[derive(Debug, Clone, Copy, Default)]
pub struct Areas {
    pub screen: Rect,
    pub title: Rect,
    pub body: Rect,
    pub five_gauge: Rect,
    pub seven_gauge: Rect,
    pub wire: Rect,
    pub wire_first_row: Rect,
}

pub struct App {
    pub view: View,
    pub theme: Theme,
    pub truecolor: bool,
    pub fx: Fx,
    pub help: bool,
    pub paused: bool,
    pub log_scale: bool,
    pub bucket_secs: i64,
    pub limit_span_hours: i64,
    pub sort: SortKey,
    pub sel_proc: usize,
    pub sel_session: usize,
    pub sel_req: usize,
    pub filter: String,
    pub filter_edit: bool,
    pub detail: bool,
    /// Session to focus when jumping from the overview.
    pub focus_session: Option<String>,
    pub areas: Areas,
    pub started: Instant,
    pub frames: u64,
}

impl App {
    pub fn new(theme: Theme, fx_enabled: bool) -> Self {
        App {
            view: View::Overview,
            truecolor: theme::truecolor_supported(),
            theme,
            fx: Fx::new(fx_enabled),
            help: false,
            paused: false,
            log_scale: false,
            bucket_secs: 60,
            limit_span_hours: 24,
            sort: SortKey::Activity,
            sel_proc: 0,
            sel_session: 0,
            sel_req: 0,
            filter: String::new(),
            filter_edit: false,
            detail: false,
            focus_session: None,
            areas: Areas::default(),
            started: Instant::now(),
            frames: 0,
        }
    }

    fn set_view(&mut self, v: View) {
        if v != self.view {
            self.view = v;
            self.detail = false;
            let (body, t) = (self.areas.body, self.theme.clone());
            self.fx.view_change(body, &t);
        }
    }

    /// Returns false when the app should start quitting.
    fn on_key(&mut self, k: KeyEvent, st: &State) -> bool {
        if k.kind != KeyEventKind::Press {
            return true;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return false;
        }
        if self.filter_edit {
            match k.code {
                KeyCode::Enter | KeyCode::Esc => self.filter_edit = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.sel_req = 0;
                }
                _ => {}
            }
            return true;
        }
        if self.help {
            self.help = false;
            return true;
        }
        if self.detail {
            if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.detail = false;
            }
            return true;
        }
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Char('?') | KeyCode::F(1) | KeyCode::Char('h') => self.help = true,
            KeyCode::Char('1') => self.set_view(View::Overview),
            KeyCode::Char('2') => self.set_view(View::Sessions),
            KeyCode::Char('3') => self.set_view(View::Requests),
            KeyCode::Char('4') => self.set_view(View::History),
            KeyCode::Char('5') => self.set_view(View::Limits),
            KeyCode::Tab => self.set_view(View::ALL[(self.view.index() + 1) % View::ALL.len()]),
            KeyCode::BackTab => self.set_view(View::ALL[(self.view.index() + View::ALL.len() - 1) % View::ALL.len()]),
            KeyCode::Char('t') => {
                self.theme = self.theme.next();
            }
            KeyCode::Char('p') | KeyCode::Char(' ') => self.paused = !self.paused,
            KeyCode::Char('s') => self.sort = self.sort.next(),
            KeyCode::Char('g') => self.log_scale = !self.log_scale,
            KeyCode::Char('f') => self.fx.enabled = !self.fx.enabled,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                if self.view == View::Limits {
                    self.limit_span_hours = match self.limit_span_hours { 6 => 24, 24 => 168, _ => 168 };
                } else {
                    self.bucket_secs = match self.bucket_secs { 10 => 30, 30 => 60, 60 => 300, _ => 900 };
                }
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                if self.view == View::Limits {
                    self.limit_span_hours = match self.limit_span_hours { 168 => 24, 24 => 6, _ => 6 };
                } else {
                    self.bucket_secs = match self.bucket_secs { 900 => 300, 300 => 60, 60 => 30, _ => 10 };
                }
            }
            KeyCode::Char('/') if self.view == View::Requests => self.filter_edit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::Home => self.move_sel(-1_000_000),
            KeyCode::End => self.move_sel(1_000_000),
            KeyCode::Enter => match self.view {
                View::Overview => {
                    let sessions = views::overview::sorted_sessions(st, self.sort);
                    if let Some(s) = sessions.get(self.sel_proc) {
                        self.focus_session = Some(s.id.clone());
                        self.set_view(View::Sessions);
                    }
                }
                View::Requests => self.detail = true,
                _ => {}
            },
            KeyCode::Char('c') => {}
            _ => {}
        }
        true
    }

    fn move_sel(&mut self, d: i64) {
        let apply = |v: &mut usize| *v = (*v as i64 + d).max(0) as usize;
        match self.view {
            View::Overview => apply(&mut self.sel_proc),
            View::Sessions => apply(&mut self.sel_session),
            View::Requests => apply(&mut self.sel_req),
            _ => {}
        }
    }

    /// Turn reducer cues into effects aimed at last frame's widgets.
    fn play_cues(&mut self, cues: Vec<FxCue>) {
        let t = self.theme.clone();
        for c in cues {
            match c {
                FxCue::Glitch => {
                    let a = if self.areas.wire.area() > 0 { self.areas.wire } else { self.areas.body };
                    self.fx.glitch(a, &t);
                }
                FxCue::PulseFive => self.fx.pulse(FxKey::PulseFive, self.areas.five_gauge),
                FxCue::PulseSeven => self.fx.pulse(FxKey::PulseSeven, self.areas.seven_gauge),
                FxCue::NewRequest => {
                    if self.view == View::Overview && self.areas.wire_first_row.area() > 0 {
                        self.fx.new_row(self.areas.wire_first_row, &t);
                    }
                }
            }
        }
    }
}

/// Run the UI until the user quits.
pub async fn run(mut app: App, mut st: State, mut rx: UnboundedReceiver<AppEvent>, store: Option<Store>, idle_fps: u32) -> Result<()> {
    let mut terminal = ratatui::init();
    let res = event_loop(&mut terminal, &mut app, &mut st, &mut rx, store.as_ref(), idle_fps).await;
    ratatui::restore();
    res
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    st: &mut State,
    rx: &mut UnboundedReceiver<AppEvent>,
    store: Option<&Store>,
    idle_fps: u32,
) -> Result<()> {
    let mut events = EventStream::new();
    let idle = Duration::from_millis(1000 / u64::from(idle_fps.clamp(1, 30)));
    let anim = Duration::from_millis(33);
    let mut paused_buf: Vec<AppEvent> = Vec::new();
    let mut dirty = true;
    let mut last_draw = Instant::now() - idle;
    let size = terminal.size()?;
    app.fx.startup(Rect::new(0, 0, size.width, size.height));

    loop {
        let frame_every = if app.fx.running() || app.fx.quitting() { anim } else { idle };
        let wait = frame_every.saturating_sub(last_draw.elapsed());
        tokio::select! {
            ev = events.next() => {
                match ev {
                    Some(Ok(Event::Key(k))) => {
                        if !app.fx.quitting() && !app.on_key(k, st) {
                            let a = app.areas.screen;
                            app.fx.quit(a);
                        }
                        dirty = true;
                    }
                    Some(Ok(Event::Resize(..))) => dirty = true,
                    Some(Err(_)) | None => break,
                    _ => {}
                }
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                let mut batch = vec![ev];
                while let Ok(e) = rx.try_recv() {
                    batch.push(e);
                    if batch.len() > 4096 { break; }
                }
                if app.paused {
                    paused_buf.extend(batch);
                } else {
                    for e in batch { st.apply(e); }
                    dirty = true;
                }
            }
            _ = tokio::time::sleep(wait) => {}
        }

        if !app.paused && !paused_buf.is_empty() {
            for e in paused_buf.drain(..) {
                st.apply(e);
            }
            dirty = true;
        }

        if let Some(store) = store {
            for r in st.persist_requests.drain(..) {
                store.send(StoreMsg::Request(Box::new(r)));
            }
            for l in st.persist_limits.drain(..) {
                store.send(StoreMsg::Limits(Box::new(l)));
            }
            for i in st.persist_incidents.drain(..) {
                store.send(StoreMsg::Incident(i));
            }
        } else {
            st.persist_requests.clear();
            st.persist_limits.clear();
            st.persist_incidents.clear();
        }

        let cues = std::mem::take(&mut st.fx);
        if !cues.is_empty() {
            app.play_cues(cues);
        }
        if st.expire_toasts() {
            dirty = true;
        }

        // Throttle redraws during scan bursts; always redraw on the idle tick.
        let due = last_draw.elapsed() >= frame_every;
        if (dirty && last_draw.elapsed() >= Duration::from_millis(40)) || due {
            st.ensure_sorted();
            terminal.draw(|f| render(f, app, st))?;
            last_draw = Instant::now();
            dirty = false;
            app.frames += 1;
        }
        if app.fx.quit_done() {
            break;
        }
    }
    Ok(())
}

/// Draw one frame. Public for snapshot tests.
pub fn render(f: &mut Frame, app: &mut App, st: &State) {
    let area = f.area();
    app.areas = Areas { screen: area, ..Areas::default() };
    let buf = f.buffer_mut();
    widgets::clear(buf, area, app.theme.bg);

    let tall = area.height >= 32 && area.width >= 90;
    let header_h = if tall { 4 } else { 1 };
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(header_h), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    app.areas.body = body;

    if tall {
        render_header_tall(buf, header, app, st);
    } else {
        render_header_compact(buf, header, app, st);
    }
    match app.view {
        View::Overview => views::overview::render(buf, body, app, st),
        View::Sessions => views::sessions::render(buf, body, app, st),
        View::Requests => views::requests::render(buf, body, app, st),
        View::History => views::history::render(buf, body, app, st),
        View::Limits => views::limits::render(buf, body, app, st),
    }
    render_footer(buf, footer, app);
    render_toasts(buf, body, app, st);
    if app.help {
        views::help::render(buf, area, app);
    }
    if app.detail && app.view == View::Requests {
        views::requests::render_detail(buf, area, app, st);
    }
    if app.view == View::Overview || tall {
        let title = app.areas.title;
        app.fx.ambient(title);
    }
    app.fx.process(buf, area);
    if !app.truecolor {
        theme::downsample(buf);
    }
}

fn collector_dots(app: &App, st: &State) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = Vec::new();
    for (c, (h, _)) in &st.collectors {
        let (dot, color) = match h {
            CollectorHealth::Ok => ("●", t.ok),
            CollectorHealth::Listening => ("◌", t.secondary),
            CollectorHealth::Error(_) => ("✕", t.danger),
            CollectorHealth::Disabled => ("○", t.dim),
        };
        spans.push(Span::styled(dot, Style::new().fg(color)));
        spans.push(Span::styled(format!("{} ", c.label()), Style::new().fg(t.muted)));
    }
    spans
}

fn render_header_tall(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = app.theme.clone();
    let title_w = 32u16.min(area.width);
    let title = Rect::new(area.x + 1, area.y, title_w, 4.min(area.height));
    app.areas.title = title;
    BigText::builder()
        .pixel_size(PixelSize::Quadrant)
        .style(Style::new().fg(t.primary))
        .lines(vec![Line::from("CLANKMON")])
        .build()
        .render(title, buf);
    widgets::gradient_fg(buf, title, t.primary, t.secondary, true);

    let info = Rect::new(title.right() + 2, area.y, area.width.saturating_sub(title_w + 3), area.height);
    if info.width < 20 {
        return;
    }
    let now = st.now();
    let today = st.today();
    let sessions: Vec<_> = st.sessions.values().filter(|s| s.alive()).collect();
    let busy = sessions.iter().filter(|s| s.state.is_busy()).count();
    let waiting = sessions.iter().filter(|s| s.state == SessionState::Waiting).count();
    let mut timeline = st.since(now - chrono::Duration::minutes(60));
    timeline.retain(|r| r.ts <= now);
    let burn = crate::windows::burn(timeline.iter().copied(), now, 60);
    let tag = |s: &str| Span::styled(format!(" {s} "), Style::new().fg(t.bg).bg(t.primary).add_modifier(Modifier::BOLD));
    let sep = || Span::styled(" ▸ ", Style::new().fg(t.dim));
    let val = |s: String| Span::styled(s, Style::new().fg(t.bright));
    let mut plan = st.plan.clone().unwrap_or_else(|| "local".into()).to_uppercase();
    if let Some(tier) = &st.tier {
        plan = format!("{plan} · {}", tier.replace("default_claude_", "").replace('_', " "));
    }
    let l1 = Line::from(vec![tag("PLAN"), Span::raw(" "), val(plan), sep(), Span::styled(format!("theme {}", t.name), Style::new().fg(t.muted))]);
    let l2 = Line::from(vec![
        tag("LIVE"),
        Span::raw(" "),
        val(format!("{} sessions", sessions.len())),
        sep(),
        Span::styled(format!("{busy} busy"), Style::new().fg(if busy > 0 { t.ok } else { t.muted })),
        sep(),
        Span::styled(format!("{waiting} waiting"), Style::new().fg(if waiting > 0 { t.warn } else { t.muted })),
    ]);
    let l3 = Line::from(vec![
        tag("TODAY"),
        Span::raw(" "),
        Span::styled(fmt::money(today.cost), Style::new().fg(t.tertiary).add_modifier(Modifier::BOLD)),
        sep(),
        val(format!("{} tok", fmt::si(today.usage.total()))),
        sep(),
        val(format!("{} req", today.requests)),
        sep(),
        Span::styled(format!("{}/h", fmt::money(burn.cost_per_hour)), Style::new().fg(t.primary)),
    ]);
    let l4 = if st.scan.finished {
        let mut v = vec![tag("FEED")];
        v.push(Span::raw(" "));
        v.extend(collector_dots(app, st));
        Line::from(v)
    } else {
        let pct = if st.scan.files_total > 0 { st.scan.files_done as f64 / st.scan.files_total as f64 * 100.0 } else { 0.0 };
        let mut v = vec![tag("SCAN"), Span::raw(" ")];
        v.extend(widgets::mini_bar(pct, 16, &t));
        v.push(Span::styled(format!(" {}/{} transcripts", st.scan.files_done, st.scan.files_total), Style::new().fg(t.muted)));
        Line::from(v)
    };
    for (i, l) in [l1, l2, l3, l4].iter().enumerate() {
        widgets::line(buf, info, i as u16, l);
    }
    // Clock in the top-right corner.
    let clock = fmt::clock(now);
    let cx = area.right().saturating_sub(clock.len() as u16 + 1);
    buf.set_string(cx, area.y, &clock, Style::new().fg(t.secondary).add_modifier(Modifier::BOLD));
}

fn render_header_compact(buf: &mut Buffer, area: Rect, app: &mut App, st: &State) {
    let t = &app.theme;
    app.areas.title = Rect::new(area.x, area.y, 12.min(area.width), 1);
    let today = st.today();
    let live = st.sessions.values().filter(|s| s.alive()).count();
    let mut spans = vec![
        Span::styled("◢ CLANKMON ◣", Style::new().fg(t.primary).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} ", st.plan.clone().unwrap_or_else(|| "local".into()).to_uppercase()), Style::new().fg(t.bg).bg(t.secondary)),
        Span::styled(format!(" {live} live ", ), Style::new().fg(t.text)),
        Span::styled(format!("{} today ", fmt::money(today.cost)), Style::new().fg(t.tertiary)),
    ];
    if !st.scan.finished {
        spans.push(Span::styled(format!("scan {}/{} ", st.scan.files_done, st.scan.files_total), Style::new().fg(t.muted)));
    } else {
        spans.extend(collector_dots(app, st));
    }
    let clock = fmt::clock(st.now());
    let room = Rect::new(area.x, area.y, area.width.saturating_sub(clock.len() as u16 + 1), 1);
    widgets::line(buf, room, 0, &Line::from(spans));
    let cx = area.right().saturating_sub(clock.len() as u16);
    buf.set_string(cx, area.y, &clock, Style::new().fg(t.secondary));
}

fn render_footer(buf: &mut Buffer, area: Rect, app: &App) {
    let t = &app.theme;
    let key = |k: &str| Span::styled(k.to_string(), Style::new().fg(t.bg).bg(t.secondary).add_modifier(Modifier::BOLD));
    let lab = |l: &str| Span::styled(format!("{l} "), Style::new().fg(t.secondary));
    let full = area.width >= 130;
    let mut spans = Vec::new();
    for (i, v) in View::ALL.iter().enumerate() {
        let active = *v == app.view;
        spans.push(key(&format!("{}", i + 1)));
        let style = if active {
            Style::new().fg(t.primary).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.muted)
        };
        let label = v.label();
        let shown = if full || active { label.to_string() } else { label[..3].to_string() };
        spans.push(Span::styled(format!("{shown} "), style));
    }
    spans.push(Span::raw(" "));
    let mut width: usize = spans.iter().map(|s| s.width()).sum();
    for (k, l) in [("?", "Help"), ("q", "Quit"), ("s", "Sort"), ("t", "Theme"), ("p", "Pause"), ("+-", "Zoom"), ("g", "Log")] {
        let w = k.len() + l.len() + 1;
        if width + w + 16 > area.width as usize {
            break;
        }
        width += w;
        spans.push(key(k));
        spans.push(lab(l));
    }
    widgets::line(buf, area, 0, &Line::from(spans));
    let mut right = Vec::new();
    if app.paused {
        right.push(Span::styled(" PAUSED ", Style::new().fg(t.bg).bg(t.warn).add_modifier(Modifier::BOLD)));
    }
    right.push(Span::styled(format!(" sort:{} ", app.sort.label()), Style::new().fg(t.dim)));
    let rl = Line::from(right);
    let w = rl.width() as u16;
    if area.width > w + 60 {
        buf.set_line(area.right() - w, area.y, &rl, w);
    }
}

fn render_toasts(buf: &mut Buffer, body: Rect, app: &App, st: &State) {
    let t = &app.theme;
    let w = 48u16.min(body.width.saturating_sub(2));
    if w < 20 {
        return;
    }
    for (i, toast) in st.toasts.iter().rev().enumerate() {
        let y = body.y + 1 + i as u16 * 3;
        if y + 3 > body.bottom() {
            break;
        }
        let r = Rect::new(body.right() - w - 1, y, w, 3);
        let color = match toast.level {
            ToastLevel::Info => t.secondary,
            ToastLevel::Warn => t.warn,
            ToastLevel::Danger => t.danger,
        };
        Clear.render(r, buf);
        Paragraph::new(Line::from(Span::styled(toast.text.clone(), Style::new().fg(t.bright))))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Thick)
                    .border_style(Style::new().fg(color))
                    .style(Style::new().bg(t.panel)),
            )
            .wrap(Wrap { trim: true })
            .render(r, buf);
    }
}

/// A centred popup rect.
pub fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(4));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}
