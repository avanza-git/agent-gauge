//! The OSD application: state, background refresh worker, window management.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, Vec2, ViewportCommand, ViewportId, WindowLevel};

use crate::config::Config;
use crate::stats::UsageStats;
use crate::worker::{self, WorkerMsg};

use super::platform;
use super::theme::{self, Theme};

pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 4.0;
const SCREEN_MARGIN: f32 = 24.0;

pub type SharedStats = Arc<Mutex<Option<UsageStats>>>;

/// Things the menu / details windows ask the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    ToggleDetails,
    Refresh,
    SetOpacity(f32),
    SetView(&'static str),
    SetPosition(&'static str),
    SetTheme(&'static str),
    ToggleMinimized,
    ToggleAlwaysOnTop,
    ToggleTaskbar,
    /// Enable/disable a provider ("claude" / "codex").
    ToggleAgent(&'static str),
    /// Show/hide an OSD row by id.
    ToggleRow(&'static str),
    CloseMenu,
    Quit,
}

pub struct OsdApp {
    pub cfg: Config,
    config_path: PathBuf,
    pub stats: SharedStats,
    wake: Sender<WorkerMsg>,
    pub theme: &'static Theme,

    /// Last size we asked the window manager for.
    sent_size: Vec2,
    /// Corner placement still pending (needs the monitor size first).
    needs_placement: bool,
    /// Retry native window hints on a clock, after the window has been mapped.
    startup_hints: StartupHints,
    /// User started a drag; persist the final position once it settles.
    user_dragged: bool,
    last_outer_pos: Option<Pos2>,
    pos_settle: Option<Instant>,

    pub menu_open: bool,
    pub menu_pos: Pos2,
    pub menu_size: Vec2,
    pub menu_was_focused: bool,
    pub details_open: bool,

    /// Set when quitting so the caller can relaunch with a fresh window.
    pub relaunch: Arc<std::sync::atomic::AtomicBool>,
}

impl OsdApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Config,
        config_path: PathBuf,
        relaunch: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let stats: SharedStats = Arc::new(Mutex::new(None));
        let shared = stats.clone();
        let ctx = cc.egui_ctx.clone();
        let wake = worker::spawn(cfg.clone(), move |snapshot, _| {
            if let Ok(mut guard) = shared.lock() {
                *guard = Some(snapshot);
            }
            ctx.request_repaint();
        });
        let needs_placement =
            !(cfg.osd_position == "custom" && cfg.osd_x.is_some() && cfg.osd_y.is_some());
        Self {
            theme: theme::get(&cfg.theme),
            cfg,
            config_path,
            stats,
            wake,
            sent_size: Vec2::ZERO,
            needs_placement,
            startup_hints: StartupHints::new(Instant::now()),
            user_dragged: false,
            last_outer_pos: None,
            pos_settle: None,
            menu_open: false,
            menu_pos: Pos2::ZERO,
            menu_size: Vec2::new(230.0, 300.0),
            menu_was_focused: false,
            details_open: false,
            relaunch,
        }
    }

    pub fn save_config(&self) {
        if let Err(e) = self.cfg.save(&self.config_path) {
            eprintln!("agent-gauge: could not save config: {e}");
        }
    }

    pub fn snapshot(&self) -> Option<UsageStats> {
        self.stats.lock().ok().and_then(|g| g.clone())
    }

    pub fn scale(&self) -> f32 {
        self.cfg.osd_scale.clamp(MIN_SCALE, MAX_SCALE)
    }

    pub fn apply(&mut self, ctx: &egui::Context, frame: &eframe::Frame, action: Action) {
        match action {
            Action::ToggleDetails => self.details_open = !self.details_open,
            Action::Refresh => {
                let _ = self.wake.send(WorkerMsg::Refresh);
            }
            Action::ToggleAgent(agent) => {
                let on = !self.cfg.providers.iter().any(|p| p == agent);
                Config::set_in_list(&mut self.cfg.providers, agent, on);
                self.save_config();
                // The worker re-collects right away with the new agent set.
                let _ = self
                    .wake
                    .send(WorkerMsg::Reconfigure(Box::new(self.cfg.clone())));
            }
            Action::ToggleRow(id) => {
                let hide = self.cfg.row_visible(id);
                Config::set_in_list(&mut self.cfg.osd_hidden_rows, id, hide);
                self.save_config();
            }
            Action::SetOpacity(o) => {
                self.cfg.osd_opacity = o;
                self.save_config();
            }
            Action::SetView(v) => {
                self.cfg.osd_view_mode = v.into();
                self.save_config();
            }
            Action::SetPosition(p) => {
                self.cfg.osd_position = p.into();
                self.cfg.osd_x = None;
                self.cfg.osd_y = None;
                self.needs_placement = true;
                self.save_config();
            }
            Action::SetTheme(t) => {
                self.cfg.theme = t.into();
                self.theme = theme::get(t);
                self.save_config();
            }
            Action::ToggleMinimized => {
                self.cfg.osd_minimized = !self.cfg.osd_minimized;
                self.save_config();
            }
            Action::ToggleAlwaysOnTop => {
                self.cfg.osd_always_on_top = !self.cfg.osd_always_on_top;
                self.apply_window_level(ctx, frame);
                self.save_config();
            }
            Action::ToggleTaskbar => {
                self.cfg.show_in_taskbar = !self.cfg.show_in_taskbar;
                self.save_config();
                if platform::TASKBAR_LIVE_TOGGLE {
                    self.apply_taskbar_hint(frame);
                } else {
                    // The taskbar flag is fixed at window creation here: restart.
                    self.relaunch
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
                }
            }
            Action::CloseMenu => self.menu_open = false,
            Action::Quit => ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close),
        }
        ctx.request_repaint();
    }

    /// Use the same native-window path at startup and when the checkbox changes.
    fn apply_window_level(&self, ctx: &egui::Context, frame: &eframe::Frame) {
        let level = if self.cfg.osd_always_on_top {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        };
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::WindowLevel(level));
        self.apply_on_top_hint(frame);
    }

    fn apply_on_top_hint(&self, frame: &eframe::Frame) {
        if let Some(win) = frame.winit_window()
            && let Err(e) = platform::set_always_on_top(win.as_ref(), self.cfg.osd_always_on_top)
        {
            eprintln!("agent-gauge: could not update always-on-top: {e}");
        }
    }

    fn apply_taskbar_hint(&self, frame: &eframe::Frame) {
        if let Some(win) = frame.winit_window()
            && let Err(e) = platform::set_skip_taskbar(win.as_ref(), !self.cfg.show_in_taskbar)
        {
            eprintln!("agent-gauge: could not update taskbar hint: {e}");
        }
    }

    /// Keep the native window sized to the content.
    fn sync_size(&mut self, ctx: &egui::Context, want: Vec2) {
        let want = Vec2::new(want.x.ceil(), want.y.ceil());
        if (want - self.sent_size).length() > 0.5 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(want));
            self.sent_size = want;
        }
    }

    /// Snap to the configured corner of the primary monitor.
    fn place_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame, size: Vec2) {
        if !self.needs_placement {
            return;
        }
        let corner = |origin: Pos2, mon: Vec2| {
            let m = SCREEN_MARGIN;
            let right = origin.x + (mon.x - size.x - m).max(0.0);
            let bottom = origin.y + (mon.y - size.y - m * 2.0).max(0.0);
            match self.cfg.osd_position.as_str() {
                "top-left" => Pos2::new(origin.x + m, origin.y + m),
                "bottom-left" => Pos2::new(origin.x + m, bottom),
                "bottom-right" => Pos2::new(right, bottom),
                _ => Pos2::new(right, origin.y + m),
            }
        };
        // Prefer the real monitor geometry (multi-monitor aware). winit reports
        // physical pixels; egui's OuterPosition takes points.
        let ppp = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .unwrap_or(1.0)
            .max(0.1);
        let pos = frame
            .winit_window()
            .and_then(|w| w.primary_monitor().or_else(|| w.current_monitor()))
            .map(|mon| {
                let (p, s) = (mon.position(), mon.size());
                corner(
                    Pos2::new(p.x as f32 / ppp, p.y as f32 / ppp),
                    Vec2::new(s.width as f32 / ppp, s.height as f32 / ppp),
                )
            })
            .or_else(|| {
                ctx.input(|i| i.viewport().monitor_size)
                    .map(|mon| corner(Pos2::ZERO, mon))
            });
        let Some(pos) = pos else { return };
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        self.last_outer_pos = Some(pos);
        self.needs_placement = false;
    }

    /// After a user drag, persist the new position once it stops changing.
    fn track_drag(&mut self, ctx: &egui::Context) {
        let Some(pos) = ctx.input(|i| i.viewport().outer_rect).map(|r| r.min) else {
            return;
        };
        if self
            .last_outer_pos
            .is_some_and(|p| (p - pos).length() > 0.5)
            && self.user_dragged
        {
            self.pos_settle = Some(Instant::now());
        }
        self.last_outer_pos = Some(pos);
        if let Some(t) = self.pos_settle {
            if t.elapsed() > Duration::from_millis(700) {
                self.cfg.osd_position = "custom".into();
                self.cfg.osd_x = Some(pos.x);
                self.cfg.osd_y = Some(pos.y);
                self.save_config();
                self.pos_settle = None;
                self.user_dragged = false;
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
    }
}

impl eframe::App for OsdApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4] // fully transparent window; the OSD paints its own panel
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let stats = self.snapshot();
        let now = chrono::Utc::now().timestamp();

        // Initial builder hints can precede mapping. Reissue the same window-level
        // command as the live toggle, including after the startup layout settles.
        let hint_time = Instant::now();
        if self.startup_hints.take_due(hint_time) {
            self.apply_window_level(&ctx, frame);
            if platform::TASKBAR_LIVE_TOGGLE {
                self.apply_taskbar_hint(frame);
            }
        }
        if let Some(delay) = self.startup_hints.remaining(hint_time) {
            ctx.request_repaint_after(delay);
        }

        let out = super::osd::paint(ui, self, stats.as_ref(), now);
        self.sync_size(&ctx, out.size);
        self.place_window(&ctx, frame, out.size);

        if out.response.drag_started() {
            self.user_dragged = true;
            self.menu_open = false;
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if out.response.clicked() {
            if self.menu_open {
                self.menu_open = false; // a click outside the menu dismisses it
            } else {
                self.apply(&ctx, frame, Action::ToggleDetails);
            }
        }
        if out.response.secondary_clicked() {
            let outer = ctx
                .input(|i| i.viewport().outer_rect)
                .map(|r| r.min)
                .unwrap_or_default();
            let local = out.response.interact_pointer_pos().unwrap_or_default();
            self.menu_pos = outer + local.to_vec2();
            if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
                self.menu_pos.x = self.menu_pos.x.min(m.x - self.menu_size.x - 8.0).max(0.0);
                self.menu_pos.y = self.menu_pos.y.min(m.y - self.menu_size.y - 8.0).max(0.0);
            }
            self.menu_open = true;
            self.menu_was_focused = false;
        }
        if out.response.hovered() {
            let dy = ctx.input(|i| i.smooth_scroll_delta.y);
            if dy.abs() > 0.0 {
                let factor = if dy > 0.0 { 1.08 } else { 1.0 / 1.08 };
                self.cfg.osd_scale = (self.scale() * factor).clamp(MIN_SCALE, MAX_SCALE);
                self.save_config();
            }
        }
        self.track_drag(&ctx);

        for action in super::menu::show(&ctx, self, stats.as_ref(), now) {
            self.apply(&ctx, frame, action);
        }
        if self.details_open && !super::details::show(&ctx, self, stats.as_ref(), now) {
            self.details_open = false;
        }

        // Countdowns tick even without new data.
        ctx.request_repaint_after(Duration::from_secs(20));
    }
}

