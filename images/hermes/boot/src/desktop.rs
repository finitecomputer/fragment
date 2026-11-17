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
//! Xvnc and Xfce hold about 220 MiB, a browser left open far more. The boot
//! does that alone (Hermes' own watchers are off: hermes.rs,
//! `managed_config`): a desktop up and unused for `IDLE_STOP_MS` has its
//! browser closed first, as a person would close it (`close_browser`), then
//! is stopped with Hermes' own `hermes computer-use screen stop`, which
//! refuses while a person holds its lease. The next use starts it again
//! (an agent's call, `bot_desktop.auto_start`; a viewer, `screen-start`),
//! and what it kept (its browser's profile, in the agent's work) is there.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fragment_bridge::runtime::Agent;
use serde_json::json;

use crate::hermes;

/// Where Hermes' browser_exec decides whether the agent's tabs come to the
/// front (relative to Hermes' root), and the one line of it the image
/// changes: on the agent's own Bot Desktop browser (`local_engine` is
/// `local`), they do, as on a cloud browser. Upstream leaves every local
/// tab in the background, so as not to switch the tab a person looks at in
/// their own Chrome; on the Bot Desktop that hid the page the agent worked
/// on from the person watching its screen, and a file input clicked there
/// opened no file dialog (2026-11-17). An exact match: an upstream change
/// fails the image's build, never skips this (docs/technical-debt-ledger.md,
/// "The image brings the agent's browser tab to the front").
pub const FRONT_TAB_FILE: &str = "tools/browser_use_cli.py";
const FRONT_TAB_FROM: &str = "    from tools.browser_use_shot import ACTIVATE_NEVER, harness_command
    cmd = harness_command(cmd, activate=env.pop(_CLOUD_BROWSER_SENTINEL, None) or ACTIVATE_NEVER)
