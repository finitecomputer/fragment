//! `fragment-identity-door --config <file>`: serves a box's sign-in from
//! its tailnet's identity (crate docs; docs/self-host.md, seam 4) until
//! SIGINT or SIGTERM. Its log is its standard error: one line a sign-in.

use std::path::Path;
use std::process::ExitCode;

use fragment_identity_door::{Config, Door};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = match args.as_slice() {
        [flag, path] if flag == "--config" => path.clone(),
        _ => {
            eprintln!("usage: fragment-identity-door --config <file>");
            return ExitCode::from(2);
        }
    };
    let config = match Config::read(Path::new(&path)) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("identity-door: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    match Door::run(config, stopped()) {
        Ok(()) => {
            eprintln!("identity-door: stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("identity-door: {e}");
            ExitCode::FAILURE
        }
    }
}

/// SIGINT or SIGTERM, whichever comes first.
async fn stopped() {
    use futures_util::future::{select, Either};
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut int), Ok(mut term)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) else {
        eprintln!("identity-door: cannot hear signals; stop it with SIGKILL");
        return std::future::pending().await;
    };
    let heard = select(Box::pin(int.recv()), Box::pin(term.recv())).await;
    match heard {
        Either::Left(_) | Either::Right(_) => {}
    };
}
