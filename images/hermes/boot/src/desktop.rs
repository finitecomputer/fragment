//! Each agent's desktop (docs/computers.md, "Our images"). Hermes v0.21.5
//! gives every profile a desktop of its own, its Bot Desktop, under
//! `<profile>/bot-desktop/` (`tools/bot_desktop/runtime.py`): its own Xvnc
//! (`rfb.sock`), its own browser profile, the lease of who drives it
//! (`lease.json`), and the file whose time says when it was last used
//! (`activity`: stamped by each `computer_use` action, each browser command
//! on it, and by the screen while a person watches it or takes over).
//!
//! The screen (the bridge's) shows each agent's own desktop, named in the
//! screens file (`screens_file`). And a desktop no one uses is stopped: its
//! Xvnc and Xfce hold about 220 MiB, a browser left open far more, and
//! Hermes stops an idle one only from its TUI's gateway, which this image
//! does not run. So the boot does, as Hermes' own watcher would
//! (`runtime.stop_if_idle`): a desktop up and unused for `IDLE_STOP_MS` is
//! stopped with Hermes' own `hermes computer-use screen stop`, which
//! refuses while a person holds its lease. The next use starts it again
//! (an agent's call, `bot_desktop.auto_start`; a viewer, `screen-start`),
//! and what it kept (its browser's profile, in the agent's work) is there.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fragment_bridge::runtime::Agent;
use serde_json::json;

use crate::hermes;

