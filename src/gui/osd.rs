//! Painting the OSD: bars view, gauge view and the minimized strip.

use std::f32::consts::{PI, TAU};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Vec2, epaint::PathShape,
};

use crate::config::Config;
use crate::forecast::Forecast;
use crate::stats::UsageStats;

use super::app::OsdApp;
use super::format;
use super::theme::{BarStyle, Theme};

pub struct OsdOutput {
    /// Desired window size in points.
    pub size: Vec2,
    pub response: Response,
}

/// One utilisation row (Session / Weekly / scoped model / Codex windows).
pub struct Row {
    /// Stable id used by the "Show" settings (`osd_hidden_rows`).
    pub id: &'static str,
    pub label: String,
    pub util: f64,
    pub reset: i64,
    pub codex: bool,
    /// Usage-rate projection for this window, if there's enough data.
    pub forecast: Option<Forecast>,
}

impl Row {
    /// Projected run-out time, only when it lands before the reset.
    pub fn exhausts_at(&self) -> Option<i64> {
        self.forecast
            .filter(|f| f.exhausts())
            .map(|f| f.exhausts_at)
    }

    /// Compact OSD label: `full 1h 20m` (warning) when on pace to run out,
    /// else the reset countdown.
    pub fn short_label(&self, now: i64) -> (String, bool) {
        match self.exhausts_at().filter(|t| *t > now) {
            Some(t) => (format!("full {}", format::reset_short(t, now)), true),
            None => (format::reset_short(self.reset, now), false),
        }
    }
}

pub fn rows(s: &UsageStats) -> Vec<Row> {
    let mut v = Vec::new();
    if s.claude_enabled {
        v.push(Row {
            id: "session",
            label: "Session".into(),
            util: s.session_utilization,
            reset: s.session_reset,
            codex: false,
            forecast: s.forecasts.get("session").copied(),
        });
        v.push(Row {
            id: "weekly",
            label: "Weekly".into(),
            util: s.weekly_utilization,
            reset: s.weekly_reset,
            codex: false,
            forecast: s.forecasts.get("weekly").copied(),
        });
        if !s.scoped_label.is_empty() {
            v.push(Row {
                id: "scoped",
                label: s.scoped_label.clone(),
                util: s.scoped_utilization,
                reset: s.scoped_reset,
                codex: false,
                forecast: s.forecasts.get("scoped").copied(),
            });
        }
    }
    if s.codex_available {
        if s.codex_has_session {
            v.push(Row {
                id: "codex_session",
                label: "Codex 5h".into(),
                util: s.codex_session_utilization,
                reset: s.codex_session_reset,
                codex: true,
                forecast: s.forecasts.get("codex_session").copied(),
            });
        }
        if s.codex_has_weekly {
            v.push(Row {
                id: "codex_weekly",
                label: "Codex 7d".into(),
                util: s.codex_weekly_utilization,
                reset: s.codex_weekly_reset,
                codex: true,
                forecast: s.forecasts.get("codex_weekly").copied(),
            });
        }
    }
    v
}

/// The rows the user chose to show on the OSD.
pub fn visible_rows(s: &UsageStats, cfg: &Config) -> Vec<Row> {
    rows(s)
        .into_iter()
        .filter(|r| cfg.row_visible(r.id))
        .collect()
}

/// Short status line shown dimmed at the bottom (error / stale data), if any.
pub fn status_text(s: &UsageStats) -> Option<(String, bool)> {
    if !s.claude_enabled {
        return None;
    }
    if s.credentials_expired {
        return Some(("Token expired — run `claude` to log in".into(), true));
    }
    if s.rate_limited {
        return Some(("Rate limited · showing last known".into(), false));
    }
    if !s.rate_limit_error.is_empty() {
        return Some((s.rate_limit_error.clone(), true));
    }
    None
}

struct Painter<'a> {
    p: &'a egui::Painter,
    cfg: &'a Config,
    t: &'a Theme,
    s: f32,
    opacity: f32,
}

