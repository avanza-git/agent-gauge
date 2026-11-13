//! Right-click menu, shown as its own small borderless window so it is never
//! clipped by the (tiny) OSD window.

use eframe::egui::{
    self, Button, CollapsingHeader, Color32, Key, RichText, Stroke, ViewportBuilder, ViewportId,
    X11WindowType,
};

use crate::stats::UsageStats;

use super::app::{Action, OsdApp};
use super::format;
use super::osd::visible_rows;
use super::platform;
use super::theme::THEMES;

const MENU_WIDTH: f32 = 230.0;

fn item(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(
        Button::new(text)
            .frame(false)
            .min_size(egui::vec2(MENU_WIDTH - 20.0, 0.0)),
    )
    .clicked()
}

fn dim(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    ui.label(RichText::new(text.into()).small().color(color));
}

pub fn show(
    ctx: &egui::Context,
    app: &mut OsdApp,
    stats: Option<&UsageStats>,
    now: i64,
) -> Vec<Action> {
    let mut actions = Vec::new();
    if !app.menu_open {
        return actions;
    }
    let id = ViewportId::from_hash_of("agent-gauge-menu");
    let builder = ViewportBuilder::default()
        .with_title("AgentGauge menu")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_window_type(X11WindowType::PopupMenu)
        .with_position(app.menu_pos)
        .with_inner_size(app.menu_size)
        .with_active(true);
    let t = app.theme;

    let content_height = ctx.show_viewport_immediate(id, builder, |ui, _class| {
        let vctx = ui.ctx().clone();
        let (focused, close_requested, escape) = vctx.input(|i| {
            (
                i.viewport().focused,
                i.viewport().close_requested(),
                i.key_pressed(Key::Escape),
            )
        });
        if focused == Some(true) {
            app.menu_was_focused = true;
        }
        if close_requested || escape || (app.menu_was_focused && focused == Some(false)) {
            actions.push(Action::CloseMenu);
        }

        let v = ui.visuals_mut();
        v.override_text_color = Some(t.text_primary);
        v.widgets.hovered.weak_bg_fill = t.separator;
        v.widgets.active.weak_bg_fill = t.separator;
        v.selection.bg_fill = t.bar.gamma_multiply(0.6);
        v.selection.stroke = Stroke::new(1.0, t.text_primary);

        let frame = egui::Frame::new()
            .fill(t.bg)
            .stroke(Stroke::new(1.0, t.separator))
            .inner_margin(8);
        let inner = frame.show(ui, |ui| {
            ui.set_width(MENU_WIDTH - 16.0);
            ui.spacing_mut().item_spacing.y = 3.0;

            // Usage summary header.
            if let Some(st) = stats {
                let summary = visible_rows(st, &app.cfg)
                    .iter()
                    .map(|r| format!("{} {}", r.label, format::pct(r.util)))
                    .collect::<Vec<_>>()
                    .join(" · ");
                dim(ui, summary, t.text_secondary);
            }
            ui.separator();

            let mut pick = |a: Action| actions.push(a);
            if item(
                ui,
                if app.details_open {
                    "Hide details"
                } else {
                    "Details…"
                },
            ) {
                pick(Action::ToggleDetails);
                pick(Action::CloseMenu);
            }
            if item(ui, "Refresh now") {
                pick(Action::Refresh);
                pick(Action::CloseMenu);
            }
            ui.separator();

            ui.horizontal(|ui| {
                dim(ui, "Opacity", t.text_dim);
                let mut op = app.cfg.osd_opacity;
                for (val, label) in [(1.0, "100"), (0.75, "75"), (0.5, "50"), (0.25, "25")] {
                    if ui.selectable_value(&mut op, val, label).clicked() {
                        pick(Action::SetOpacity(val));
                    }
                }
            });
            ui.horizontal(|ui| {
                dim(ui, "View", t.text_dim);
                let mut view = app.cfg.osd_view_mode.as_str();
                for (val, label) in [("bars", "Bars"), ("gauge", "Gauge")] {
                    if ui.selectable_value(&mut view, val, label).clicked() {
                        pick(Action::SetView(val));
                    }
                }
            });
            ui.horizontal(|ui| {
                dim(ui, "Position", t.text_dim);
                let mut pos = app.cfg.osd_position.as_str();
                for (val, label) in [
                    ("top-left", "TL"),
                    ("top-right", "TR"),
                    ("bottom-left", "BL"),
                    ("bottom-right", "BR"),
                ] {
                    if ui.selectable_value(&mut pos, val, label).clicked() {
                        pick(Action::SetPosition(val));
                    }
                }
            });
            CollapsingHeader::new("Agents")
                .id_salt("agents")
                .show(ui, |ui| {
                    for (id, label) in [("claude", "Claude Code"), ("codex", "OpenAI Codex")] {
                        let mut on = app.cfg.providers.iter().any(|p| p == id);
                        if ui.checkbox(&mut on, label).clicked() {
                            pick(Action::ToggleAgent(id));
                        }
                    }
                    if let Some(st) = stats
                        && app.cfg.codex_enabled()
                        && !st.codex_available
                        && !st.codex_error.is_empty()
                    {
                        dim(ui, format!("Codex: {}", st.codex_error), t.warn);
                    }
                });
            CollapsingHeader::new("Show")
                .id_salt("show")
                .show(ui, |ui| {
                    let scoped = stats
                        .map(|st| st.scoped_label.as_str())
                        .filter(|l| !l.is_empty())
                        .map(|l| format!("{l} weekly cap"))
                        .unwrap_or_else(|| "Model weekly cap".into());
                    let items: [(&'static str, String, bool); 7] = [
                        ("session", "Session (5h)".into(), app.cfg.claude_enabled()),
                        ("weekly", "Weekly".into(), app.cfg.claude_enabled()),
                        ("scoped", scoped, app.cfg.claude_enabled()),
                        ("codex_session", "Codex 5h".into(), app.cfg.codex_enabled()),
                        ("codex_weekly", "Codex 7d".into(), app.cfg.codex_enabled()),
                        ("cost", "Today's cost".into(), app.cfg.claude_enabled()),
                        (
                            "live",
                            "LIVE tok/min badge".into(),
                            app.cfg.claude_enabled(),
                        ),
                    ];
                    for (id, label, agent_on) in items {
                        let mut on = app.cfg.row_visible(id);
                        // Rows of a disabled agent are greyed out (nothing to show).
                        let resp = ui.add_enabled(agent_on, egui::Checkbox::new(&mut on, label));
                        if resp.clicked() {
                            pick(Action::ToggleRow(id));
                        }
                    }
                });
            CollapsingHeader::new(format!("Theme: {}", app.theme.name))
                .id_salt("theme")
                .show(ui, |ui| {
                    let mut cur = app.theme.name;
                    for th in THEMES {
                        if ui.selectable_value(&mut cur, th.name, th.name).clicked() {
                            pick(Action::SetTheme(th.name));
                        }
                    }
                });
            ui.separator();

            if item(
                ui,
                if app.cfg.osd_minimized {
                    "Restore"
                } else {
                    "Minimize"
                },
            ) {
                pick(Action::ToggleMinimized);
            }
            let mut on_top = app.cfg.osd_always_on_top;
            if ui.checkbox(&mut on_top, "Always on top").clicked() {
                pick(Action::ToggleAlwaysOnTop);
            }
            let mut taskbar = app.cfg.show_in_taskbar;
            let label = if platform::TASKBAR_LIVE_TOGGLE {
                "Show window in taskbar"
            } else {
                "Show window in taskbar (restarts)"
            };
            if ui.checkbox(&mut taskbar, label).clicked() {
                pick(Action::ToggleTaskbar);
            }
            ui.separator();

            dim(
                ui,
                format!("AgentGauge v{}", env!("CARGO_PKG_VERSION")),
                t.text_dim,
            );
            if let Some(st) = stats {
                dim(
                    ui,
                    format!("Updated {}", format::ago(now - st.collected_at)),
                    t.text_dim,
                );
            }
            if item(ui, "Quit") {
                pick(Action::Quit);
            }
        });
        inner.response.rect.height()
    });

    // Fit the window to its content.
    let want = egui::vec2(MENU_WIDTH, content_height.ceil());
    if (want - app.menu_size).length() > 0.5 {
        app.menu_size = want;
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::InnerSize(want));
    }
    actions
}