";
const FRONT_TAB_TO: &str = "    from tools.browser_use_shot import ACTIVATE_ALWAYS, ACTIVATE_NEVER, harness_command
    # Fragment's image (images/hermes/boot/src/desktop.rs): the Bot Desktop's tab in front.
    cmd = harness_command(cmd, activate=env.pop(_CLOUD_BROWSER_SENTINEL, None) or (ACTIVATE_ALWAYS if local_engine == \"local\" else ACTIVATE_NEVER))
";

/// Makes `FRONT_TAB_FILE` under `root` bring the Bot Desktop's tab to the
/// front, once, and drops its stale bytecode (the base's is unchecked-hash:
/// editing a source does not invalidate it; the image's build compiles it
/// again).
pub fn front_tabs(root: &Path) -> Result<(), String> {
    let path = root.join(FRONT_TAB_FILE);
    let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if source.matches(FRONT_TAB_FROM).count() != 1 || !source.contains("local_engine = env.pop(_BOT_DESKTOP_BROWSER_SENTINEL, None)") {
        return Err(format!("{}: Hermes' tab activation changed; check the Bot Desktop's tabs come to the front", path.display()));
    }
    std::fs::write(&path, source.replacen(FRONT_TAB_FROM, FRONT_TAB_TO, 1)).map_err(|e| format!("{}: {e}", path.display()))?;
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
    Ok(())
}

/// How long a desktop's browser is given to close before its desktop stops
/// all the same.
pub const CLOSE_MS_MAX: u64 = 10_000;

/// The browser on `profile`'s desktop, the agent's own: its browser
/// profile (`hermes::BROWSER_PROFILE`) must resolve to exactly `own`, the
/// agent's work's (`hermes::browser_dir`, as the boot links it), or be that
/// path itself, unlinked (an older profile's own directory). The agents
/// share one user, so one could point its link at another's profile; the
/// boot, as root, never acts on that. Then Chromium's own `SingletonLock`
/// there (`<host>-<pid>`) names it, as `proc` (`/proc`) has it: a live
/// process, no zombie, whose command line names that browser profile and is
/// not one of its helpers (`--type=`). A pid the lock names that is now
/// another process's is none.
pub fn browser_pid(profile: &Path, own: &Path, proc: &Path) -> Option<u32> {
    let link = profile.join(hermes::BROWSER_PROFILE);
    let real = std::fs::canonicalize(&link).ok()?;
    if real != own && real != link {
        return None;
    }
    let lock = std::fs::read_link(real.join("SingletonLock")).ok()?;
    let pid: u32 = lock.to_string_lossy().rsplit_once('-')?.1.parse().ok()?;
    let at = proc.join(pid.to_string());
    let stat = std::fs::read_to_string(at.join("stat")).ok()?;
    if stat.rsplit_once(") ").is_none_or(|(_, rest)| rest.starts_with('Z')) {
        return None;
    }
    let cmdline = std::fs::read(at.join("cmdline")).ok()?;
    let mut args: Vec<String> = cmdline.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    // Chromium rewrites its process title: its arguments then read as one,
    // joined by spaces (as psutil, Hermes' reader, splits them again)
    if let [title] = args.as_slice() {
        args = title.split(' ').map(str::to_string).collect();
    }
    // named as given, never by resolving a path the agent can link
    let ours = args.iter().any(|a| a.strip_prefix("--user-data-dir=").is_some_and(|dir| Path::new(dir) == link || Path::new(dir) == real));
    (ours && !args.iter().any(|a| a.starts_with("--type="))).then_some(pid)
}

/// Closes `profile`'s desktop browser, if it has one, as a person closing
/// it would: CDP's `Browser.close` over its own DevTools socket, Chromium's
/// normal shutdown, which writes its cookies. Hermes' stop ends the
/// desktop's process group, or its display, under the browser, and a
/// SIGTERM is Chromium's end-of-session exit: either way the cookies a page
/// set in Chromium's last 30 s (it writes them that often) were lost, and a
/// site that rotates its sign-in cookie signed the agent out. The socket
/// is the one its `DevToolsActivePort` names, which the agent can write,
/// so it is used only once CDP itself names this browser's process; else
/// a SIGTERM. Waits at most `CLOSE_MS_MAX`. Whether a browser was there,
/// and whether it had gone.
pub async fn close_browser(profile: &Path, own: &Path, proc: &Path) -> Option<bool> {
    let pid = browser_pid(profile, own, proc)?;
    if ask_to_close(own, pid).await.is_err() {
        // SAFETY: kill(2) with a pid read and checked just now; no memory is shared.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
    let t = std::time::Instant::now();
    // bounded by CLOSE_MS_MAX
    while t.elapsed() < Duration::from_millis(CLOSE_MS_MAX) {
        if browser_pid(profile, own, proc) != Some(pid) {
            return Some(true);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Some(false)
}

/// The most a DevTools answer is read, and how many it reads for its own.
const CDP_REPLY_BYTES: usize = 64 * 1024;
const CDP_REPLIES_MAX: usize = 16;

/// Sends `Browser.close` to the browser `pid` on the browser profile
/// `browser`, once CDP's `SystemInfo.getProcessInfo` names `pid` as the
/// browser behind the socket its `DevToolsActivePort` names (a port reused,
/// or a file written to point elsewhere, closes nothing).
async fn ask_to_close(browser: &Path, pid: u32) -> Result<(), String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let file = std::fs::read_to_string(browser.join("DevToolsActivePort")).map_err(|e| e.to_string())?;
    let mut lines = file.lines();
    let port: u16 = lines.next().and_then(|p| p.trim().parse().ok()).filter(|p| *p > 0).ok_or("no DevTools port")?;
    let path = lines.next().map(str::trim).filter(|p| p.strip_prefix("/devtools/browser/").is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))).ok_or("no DevTools browser path")?;
    let base = fragment_bridge::net::Base::parse(&format!("http://127.0.0.1:{port}"))?;
    let mut ws = fragment_bridge::net::connect_ws(&base, path, &[]).await?;
    let send = |id: u32, method: &str| Message::text(serde_json::json!({ "id": id, "method": method }).to_string());
    ws.send(send(1, "SystemInfo.getProcessInfo")).await.map_err(|e| e.to_string())?;
    let read = async {
        // bounded: CDP_REPLIES_MAX messages
        for _ in 0..CDP_REPLIES_MAX {
            let Some(Ok(Message::Text(text))) = ws.next().await else { return Err("the DevTools socket closed".to_string()) };
            if text.len() > CDP_REPLY_BYTES {
                return Err("a DevTools answer too large".into());
            }
            let answer: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if answer["id"] == 1 {
                let ours = answer["result"]["processInfo"].as_array().is_some_and(|ps| ps.iter().any(|p| p["type"] == "browser" && p["id"] == pid));
                return if ours { Ok(()) } else { Err("the DevTools socket is another process's".into()) };
            }
        }
        Err("no answer from DevTools".into())
    };
    tokio::time::timeout(Duration::from_secs(2), read).await.map_err(|_| "no answer from DevTools in 2 s".to_string())??;
    ws.send(send(2, "Browser.close")).await.map_err(|e| e.to_string())
}

/// Whether a person holds `profile`'s desktop (its lease, as the bridge
/// writes it and Hermes reads it, failing closed: one that does not read is
/// a person's). None written is the agent's.
pub fn held_by_a_person(profile: &Path) -> bool {
    match std::fs::read(profile.join(LEASE)) {
        Ok(bytes) => !matches!(fragment_bridge::lease::parse(&bytes).holder, fragment_bridge::lease::Holder::Agent),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
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

    /// Goal: the desktop's browser is the live process Chromium's lock
    /// names, on that browser profile; never a helper, another profile's
    /// browser, a zombie, or a pid recycled since. Then it is closed by a
    /// signal to it alone, and the close is waited for.
    #[tokio::test]
    async fn the_desktops_browser_is_closed_alone_and_waited_for() {
        let root = std::env::temp_dir().join(format!("close-browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let profile = root.join("profile");
        let own = root.join("work/browser-profile");
        let browser = profile.join(hermes::BROWSER_PROFILE);
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(browser.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&own, &browser).unwrap();
        let fake = root.join("proc");
        let process = |pid: u32, state: char, args: &[&str]| {
            let at = fake.join(pid.to_string());
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join("stat"), format!("{pid} (chromium) {state} 1 1")).unwrap();
            std::fs::write(at.join("cmdline"), args.iter().map(|a| format!("{a}\0")).collect::<String>()).unwrap();
        };
        let ours = format!("--user-data-dir={}", browser.display());
        let lock = |pid: u32| {
            let _ = std::fs::remove_file(own.join("SingletonLock"));
            std::os::unix::fs::symlink(format!("box-{pid}"), own.join("SingletonLock")).unwrap();
        };
        assert_eq!(browser_pid(&profile, &own, &fake), None, "no lock, no browser");
        process(41, 'S', &["/usr/lib/chromium/chromium", "--no-sandbox", &ours]);
        lock(41);
        assert_eq!(browser_pid(&profile, &own, &fake), Some(41));
        // Chromium's own process title: its arguments in one, by spaces
        process(40, 'S', &[&format!("/usr/lib/chromium/chromium --no-sandbox {ours} about:blank")]);
        lock(40);
        assert_eq!(browser_pid(&profile, &own, &fake), Some(40), "a rewritten title");
        process(39, 'S', &[&format!("/usr/lib/chromium/chromium --type=renderer {ours}")]);
        lock(39);
        assert_eq!(browser_pid(&profile, &own, &fake), None, "a helper's rewritten title");
        process(42, 'S', &["/usr/lib/chromium/chromium", "--type=renderer", &ours]);
        lock(42);
        assert_eq!(browser_pid(&profile, &own, &fake), None, "a helper");
        process(43, 'S', &["/usr/lib/chromium/chromium", "--user-data-dir=/data/work/fred--k3x9/browser-profile"]);
        lock(43);
        assert_eq!(browser_pid(&profile, &own, &fake), None, "another profile's");
        process(44, 'Z', &[]);
        lock(44);
        assert_eq!(browser_pid(&profile, &own, &fake), None, "a zombie");
        lock(45);
        assert_eq!(browser_pid(&profile, &own, &fake), None, "a pid no process has");
        // its link pointed at another agent's browser profile: never acted on
        let other = root.join("other/browser-profile");
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink("box-41", other.join("SingletonLock")).unwrap();
        process(41, 'S', &["/usr/lib/chromium/chromium", &format!("--user-data-dir={}", other.display())]);
        std::fs::remove_file(&browser).unwrap();
        std::os::unix::fs::symlink(&other, &browser).unwrap();
        assert_eq!(browser_pid(&profile, &own, &fake), None, "a link to another agent's profile");
        process(41, 'S', &["/usr/lib/chromium/chromium", &ours]);
        assert_eq!(browser_pid(&profile, &own, &fake), None, "nor its browser named through that link");
        std::fs::remove_file(&browser).unwrap();
        std::os::unix::fs::symlink(&own, &browser).unwrap();
        // a real process holding the profile (its argv names it), closed
        let mut child = std::process::Command::new("sh").args(["-c", "sleep 30; :", &ours]).spawn().unwrap();
        lock(child.id());
        let proc = Path::new("/proc");
        // bounded: 2 s for the child's exec to show its arguments
        let t = std::time::Instant::now();
        while browser_pid(&profile, &own, proc).is_none() && t.elapsed() < Duration::from_secs(2) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(browser_pid(&profile, &own, proc), Some(child.id()));
        let waited = tokio::spawn(async move { child.wait().unwrap() });
        assert_eq!(close_browser(&profile, &own, proc).await, Some(true), "with no DevTools socket of its own: closed by its signal, and waited for");
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&waited.await.unwrap()), Some(libc::SIGTERM));
        assert_eq!(close_browser(&profile, &own, proc).await, None, "replay: none left to close");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal: a person holding the desktop is never interrupted by its idle
    /// stop; a lease that does not read is a person's (fail closed); none is
    /// the agent's.
    #[test]
    fn a_desktop_a_person_holds_is_theirs() {
        let profile = std::env::temp_dir().join(format!("held-{}", std::process::id()));
        std::fs::create_dir_all(profile.join("bot-desktop")).unwrap();
        assert!(!held_by_a_person(&profile), "no lease: the agent's");
        for (lease, held) in [(r#"{"holder": "agent", "epoch": 2}"#, false), (r#"{"holder": "human", "viewer_id": "v", "epoch": 3}"#, true), ("not json", true)] {
            std::fs::write(profile.join(LEASE), lease).unwrap();
            assert_eq!(held_by_a_person(&profile), held, "{lease}");
        }
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// Goal: the image's one change to Hermes' tab activation applies once
    /// to the pinned source, its old bytecode dropped. Replay: a source
    /// already changed is refused, as is an upstream that changed the line
    /// (the build fails, never skips it).
    #[test]
    fn the_bot_desktops_tab_comes_to_the_front_once() {
        let root = std::env::temp_dir().join(format!("front-tabs-{}", std::process::id()));
        let tools = root.join("tools");
        std::fs::create_dir_all(tools.join("__pycache__")).unwrap();
        let pinned = format!("def browser_exec():\n    local_engine = env.pop(_BOT_DESKTOP_BROWSER_SENTINEL, None)\n{}    return cmd\n", super::FRONT_TAB_FROM);
        std::fs::write(tools.join("browser_use_cli.py"), &pinned).unwrap();
        std::fs::write(tools.join("__pycache__/browser_use_cli.cpython-314.pyc"), b"old").unwrap();
        std::fs::write(tools.join("__pycache__/browser_use_shot.cpython-314.pyc"), b"other").unwrap();
        super::front_tabs(&root).unwrap();
        let patched = std::fs::read_to_string(tools.join("browser_use_cli.py")).unwrap();
        assert!(patched.contains("or (ACTIVATE_ALWAYS if local_engine == \"local\" else ACTIVATE_NEVER))") && !patched.contains(super::FRONT_TAB_FROM), "{patched}");
        assert!(!tools.join("__pycache__/browser_use_cli.cpython-314.pyc").exists() && tools.join("__pycache__/browser_use_shot.cpython-314.pyc").exists(), "its own stale bytecode only");
        assert!(super::front_tabs(&root).is_err(), "replay: already changed");
        std::fs::write(tools.join("browser_use_cli.py"), pinned.replace("or ACTIVATE_NEVER)", "or ACTIVATE_IF_CLOUD)")).unwrap();
        assert!(super::front_tabs(&root).unwrap_err().contains("tab activation changed"), "an upstream change fails the build");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn agent(label: &str) -> Agent {
        Agent { fragment: format!("{label}--k3x9"), identity: format!("npub1{label}"), name: label.into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None, claude: Default::default() }
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