impl Painter<'_> {
    fn font(&self, size: f32) -> FontId {
        let fam = if self.t.monospace {
            FontFamily::Monospace
        } else {
            FontFamily::Proportional
        };
        FontId::new(size * self.s, fam)
    }

    fn text(&self, pos: Pos2, anchor: Align2, text: &str, size: f32, color: Color32) -> Rect {
        self.p.text(pos, anchor, text, self.font(size), color)
    }

    fn bg(&self, c: Color32) -> Color32 {
        c.gamma_multiply(self.opacity)
    }

    fn bar(&self, rect: Rect, frac: f64, fill: Color32) {
        let frac = frac.clamp(0.0, 1.0) as f32;
        match self.t.bar_style {
            BarStyle::Ascii => {
                // Size the glyphs so the run of cells spans the bar width.
                let cells = 24usize;
                let filled = (frac * cells as f32).round() as usize;
                let txt = "█".repeat(filled) + &"░".repeat(cells - filled);
                let probe = self
                    .p
                    .layout_no_wrap(txt.clone(), FontId::monospace(10.0), fill);
                let size = 10.0 * rect.width() / probe.size().x.max(1.0);
                self.p.text(
                    rect.left_center(),
                    Align2::LEFT_CENTER,
                    txt,
                    FontId::monospace(size),
                    fill,
                );
            }
            style => {
                let r = if style == BarStyle::Block {
                    0
                } else {
                    (rect.height() / 2.0) as u8
                };
                self.p.rect_filled(rect, r, self.bg(self.t.bar_track));
                if frac > 0.0 {
                    let mut f = rect;
                    f.set_width((rect.width() * frac).max(rect.height().min(rect.width())));
                    self.p.rect_filled(f, r, fill);
                }
            }
        }
    }

    fn ring(&self, center: Pos2, radius: f32, width: f32, frac: f64, fill: Color32) {
        self.p.circle_stroke(
            center,
            radius,
            Stroke::new(width, self.bg(self.t.bar_track)),
        );
        let frac = frac.clamp(0.0, 1.0) as f32;
        if frac <= 0.0 {
            return;
        }
        let n = ((64.0 * frac).ceil() as usize).max(2);
        let start = -PI / 2.0;
        let points: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = start + TAU * frac * (i as f32 / n as f32);
                center + Vec2::new(a.cos(), a.sin()) * radius
            })
            .collect();
        self.p
            .add(PathShape::line(points, Stroke::new(width, fill)));
    }
}

pub fn paint(ui: &mut egui::Ui, app: &OsdApp, stats: Option<&UsageStats>, now: i64) -> OsdOutput {
    let s = app.scale();
    let t = app.theme;
    let full = ui.max_rect();
    let painter = ui.painter().clone();
    let pp = Painter {
        p: &painter,
        cfg: &app.cfg,
        t,
        s,
        opacity: app.cfg.osd_opacity.clamp(0.1, 1.0),
    };

    let (size, draw): (Vec2, Box<dyn Fn(Rect)>) = if app.cfg.osd_minimized {
        minimized(&pp, stats)
    } else if app.cfg.osd_view_mode == "gauge" {
        gauge(&pp, stats, now)
    } else {
        bars(&pp, stats, now)
    };

    let rect = Rect::from_min_size(full.min, size);
    // Panel background + optional border.
    let radius = CornerRadius::same(((t.corner_radius as f32) * s.min(1.5)) as u8);
    painter.rect_filled(rect, radius, pp.bg(t.bg));
    if t.border_width > 0.0 {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(t.border_width * s, t.text_primary),
            StrokeKind::Inside,
        );
    }
    draw(rect);

    let response = ui.interact(rect, ui.scope_id().with("osd"), Sense::click_and_drag());
    OsdOutput { size, response }
}

