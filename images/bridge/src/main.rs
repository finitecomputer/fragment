//! `fragment-bridge`: the process a computer image runs to connect its agent
//! runtime to the fragment API (docs/computers.md).
//!
//! ```text
//! fragment-bridge run      the bridge (BRIDGE_RUNTIME=relay|script, run for
//!                          every agent that names no other runtime), and
//!                          its screens on BRIDGE_SCREEN_LISTEN
//! fragment-bridge version
//! ```
//!
//! Its settings are environment variables (docs/bridge.md lists them);
//! secrets are files named by path, never values.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fragment_bridge::driver::{self, Config};
use fragment_bridge::engine::Settings;
use fragment_bridge::runtime::relay::{Relay, RelayConfig};
use fragment_bridge::runtime::script::{Script, ScriptConfig};
use fragment_bridge::runtime::claude::{Claude, ClaudeConfig};
use fragment_bridge::runtime::{Runtime, Runtimes};
use fragment_bridge::{ev, limits, screen};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    env(name).unwrap_or_else(|| default.to_string())
}

fn fail(why: &str) -> ! {
    ev!("bridge.misconfigured", { "why": why });
    std::process::exit(2);
}

fn parse_ms(name: &str, default: u64) -> u64 {
    match env(name) {
        Some(v) => v.parse().unwrap_or_else(|_| fail(&format!("{name} is not a number of ms"))),
        None => default,
    }
}

/// The runtimes the bridge runs: the default (`runtime()`), and Claude Code
/// for agents whose `agent.json` names `claude`, only where the image sets
/// `BRIDGE_CLAUDE_BIN` (docs/bridge.md, "The Claude runtime").
fn runtimes() -> Runtimes {
    let r = Runtimes::from(runtime());
    let Some(bin) = env("BRIDGE_CLAUDE_BIN") else { return r };
    let run_as = env("BRIDGE_CLAUDE_USER").map(|u| {
        let (uid, gid) = u.split_once(':').unwrap_or_else(|| fail("BRIDGE_CLAUDE_USER is uid:gid"));
        (uid.parse().unwrap_or_else(|_| fail("BRIDGE_CLAUDE_USER's uid is a number")), gid.parse().unwrap_or_else(|_| fail("BRIDGE_CLAUDE_USER's gid is a number")))
    });
    let config = ClaudeConfig {
        bin: PathBuf::from(bin),
        config_dir: PathBuf::from(env_or("BRIDGE_CLAUDE_CONFIG_DIR", "/data/claude")),
        work_dir: PathBuf::from(env_or("BRIDGE_CLAUDE_WORK_DIR", "/data/work")),
        state_dir: PathBuf::from(env_or("BRIDGE_CLAUDE_STATE_DIR", "/data/bridge/claude")),
        model: env("BRIDGE_CLAUDE_MODEL"),
        idle: Duration::from_millis(parse_ms("BRIDGE_CLAUDE_IDLE_MS", 300_000)),
        drain: Duration::from_millis(fragment_bridge::runtime::claude::DRAIN_MS_DEFAULT),
        run_as,
    };
    r.with("claude", Box::new(Claude { config }))
}

/// The runtime agents that name none run (`runtime::DEFAULT`): Hermes'
/// Relay, or the stub's scripted agent standing in for it.
fn runtime() -> Box<dyn Runtime> {
    match env_or("BRIDGE_RUNTIME", "relay").as_str() {
        "relay" => {
            let listen: SocketAddr = env_or("BRIDGE_RELAY_LISTEN", "127.0.0.1:8650").parse().unwrap_or_else(|_| fail("BRIDGE_RELAY_LISTEN is not host:port"));
            let secret_file = env("BRIDGE_RELAY_SECRET_FILE").unwrap_or_else(|| fail("BRIDGE_RELAY_SECRET_FILE names the Relay secret's file"));
            let secret = std::fs::read_to_string(&secret_file).unwrap_or_else(|e| fail(&format!("{secret_file}: {e}"))).trim().to_string();
            if secret.len() < 32 {
                fail("the Relay secret is at least 32 characters");
            }
            let config = RelayConfig {
                listen,
                gateway_id: env_or("GATEWAY_RELAY_ID", "fragment-computer"),
                secret,
                media_dir: PathBuf::from(env_or("BRIDGE_RELAY_MEDIA_DIR", "/tmp/bridge-relay-media")),
                // a test's fake servers, local and plain http (runtime/relay/fetch.rs)
                media_local: match env("BRIDGE_MEDIA_LOCAL").as_deref() {
                    None | Some("") => false,
                    Some("allow") => true,
                    Some(_) => fail("BRIDGE_MEDIA_LOCAL is `allow`, or unset"),
                },
            };
            Box::new(Relay { config })
        }
        "script" => Box::new(Script { config: ScriptConfig { pace: Duration::from_millis(parse_ms("BRIDGE_SCRIPT_PACE_MS", 40)), scratch: PathBuf::from(env_or("BRIDGE_SCRIPT_SCRATCH", "/tmp/bridge-script")), data: PathBuf::from(env_or("BRIDGE_SCRIPT_DATA", "/data")) } }),
        other => fail(&format!("BRIDGE_RUNTIME {other:?} is neither relay nor script")),
    }
}