/// Wait for mapping rather than counting frames (several frames can precede mapping).
struct StartupHints {
    next: Option<Instant>,
    attempts: usize,
}

impl StartupHints {
    const DELAYS: [Duration; 5] = [
        Duration::from_millis(250),
        Duration::from_millis(250),
        Duration::from_millis(500),
        Duration::from_secs(1),
        Duration::from_secs(2),
    ];

    fn new(now: Instant) -> Self {
        Self {
            next: Some(now + Self::DELAYS[0]),
            attempts: 0,
        }
    }

    fn take_due(&mut self, now: Instant) -> bool {
        if self.next.is_none_or(|at| now < at) {
            return false;
        }
        self.attempts += 1;
        self.next = Self::DELAYS.get(self.attempts).map(|delay| now + *delay);
        true
    }

    fn remaining(&self, now: Instant) -> Option<Duration> {
        self.next.map(|at| at.saturating_duration_since(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_hints_wait_for_mapping_and_finish() {
        let mut now = Instant::now();
        let mut hints = StartupHints::new(now);
        // Rendering several initial frames must not consume the delayed retries.
        for _ in 0..10 {
            assert!(!hints.take_due(now));
        }
        for delay in StartupHints::DELAYS {
            now += delay;
            assert!(hints.take_due(now));
            assert!(!hints.take_due(now));
        }
        assert_eq!(hints.remaining(now), None);
        assert!(!hints.take_due(now + Duration::from_secs(60)));
    }

    #[test]
    fn delayed_frame_still_applies_hints_without_deadline_underflow() {
        let now = Instant::now();
        let mut hints = StartupHints::new(now);
        let later = now + Duration::from_secs(10);
        assert_eq!(hints.remaining(later), Some(Duration::ZERO));
        assert!(hints.take_due(later));
        assert_eq!(hints.remaining(later), Some(Duration::from_millis(250)));
    }
}