fn title_row<'a>(pp: &'a Painter<'a>, stats: Option<&'a UsageStats>, rect: Rect, pad: f32) -> f32 {
    let (t, s) = (pp.t, pp.s);
    let y = rect.top() + pad + 8.0 * s;
    let name = match stats {
        Some(st) if !st.claude_enabled && st.codex_available => "CODEX",
        Some(st) if !st.claude_enabled => "USAGE",
        _ => "CLAUDE",
    };
    let title = format!("{}{name}", t.title_prefix);
    let r = pp.text(
        Pos2::new(rect.left() + pad, y),
        Align2::LEFT_CENTER,
        &title,
        13.0,
        t.text_primary,
    );
    let mut x = r.right() + 6.0 * s;
    if let Some(st) = stats {
        if st.claude_enabled && !st.subscription_type.is_empty() {
            let badge = st.subscription_type.to_uppercase();
            let r = pp.text(
                Pos2::new(x, y),
                Align2::LEFT_CENTER,
                &badge,
                9.0,
                t.text_link,
            );
            x = r.right() + 6.0 * s;
        }
        if st.active_subagent_count > 0 {
            pp.text(
                Pos2::new(x, y),
                Align2::LEFT_CENTER,
                &format!("+{} sub", st.active_subagent_count),
                10.0,
                t.live,
            );
        }
        if st.claude_enabled && pp.cfg.row_visible("cost") {
            let cost = format!("${:.2}", st.today_cost);
            pp.text(
                Pos2::new(rect.right() - pad, y),
                Align2::RIGHT_CENTER,
                &cost,
                11.0,
                t.text_secondary,
            );
        }
    }
    let mut y = y + 10.0 * s;
    if let Some(badge) = live_badge(pp, stats) {
        // The dot is painted (the default font has no `●` glyph).
        let dot_r = 3.0 * s;
        pp.p.circle_filled(
            Pos2::new(rect.left() + pad + dot_r, y + 7.0 * s),
            dot_r,
            t.live,
        );
        pp.text(
            Pos2::new(rect.left() + pad + dot_r * 2.0 + 4.0 * s, y + 1.0 * s),
            Align2::LEFT_TOP,
            &badge,
            9.5,
            t.live,
        );
        y += 13.0 * s;
    }
    y
}

/// `● LIVE 5.3k tok/min` while a session is writing (and the row isn't hidden).
fn live_badge(pp: &Painter<'_>, stats: Option<&UsageStats>) -> Option<String> {
    let st = stats?;
    let live = st.live_activity;
    (st.claude_enabled && live.is_live && pp.cfg.row_visible("live")).then(|| {
        format!(
            "LIVE {} tok/min",
            format::tokens(live.tokens_per_minute.round() as u64)
        )
    })
}

fn live_height(pp: &Painter<'_>, stats: Option<&UsageStats>) -> f32 {
    if live_badge(pp, stats).is_some() {
        13.0 * pp.s
    } else {
        0.0
    }
}

fn footer(pp: &Painter<'_>, stats: Option<&UsageStats>, rect: Rect, y: f32, pad: f32) {
    if let Some((msg, is_err)) = stats.and_then(status_text) {
        let c = if is_err { pp.t.error } else { pp.t.text_dim };
        pp.text(
            Pos2::new(rect.left() + pad, y),
            Align2::LEFT_TOP,
            &msg,
            9.0,
            c,
        );
    }
}

fn footer_height(stats: Option<&UsageStats>, s: f32) -> f32 {
    if stats.and_then(status_text).is_some() {
        14.0 * s
    } else {
        0.0
    }
}

fn bars<'a>(
    pp: &'a Painter<'a>,
    stats: Option<&'a UsageStats>,
    now: i64,
) -> (Vec2, Box<dyn Fn(Rect) + 'a>) {
    let s = pp.s;
    let pad = 12.0 * s;
    let row_h = 32.0 * s;
    let n_rows = stats.map(|st| visible_rows(st, pp.cfg).len()).unwrap_or(2);
    let width = 250.0 * s;
    let height = pad
        + 20.0 * s
        + live_height(pp, stats)
        + n_rows as f32 * row_h
        + footer_height(stats, s)
        + pad * 0.6;
    let draw = move |rect: Rect| {
        let t = pp.t;
        let mut y = title_row(pp, stats, rect, pad);
        let Some(st) = stats else {
            pp.text(
                Pos2::new(rect.left() + pad, y + 12.0 * s),
                Align2::LEFT_TOP,
                "Loading…",
                11.0,
                t.text_dim,
            );
            return;
        };
        for row in visible_rows(st, pp.cfg) {
            // An expired Claude token only stales the Claude rows.
            let dim = st.credentials_expired && !row.codex;
            y += 6.0 * s;
            let label_c = if dim { t.text_dim } else { t.text_secondary };
            pp.text(
                Pos2::new(rect.left() + pad, y),
                Align2::LEFT_TOP,
                &t.label(&row.label),
                11.0,
                label_c,
            );
            let pct_r = pp.text(
                Pos2::new(rect.right() - pad, y),
                Align2::RIGHT_TOP,
                &format::pct(row.util),
                11.0,
                if dim { t.text_dim } else { t.text_primary },
            );
            let (reset, warn) = row.short_label(now);
            if !reset.is_empty() {
                pp.text(
                    Pos2::new(pct_r.left() - 8.0 * s, y + 1.0 * s),
                    Align2::RIGHT_TOP,
                    &reset,
                    9.5,
                    if warn && !dim { t.warn } else { t.text_dim },
                );
            }
            y += 16.0 * s;
            let bar = Rect::from_min_size(
                Pos2::new(rect.left() + pad, y),
                Vec2::new(rect.width() - 2.0 * pad, 6.0 * s),
            );
            let fill = if dim {
                t.text_dim
            } else {
                t.bar_color(row.util)
            };
            pp.bar(bar, row.util, fill);
            y += row_h - 22.0 * s;
        }
        footer(pp, stats, rect, y + 2.0 * s, pad);
    };
    (Vec2::new(width, height), Box::new(draw))
}

