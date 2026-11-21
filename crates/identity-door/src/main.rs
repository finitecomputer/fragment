//! `fragment-identity-door --config <file>`: serves a box's sign-in (crate
//! docs; docs/self-host.md, seam 4) until SIGINT or SIGTERM. Its log is its
//! standard error: one line a sign-in.
//!
//! `fragment-identity-door invite --config <file> --email <email> [--hours
//! <n>]`: a passkey door's one-time invite for a new account (passkeys.rs),
//! its link printed on standard output, for its maker alone to hand on.

use std::path::Path;
use std::process::ExitCode;

use fragment_identity_door::{Config, Door};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("invite") {
        return invite(&args[1..]);
    }
    let path = match args.as_slice() {
        [flag, path] if flag == "--config" => path.clone(),
        _ => {
            eprintln!("usage: fragment-identity-door --config <file> | invite --config <file> --email <email> [--hours <n>]");
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

/// `invite --config <file> --email <email> [--hours <n>]`.
fn invite(args: &[String]) -> ExitCode {
    let flag = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let (Some(path), Some(email)) = (flag("--config"), flag("--email")) else {
        eprintln!("usage: fragment-identity-door invite --config <file> --email <email> [--hours <n>]");
        return ExitCode::from(2);
    };
    let hours: i64 = match flag("--hours").map(|h| h.parse()) {
        None => fragment_identity_door::passkeys::INVITE_TTL_S / 3600,
        Some(Ok(h)) => h,
        Some(Err(_)) => {
            eprintln!("identity-door: --hours is a whole number");
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
    let fragment_identity_door::SignIn::Passkeys(k) = &config.signin else {
        eprintln!("identity-door: {path} signs people in through a provider: it makes no invites");
        return ExitCode::from(2);
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let made = fragment_identity_door::passkeys::Passkeys::new(k.clone()).and_then(|p| p.invite(&email, hours * 3600, now));
    match made {
        Ok(token) => {
            println!("{}/invite/{token}", config.public_url.as_str().trim_end_matches('/'));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("identity-door: no invite: {e}");
            ExitCode::FAILURE
        }
    }
}
