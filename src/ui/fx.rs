//! tachyonfx timelines. Effects only run briefly; while none are active the UI
//! drops back to its idle frame rate.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tachyonfx::fx::{self, Glitch, RepeatMode};
use tachyonfx::pattern::SweepPattern;
use tachyonfx::{Effect, EffectManager, Interpolation, IntoEffect, Motion};

use super::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FxKey {
    #[default]
    Startup,
    View,
    Glitch,
    PulseFive,
    PulseSeven,
    Ambient,
    Row,
    Quit,
}

pub struct Fx {
    mgr: EffectManager<FxKey>,
    pub enabled: bool,
    last: Instant,
    next_ambient: Instant,
    quit_started: Option<Instant>,
}

impl Fx {
    pub fn new(enabled: bool) -> Self {
        Fx {
            mgr: EffectManager::default(),
            enabled,
            last: Instant::now(),
            next_ambient: Instant::now() + Duration::from_secs(20),
            quit_started: None,
        }
    }

    pub fn running(&self) -> bool {
        self.enabled && self.mgr.is_running()
    }

    fn add(&mut self, key: FxKey, effect: Effect) {
        if self.enabled {
            self.mgr.add_unique_effect(key, effect);
        }
    }

    pub fn startup(&mut self, area: Rect) {
        self.add(FxKey::Startup, fx::coalesce((900, Interpolation::QuadOut)).with_area(area));
    }

    pub fn view_change(&mut self, area: Rect, t: &Theme) {
        self.add(
            FxKey::View,
            fx::sweep_in(Motion::LeftToRight, 24, 6, t.bg, (420, Interpolation::QuadOut)).with_area(area),
        );
    }

    /// Brighten a gauge a few times when it crosses a threshold.
    pub fn pulse(&mut self, key: FxKey, area: Rect) {
        let one = fx::ping_pong(fx::hsl_shift_fg([0.0, 25.0, 30.0], (260, Interpolation::SineInOut)));
        self.add(key, fx::repeat(one, RepeatMode::Times(3)).with_area(area));
    }

    /// Corrupt a region briefly when a 429 lands.
    pub fn glitch(&mut self, area: Rect, t: &Theme) {
        let g = Glitch::builder()
            .cell_glitch_ratio(0.08)
            .action_start_delay_ms(0..200)
            .action_ms(60..260)
            .build()
            .into_effect();
        let effect = fx::parallel(&[
            fx::with_duration(Duration::from_millis(700).into(), g),
            fx::fade_from_fg(t.danger, (900, Interpolation::ExpoOut)),
        ]);
        self.add(FxKey::Glitch, effect.with_area(area));
    }

    /// Flash the newest wire row.
    pub fn new_row(&mut self, area: Rect, t: &Theme) {
        self.add(FxKey::Row, fx::fade_from_fg(t.primary, (700, Interpolation::QuadOut)).with_area(area));
    }

    /// Periodic shimmer across the title.
    pub fn ambient(&mut self, area: Rect) {
        if !self.enabled || Instant::now() < self.next_ambient {
            return;
        }
        self.next_ambient = Instant::now() + Duration::from_secs(25);
        let shimmer = fx::ping_pong(fx::hsl_shift_fg([0.0, 0.0, 35.0], (700, Interpolation::SineInOut)))
            .with_pattern(SweepPattern::left_to_right(12));
        self.add(FxKey::Ambient, shimmer.with_area(area));
    }

    pub fn quit(&mut self, area: Rect) {
        self.quit_started = Some(Instant::now());
        self.add(FxKey::Quit, fx::dissolve((450, Interpolation::QuadIn)).with_area(area));
    }

    /// True once the quit animation finished (or effects are off).
    pub fn quit_done(&self) -> bool {
        match self.quit_started {
            None => false,
            Some(t) => !self.enabled || t.elapsed() > Duration::from_millis(480),
        }
    }

    pub fn quitting(&self) -> bool {
        self.quit_started.is_some()
    }

    /// Advance and apply effects to the frame.
    pub fn process(&mut self, buf: &mut Buffer, area: Rect) {
        let now = Instant::now();
        let elapsed = now - self.last;
        self.last = now;
        if self.enabled {
            self.mgr.process_effects(elapsed.into(), buf, area);
        }
    }
}