fn gauge<'a>(
    pp: &'a Painter<'a>,
    stats: Option<&'a UsageStats>,
    now: i64,
) -> (Vec2, Box<dyn Fn(Rect) + 'a>) {
    let s = pp.s;
    let pad = 12.0 * s;
    let cell = 78.0 * s;
    let ring_row_h = cell + 26.0 * s;
    // Claude rings on the first line, Codex rings (if any) on a second.
    let (n_claude, n_codex) = stats
        .map(|st| {
            let rs = visible_rows(st, pp.cfg);
            let c = rs.iter().filter(|r| r.codex).count();
            (rs.len() - c, c)
        })
        .unwrap_or((2, 0));
    let lines = if n_codex > 0 { 2.0 } else { 1.0 };
    let width = (pad * 2.0 + cell * n_claude.max(n_codex) as f32).max(200.0 * s);
    let height = pad
        + 20.0 * s
        + live_height(pp, stats)
        + ring_row_h * lines
        + footer_height(stats, s)
        + pad * 0.5;
    let draw = move |rect: Rect| {
        let t = pp.t;
        let mut y = title_row(pp, stats, rect, pad);
        let Some(st) = stats else { return };
        let rs = visible_rows(st, pp.cfg);
        for group in [false, true] {
            let line: Vec<&Row> = rs.iter().filter(|r| r.codex == group).collect();
            if line.is_empty() {
                continue;
            }
            let x0 = rect.center().x - cell * line.len() as f32 / 2.0;
            for (i, row) in line.iter().enumerate() {
                let c = Pos2::new(x0 + cell * (i as f32 + 0.5), y + 6.0 * s + cell / 2.0);
                let dim = st.credentials_expired && !row.codex;
                let fill = if dim {
                    t.text_dim
                } else {
                    t.bar_color(row.util)
                };
                pp.ring(c, cell * 0.36, 6.0 * s, row.util, fill);
                pp.text(
                    c,
                    Align2::CENTER_CENTER,
                    &format::pct(row.util),
                    13.0,
                    t.text_primary,
                );
                pp.text(
                    Pos2::new(c.x, c.y + cell / 2.0 + 2.0 * s),
                    Align2::CENTER_TOP,
                    &t.label(&row.label),
                    10.0,
                    t.text_secondary,
                );
                let (reset, warn) = row.short_label(now);
                pp.text(
                    Pos2::new(c.x, c.y + cell / 2.0 + 14.0 * s),
                    Align2::CENTER_TOP,
                    &reset,
                    9.0,
                    if warn && !dim { t.warn } else { t.text_dim },
                );
            }
            y += ring_row_h;
        }
        footer(pp, stats, rect, y + 2.0 * s, pad);
    };
    (Vec2::new(width, height), Box::new(draw))
}

fn minimized<'a>(
    pp: &'a Painter<'a>,
    stats: Option<&'a UsageStats>,
) -> (Vec2, Box<dyn Fn(Rect) + 'a>) {
    let s = pp.s;
    let size = Vec2::new(170.0 * s, 20.0 * s);
    let draw = move |rect: Rect| {
        let t = pp.t;
        let Some(st) = stats else { return };
        let inner = rect.shrink(3.0 * s);
        let r = (inner.height() / 2.0) as u8;
        let frac = st.session_utilization.clamp(0.0, 1.0) as f32;
        if frac > 0.0 {
            let mut f = inner;
            f.set_width(inner.width() * frac);
            pp.p.rect_filled(
                f,
                r,
                t.bar_color(st.session_utilization).gamma_multiply(0.75),
            );
        }
        let txt = format!(
            "S {} · W {}",
            format::pct(st.session_utilization),
            format::pct(st.weekly_utilization)
        );
        pp.text(
            rect.center(),
            Align2::CENTER_CENTER,
            &txt,
            10.0,
            t.text_primary,
        );
    };
    (size, Box::new(draw))
}