/// Adapt the pinned Python runtime at its existing local-session and dock
/// boundaries. The helper is Python because it extends Hermes' own process
/// and lease helpers; installation and its pin guards stay in this Rust tool.
/// Exact matches make an upstream change a build failure, never a skipped fix.
pub fn install_browser(root: &Path) -> Result<(), String> {
    const FENCE: &str = r#"        engine, result = run_fenced_pair(session_info, lambda: _dispatch_browser_command(
"#;
    const ATTACH: &str = r#"    from tools.bot_desktop import browser as _bd_browser
    return _bd_browser.running_instance_cdp_port(str(_bd_browser.profile_dir()),
                                                 exclude_session=session_info["session_name"])
"#;
    const DOCK: &str = r#"    return "Exec=" + " ".join(quote(arg) for arg in dock_argv(exe, user_data_dir, sandbox_bypass=sandbox_bypass))
"#;
    const STOP: &str = r#"def _stop_locked(sd: Path) -> bool:
    pid = _launcher_pid()
"#;
    const SPAWN: &str = r#"    try:
        result = _unwrap_batch_result(
            _spawn_and_collect(task_id, session_info, cmd_parts, command, engine, timeout, stdin_payload), command)
"#;
    for (rel, replacements) in [
        ("tools/browser_tool_session.py", vec![
            (FENCE, r#"        if command != "close" and (session_info.get("features") or {}).get("local") and _cloud._is_headed_mode() and _cloud._get_browser_engine() != "lightpanda":
            _ensure_screen_for_headed_chromium()
        engine, result = run_fenced_pair(session_info, lambda: _dispatch_browser_command(
"#),
            (ATTACH, r#"    if not _cloud._is_headed_mode() or _cloud._get_browser_engine() == "lightpanda":
        return None
    from tools.bot_desktop.fragment_browser import ensure_started
    port = ensure_started()
    session_info["fragment_desktop_port"] = port
    return port
"#),
            (r#"        if (bd_port := _bot_desktop_attach_port(session_info)) is not None:
"#, r#"        if command != "close" and (bd_port := _bot_desktop_attach_port(session_info)) is not None:
"#),
            (SPAWN, r#"    try:
        port = session_info.get("fragment_desktop_port")
        if command != "close" and port and session_info.get("fragment_connected_port") not in (None, port):
            connected = _spawn_and_collect(task_id, session_info, argv + backend_args + ["--json", "connect", str(port)], "connect", engine, timeout)
            if not connected.get("success"):
                return engine, connected
        if port and command != "close":
            session_info["fragment_connected_port"] = port
        result = _unwrap_batch_result(
            _spawn_and_collect(task_id, session_info, cmd_parts, command, engine, timeout, stdin_payload), command)
"#),
        ]),
        ("tools/bot_desktop/browser.py", vec![
            (DOCK, r#"    return "Exec=" + " ".join(quote(arg) for arg in ["/opt/hermes/.venv/bin/python", "/opt/hermes/tools/bot_desktop/fragment_browser.py", user_data_dir])
"#),
        ]),
        ("tools/bot_desktop/runtime.py", vec![
            (STOP, r#"def _stop_locked(sd: Path) -> bool:
    from tools.bot_desktop.fragment_browser import stop as stop_browser
    stop_browser()
    pid = _launcher_pid()
"#),
        ]),
    ] {
        let path = root.join(rel);
        let mut source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        for (old, new) in replacements {
            if source.matches(old).count() != 1 {
                return Err(format!("{}: the pinned desktop browser integration changed", path.display()));
            }
            source = source.replacen(old, new, 1);
        }
        std::fs::write(&path, source).map_err(|e| format!("{}: {e}", path.display()))?;
        // The base carries unchecked-hash bytecode: merely editing its source
        // does not invalidate it. The image build recompiles these three files.
        let cache = path.parent().expect("a source's directory").join("__pycache__");
        if cache.exists() {
            let prefix = format!("{}.", path.file_stem().expect("a Python module").to_string_lossy());
            for file in std::fs::read_dir(&cache).map_err(|e| e.to_string())? {
                let file = file.map_err(|e| e.to_string())?;
                if file.file_name().to_string_lossy().starts_with(&prefix) {
                    std::fs::remove_file(file.path()).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

/// A desktop up and unused this long is stopped. Hermes' own default is
/// 30 minutes; ten keeps a computer whose agents each opened a browser once
/// from holding gigabytes for half an hour, and a person's pause while
/// reading never stops the desktop they watch (the screen touches it every
/// `ACTIVITY_EVERY_MS` while they do).
pub const IDLE_STOP_MS: u64 = 10 * 60_000;
/// Desktops are looked at this often (or a quarter of a shorter idle
/// bound, a test's).
pub const IDLE_EVERY_MS: u64 = 60_000;
/// Hermes' own stop is given this long (its launcher's group gets 5 s to go).
pub const STOP_MS_MAX: u64 = 30_000;

const _: () = assert!(IDLE_STOP_MS >= 6 * fragment_bridge::screen::ACTIVITY_EVERY_MS, "a watched desktop is touched many times within its idle bound");
const _: () = assert!(IDLE_EVERY_MS <= IDLE_STOP_MS / 4, "an idle desktop is stopped within a quarter of its bound more");

/// Under a profile: the files of its desktop the image names or reads.
pub const RFB: &str = "bot-desktop/rfb.sock";
pub const LEASE: &str = "bot-desktop/lease.json";
pub const ACTIVITY: &str = "bot-desktop/activity";
/// What its launcher publishes once the desktop is up (DISPLAY and the
/// rest), and Hermes' stop removes.
pub const PUBLISHED: &str = "bot-desktop/env";

/// The idle bound a boot uses: `setting` (`HERMES_BOOT_SCREEN_IDLE_MS`, a
/// test's) of at least 2 s, else `IDLE_STOP_MS`; and how often desktops are
/// looked at for it.
pub fn idle_bounds(setting: Option<&str>) -> (u64, u64) {
    let idle = setting.and_then(|v| v.trim().parse::<u64>().ok()).map_or(IDLE_STOP_MS, |ms| ms.max(2_000));
    let every = IDLE_EVERY_MS.min(idle / 4).max(500);
    assert!(every <= idle / 4 && every >= 500, "looked at a few times within the bound, never in a loop");
    (idle, every)
}

/// Whether a desktop is due its stop: up (published), and its last use
/// (its activity's time, or its publish's when it has none, as Hermes'
/// `idle_seconds` reads it) at least `idle_ms` before `now`. A last use
/// after `now` (a clock set back) is no idle time.
pub fn due(published: Option<SystemTime>, activity: Option<SystemTime>, now: SystemTime, idle_ms: u64) -> bool {
    let Some(published) = published else { return false };
    let last = activity.unwrap_or(published);
    now.duration_since(last).is_ok_and(|idle| idle >= Duration::from_millis(idle_ms))
}

/// When a file under `profile` was last changed, if it is there.
pub fn mtime(profile: &Path, rel: &str) -> Option<SystemTime> {
    std::fs::metadata(profile.join(rel)).and_then(|m| m.modified()).ok()
}

/// The bridge's screens file (its `screens.rs`): each agent's own desktop,
/// its lease and its activity, in its own profile.
pub fn screens_file(agents: &[Agent], home: &Path) -> String {
    let at = |a: &Agent, rel: &str| -> PathBuf { hermes::profile_dir(home, &a.fragment).join(rel) };
    let screens: Vec<serde_json::Value> = agents
        .iter()
        .map(|a| {
            json!({
                "agent": a.fragment,
                "rfb": format!("unix:{}", at(a, RFB).display()),
                "lease": at(a, LEASE).display().to_string(),
                "activity": at(a, ACTIVITY).display().to_string(),
            })
        })
        .collect();
    json!({ "screens": screens }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(label: &str) -> Agent {
        Agent { fragment: format!("{label}--k3x9"), identity: format!("npub1{label}"), name: label.into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None }
    }

    /// Valid: each agent's screen is its own profile's desktop, as the
    /// bridge reads it; two agents, two desktops.
    #[test]
    fn each_agent_has_its_own_desktop() {
        let file = screens_file(&[agent("juniper"), agent("fred")], Path::new("/data/hermes"));
        let screens = fragment_bridge::screens::parse(file.as_bytes()).expect("the bridge reads it");
        assert_eq!(screens.len(), 2);
        let j = &screens["juniper--k3x9"];
        assert_eq!(j.rfb, fragment_bridge::screen::Target::Unix("/data/hermes/profiles/juniper--k3x9/bot-desktop/rfb.sock".into()));
        assert_eq!(j.lease.as_deref(), Some(Path::new("/data/hermes/profiles/juniper--k3x9/bot-desktop/lease.json")));
        assert_eq!(j.activity.as_deref(), Some(Path::new("/data/hermes/profiles/juniper--k3x9/bot-desktop/activity")));
        assert_eq!(screens["fred--k3x9"].rfb, fragment_bridge::screen::Target::Unix("/data/hermes/profiles/fred--k3x9/bot-desktop/rfb.sock".into()));
        assert!(fragment_bridge::screens::parse(screens_file(&[], Path::new("/data/hermes")).as_bytes()).unwrap().is_empty(), "no agent, no screen");
    }

    /// Valid, invalid and replay: up and unused past the bound is due; down,
    /// used within it, or used "after" now is not; its activity counts over
    /// its publish.
    #[test]
    fn an_idle_desktop_is_due_its_stop() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let idle = IDLE_STOP_MS;
        assert!(due(Some(t0), None, at(idle), idle), "published and never used since: its publish is its last use");
        assert!(!due(Some(t0), None, at(idle - 1), idle));
        assert!(!due(None, Some(t0), at(idle * 3), idle), "down: nothing to stop");
        assert!(!due(Some(t0), Some(at(5_000)), at(idle), idle), "used since it started");
        assert!(due(Some(t0), Some(at(5_000)), at(idle + 5_000), idle));
        assert!(!due(Some(t0), Some(at(idle * 2)), at(idle), idle), "a use after now (a clock set back) is no idle time");
    }

    #[test]
    fn the_idle_bound_is_a_tests_or_ten_minutes() {
        // what the agents are told (computer.md) is the bound kept
        assert_eq!(IDLE_STOP_MS, 10 * 60_000);
        assert!(include_str!("computer.md").contains("no one has used it for ten minutes"));
        assert_eq!(idle_bounds(None), (IDLE_STOP_MS, IDLE_EVERY_MS));
        assert_eq!(idle_bounds(Some("30000")), (30_000, 7_500));
        assert_eq!(idle_bounds(Some("1")), (2_000, 500), "at least 2 s, looked at every half second");
        assert_eq!(idle_bounds(Some("soon")), (IDLE_STOP_MS, IDLE_EVERY_MS));
    }
}
