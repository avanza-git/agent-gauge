//! The details window (left-click on the OSD): limits, cost breakdown,
//! top projects and running sessions.

use eframe::egui::{self, Grid, ProgressBar, RichText, ScrollArea, ViewportBuilder, ViewportId};

use crate::pricing;
use crate::stats::UsageStats;

use super::app::OsdApp;
use super::format;
use super::osd::{rows, status_text};

fn heading(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).strong().color(color));
    ui.separator();
}

/// Draw the details window; returns false once the user closes it.
pub fn show(ctx: &egui::Context, app: &OsdApp, stats: Option<&UsageStats>, now: i64) -> bool {
    let id = ViewportId::from_hash_of("agent-gauge-details");
    let builder = ViewportBuilder::default()
        .with_title("AgentGauge — details")
        .with_app_id("agent-gauge")
        .with_inner_size([460.0, 600.0])
        .with_min_inner_size([360.0, 300.0]);
    let t = app.theme;

    ctx.show_viewport_immediate(id, builder, |ui, _class| {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            return false;
        }
        ui.visuals_mut().override_text_color = Some(t.text_primary);
        let panel = egui::Frame::new().fill(t.bg).inner_margin(14);
        egui::CentralPanel::default().frame(panel).show(ui, |ui| {
            let Some(st) = stats else {
                ui.label("Collecting usage…");
                return;
            };
            ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| body(ui, app, st, now));
        });
        true
    })
}

fn body(ui: &mut egui::Ui, app: &OsdApp, st: &UsageStats, now: i64) {
    let t = app.theme;
    let dim = |s: String| RichText::new(s).small().color(t.text_dim);

    heading(ui, "Plan limits", t.text_secondary);
    if let Some((msg, is_err)) = status_text(st) {
        ui.label(RichText::new(msg).color(if is_err { t.error } else { t.warn }));
    }
    for row in rows(st) {
        ui.horizontal(|ui| {
            ui.label(&row.label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(dim(format::reset_long(row.reset, now)));
            });
        });
        ui.add(
            ProgressBar::new(row.util as f32)
                .fill(t.bar_color(row.util))
                .text(format::pct(row.util)),
        );
        if let Some(f) = row.forecast {
            let pace = format!("+{:.0}%/h", f.rate_per_hour * 100.0);
            match row.exhausts_at().filter(|t| *t > now) {
                Some(at) => ui.label(
                    RichText::new(format!(
                        "{pace} · full in ~{} at this rate",
                        format::until_compact(at, now)
                    ))
                    .small()
                    .color(t.warn),
                ),
                None => ui.label(dim(format!(
                    "{pace} · on pace for ~{} at reset",
                    format::pct(f.at_reset(row.util, now, row.reset))
                ))),
            };
        }
    }
    if st.from_statusline_cache {
        ui.label(dim("Session/weekly from the statusline cache".into()));
    }

    let cost_label = if st.is_subscriber() {
        "API-equivalent value"
    } else {
        "Estimated cost"
    };
    heading(ui, &format!("{cost_label} (USD)"), t.text_secondary);
    Grid::new("cost")
        .num_columns(2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            ui.label("Today");
            ui.label(format!("${:.2}", st.today_cost));
            ui.end_row();
            ui.label("Last 7 days");
            ui.label(format!("${:.2}", st.week_cost));
            ui.end_row();
            ui.label("Saved by caching (7d)");
            ui.label(RichText::new(format!("${:.2}", st.cache_savings)).color(t.live));
            ui.end_row();
            ui.label("Output tokens today / 7d");
            ui.label(format!(
                "{} / {}",
                format::tokens(st.today_tokens),
                format::tokens(st.week_tokens)
            ));
            ui.end_row();
        });

    if !st.today_by_model_detailed.is_empty() {
        heading(ui, "Today by model", t.text_secondary);
        Grid::new("models")
            .striped(true)
            .num_columns(5)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for h in ["Model", "Input", "Output", "Cache r/w", "Cost"] {
                    ui.label(RichText::new(h).small().color(t.text_dim));
                }
                ui.end_row();
                for (model, b) in &st.today_by_model_detailed {
                    let cost = st
                        .today_cost_by_model
                        .get(model)
                        .copied()
                        .unwrap_or_else(|| pricing::calculate_cost(model, b));
                    ui.label(model.trim_start_matches("claude-"));
                    ui.label(format::tokens(b.input));
                    ui.label(format::tokens(b.output));
                    ui.label(format!(
                        "{} / {}",
                        format::tokens(b.cache_read),
                        format::tokens(b.cache_creation)
                    ));
                    ui.label(format!("${:.2}", cost.total));
                    ui.end_row();
                }
            });
    }

    if !st.today_by_project.is_empty() {
        heading(ui, "Top projects today (output tokens)", t.text_secondary);
        Grid::new("projects")
            .num_columns(2)
            .spacing([24.0, 4.0])
            .show(ui, |ui| {
                for (proj, n) in st.today_by_project.iter().take(5) {
                    ui.label(format::project_name(proj));
                    ui.label(format::tokens(*n));
                    ui.end_row();
                }
            });
    }

    heading(
        ui,
        &format!("Active sessions ({})", st.active_sessions.len()),
        t.text_secondary,
    );
    if st.active_sessions.is_empty() {
        ui.label(dim("No running Claude Code sessions".into()));
    }
    for s in &st.active_sessions {
        let name = if s.name.is_empty() { &s.cwd } else { &s.name };
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).color(t.text_link));
            ui.label(dim(format!("pid {} {}", s.pid, s.status)));
        });
    }
    if st.live_activity.is_live {
        ui.label(
            RichText::new(format!(
                "LIVE: {} output tok/min over the last 5 min",
                format::tokens(st.live_activity.tokens_per_minute.round() as u64)
            ))
            .color(t.live),
        );
    }
    if st.active_subagent_count > 0 {
        ui.label(dim(format!(
            "{} subagent(s) writing",
            st.active_subagent_count
        )));
    }

    ui.add_space(10.0);
    ui.label(dim(format!(
        "Updated {} · refresh every {}s",
        format::ago(now - st.collected_at),
        app.cfg.refresh_seconds
    )));
}
