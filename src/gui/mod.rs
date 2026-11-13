//! egui/eframe desktop OSD.

mod app;
mod details;
use crate::format;
mod menu;
mod osd;
pub mod platform;
pub mod theme;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, ViewportBuilder};

use crate::config::Config;
use crate::lock::InstanceLock;

/// Launch the OSD. Returns the process exit code.
pub fn run(config_path: PathBuf) -> i32 {
    let cfg = Config::load(&config_path);
    platform::prefer_x11(cfg.force_x11);

    let lock = match InstanceLock::acquire() {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            eprintln!("agent-gauge is already running.");
            return 0;
        }
        Err(e) => {
            eprintln!("agent-gauge: single-instance lock unavailable ({e}); continuing.");
            None
        }
    };

    let mut viewport = ViewportBuilder::default()
        .with_title("AgentGauge")
        .with_app_id("agent-gauge")
        .with_decorations(false)
        .with_transparent(true)
        .with_has_shadow(false)
        .with_resizable(false)
        .with_inner_size([250.0, 120.0])
        .with_taskbar(cfg.show_in_taskbar);
    if cfg.osd_always_on_top {
        viewport = viewport.with_always_on_top();
    }
    if cfg.osd_position == "custom"
        && let (Some(x), Some(y)) = (cfg.osd_x, cfg.osd_y)
    {
        viewport = viewport.with_position(egui::pos2(x, y));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let relaunch = Arc::new(AtomicBool::new(false));
    let relaunch_flag = relaunch.clone();
    let result = eframe::run_native(
        "AgentGauge",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::OsdApp::new(
                cc,
                cfg,
                config_path,
                relaunch_flag,
            )))
        }),
    );

    drop(lock); // release before a relaunched copy tries to take it
    if relaunch.load(Ordering::SeqCst) {
        platform::relaunch();
    }
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("agent-gauge: GUI failed to start: {e}");
            1
        }
    }
}