/// `BRIDGE_HELD_LEAVE_OUT`: what the save may leave out (whitespace
/// between), each a pattern the platform takes.
fn left_out() -> Vec<String> {
    let patterns: Vec<String> = env("BRIDGE_HELD_LEAVE_OUT").map(|v| v.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
    if patterns.len() > limits::HELD_PATTERNS_MAX || !patterns.iter().all(|p| driver::left_out_ok(p)) {
        fail(&format!("BRIDGE_HELD_LEAVE_OUT is at most {} patterns of letters, digits and ._-/*?[]", limits::HELD_PATTERNS_MAX));
    }
    patterns
}

fn screen_config() -> Option<screen::ScreenConfig> {
    let listen: SocketAddr = env("BRIDGE_SCREEN_LISTEN")?.parse().unwrap_or_else(|_| fail("BRIDGE_SCREEN_LISTEN is not host:port"));
    // one display for every agent was the cut model: each agent's is its own now
    if env("BRIDGE_SCREEN_RFB").is_some() {
        fail("BRIDGE_SCREEN_RFB is gone: name each agent's display in BRIDGE_SCREENS_FILE");
    }
    let screens_file = env("BRIDGE_SCREENS_FILE").map(PathBuf::from);
    let start = env("BRIDGE_SCREEN_START").map(|s| s.split_whitespace().map(str::to_string).collect());
    // bound here, at the start; a port in use leaves the bridge without a
    // screen, as a screen that fails does (driver.rs), not without agents
    let listener = match std::net::TcpListener::bind(listen) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            ev!("screen.failed", { "error": format!("screen listen {listen}: {e}") });
            return None;
        }
    };
    Some(screen::ScreenConfig { listener, dir: PathBuf::from(env_or("BRIDGE_SCREEN_DIR", "/opt/fragment/screen")), screens_file, start })
}

/// `BRIDGE_SIMPLEX_BIN` and the rest: SimpleX, when the image names its
/// core (simplex/run.rs); unset, none, and nothing of it runs.
fn simplex_config() -> Option<fragment_bridge::simplex::run::Config> {
    let bin = PathBuf::from(env("BRIDGE_SIMPLEX_BIN")?);
    if !bin.is_absolute() {
        fail("BRIDGE_SIMPLEX_BIN is the core's absolute path");
    }
    let dir = PathBuf::from(env_or("BRIDGE_SIMPLEX_DIR", "/data/simplex"));
    if !dir.is_absolute() {
        fail("BRIDGE_SIMPLEX_DIR is an absolute path");
    }
    Some(fragment_bridge::simplex::run::Config { bin, dir, smp: env("BRIDGE_SIMPLEX_SMP"), xftp: env("BRIDGE_SIMPLEX_XFTP") })
}

/// SIGTERM (or SIGINT) turns `stop` true; the process is gone within
/// `SHUTDOWN_MS_MAX` of it, whatever is left (docs/computers.md).
fn on_signal(stop: tokio::sync::watch::Sender<bool>) {
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("a SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("a SIGINT handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        ev!("bridge.signal", { "graceMs": limits::SHUTDOWN_MS_MAX });
        let _ = stop.send(true);
        tokio::time::sleep(Duration::from_millis(limits::SHUTDOWN_MS_MAX)).await;
        ev!("bridge.forced_exit");
        // no SimpleX core outlives the bridge, even now
        fragment_bridge::simplex::run::kill_cores();
        std::process::exit(0);
    });
}

#[tokio::main]
async fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    let (stop_tx, stop) = tokio::sync::watch::channel(false);
    match cmd.as_str() {
        "version" => {
            println!("fragment-bridge {}", env!("CARGO_PKG_VERSION"));
        }
        "run" => {
            on_signal(stop_tx);
            let api = env("FRAGMENT_API").unwrap_or_else(|| fail("FRAGMENT_API is the fragment API's address"));
            let cfg = Config {
                api,
                state_dir: PathBuf::from(env_or("BRIDGE_STATE_DIR", "/data/bridge")),
                media_dir: PathBuf::from(env_or("BRIDGE_MEDIA_DIR", "/tmp/bridge-media")),
                restore_pending: env("RESTORE_PENDING").is_some_and(|v| v == "1"),
                restored: PathBuf::from(env_or("BRIDGE_RESTORED", "/run/computer/restored")),
                hold: PathBuf::from(env_or("BRIDGE_HOLD", "/run/computer/hold")),
                held: PathBuf::from(env_or("BRIDGE_HELD", "/run/computer/held")),
                left_out: left_out(),
                settings: Settings { prompt_ttl_ms: parse_ms("BRIDGE_PROMPT_TTL_MS", limits::PROMPT_TTL_MS_DEFAULT), turn_idle_ms: parse_ms("BRIDGE_TURN_IDLE_MS", limits::TURN_IDLE_MS_MAX) },
                agents_file: env("BRIDGE_AGENTS_FILE").map(PathBuf::from),
                screen: screen_config(),
                keepalive_ping: Duration::from_millis(match parse_ms("BRIDGE_KEEPALIVE_PING_MS", limits::KEEPALIVE_PING_MS) {
                    0 => fail("BRIDGE_KEEPALIVE_PING_MS is more than 0"),
                    ms => ms,
                }),
                simplex: simplex_config(),
            };
            ev!("bridge.boot", { "computer": env("FRAGMENT_COMPUTER"), "image": env("FRAGMENT_IMAGE"), "restorePending": cfg.restore_pending, "agentsFile": cfg.agents_file.as_ref().map(|p| p.display().to_string()), "simplex": cfg.simplex.is_some() });
            let ran = driver::run(cfg, runtimes(), stop).await;
            // its SimpleX cores stopped and reaped first, within the shutdown's bound
            fragment_bridge::simplex::run::stop_cores(Duration::from_millis(limits::SHUTDOWN_MS_MAX / 2)).await;
            match ran {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    ev!("bridge.failed", { "error": e.to_string() });
                    std::process::exit(1);
                }
            }
        }
        other => fail(&format!("{other:?}: run or version")),
    }
}
