use clap::Parser;

use agent_gauge::cli::{self, Args};
use agent_gauge::config;

fn main() {
    let args = Args::parse();
    if let Err(e) = agent_gauge::migration::initialize(args.config.is_none()) {
        eprintln!("agent-gauge: {e}");
        std::process::exit(1);
    }

    if args.panel_snapshot {
        std::process::exit(agent_gauge::panel::print_snapshot());
    }

    if args.is_cli() {
        std::process::exit(cli::run(&args));
    }

    if args.detach {
        std::process::exit(detach());
    }

    if args.panel_service {
        std::process::exit(agent_gauge::panel::run(args.config_path()));
    }

    std::process::exit(agent_gauge::gui::run(args.config_path()));
}

/// Respawn ourselves without `--detach` in a new session, logging to the cache dir.
fn detach() -> i32 {
    use std::process::{Command, Stdio};

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("agent-gauge: cannot locate own executable: {e}");
            return 1;
        }
    };
    let child_args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--detach" && a != "-d")
        .collect();

    let log_dir = config::cache_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("widget.log"));
    let (out, err) = match log.and_then(|f| Ok((f.try_clone()?, f))) {
        Ok((a, b)) => (Stdio::from(a), Stdio::from(b)),
        Err(_) => (Stdio::null(), Stdio::null()),
    };

    let mut cmd = Command::new(exe);
    cmd.args(child_args)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("agent-gauge: failed to start in background: {e}");
            1
        }
    }
}
