//! `fragment-lan --config <file>`: serves a box's zone, its door and its
//! root (crate docs; docs/self-host.md, seam 9) until SIGINT or SIGTERM.
//! Its log is its standard error.

use std::path::Path;
use std::process::ExitCode;

use fragment_lan::{Config, Lan};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = match args.as_slice() {
        [flag, path] if flag == "--config" => path.clone(),
        _ => {
            eprintln!("usage: fragment-lan --config <file>");
            return ExitCode::from(2);
        }
    };
    let config = match Config::read(Path::new(&path)) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("fragment-lan: {e}");
            return ExitCode::from(2);
        }
    };
    match Lan::run(config, stopped()) {
        Ok(()) => {
            eprintln!("fragment-lan: stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fragment-lan: {e}");
            ExitCode::FAILURE
        }
    }
}

/// SIGINT or SIGTERM, whichever comes first.
async fn stopped() {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut int), Ok(mut term)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) else {
        eprintln!("fragment-lan: cannot hear signals; stop it with SIGKILL");
        return std::future::pending().await;
    };
    tokio::select! {
        _ = int.recv() => {}
        _ = term.recv() => {}
    }
}
