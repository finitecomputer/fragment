//! What the Hermes image writes for its Hermes at each boot (and for an
//! agent assigned while it runs), as pure functions of the computer's
//! agents: the managed overlay (`/etc/hermes/config.yaml`, merged over every
//! profile's config), each agent's profile config, the gateway's Relay
//! environment; and the wire of the
//! gateway's control socket. YAML is written by hand: every string is a
//! JSON string, which is a YAML double-quoted scalar.

use std::path::{Path, PathBuf};

use fragment_bridge::api::{model_id_ok, Home, ModelIds};
use fragment_bridge::runtime::relay::wire;
use fragment_bridge::runtime::Agent;

/// A model tier (docs/computers.md, Models): an agent's `agent.json` names
/// it, and Hermes sends its model's id (`ModelIds`) through the model
/// intercept, saying its tier (`TIER_HEADER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Cheap,
    Medium,
    High,
}

/// New agents run on the cheap tier (GLM-5.3 Flash, DeepSeek V4 Flash its
/// fallback; Paul, 2026-10-09).
pub const DEFAULT_TIER: Tier = Tier::Cheap;

impl Tier {
    /// An agent fragment's `agent.json` names its tier (`{"tier": …}`);
    /// anything else is the default tier. The high tier is off until
    /// Cloudflare raises its limit (decision 23), so it falls back to the default
    /// unless `high_on`.
    pub fn of(agent_json: Option<&[u8]>, high_on: bool) -> Tier {
        let tier = agent_json.and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok()).and_then(|v| v["tier"].as_str().map(str::to_string));
        match tier.as_deref() {
            Some("cheap") => Tier::Cheap,
            Some("medium") => Tier::Medium,
            Some("high") if high_on => Tier::High,
            _ => DEFAULT_TIER,
        }
    }

    /// An agent's tier as the platform answered for its fragment's
    /// `agent.json`: the file's (`of`), or the default tier when it has none
    /// (403, 404); `None` when the platform did not answer for it.
    pub fn read(answer: &Result<bytes::Bytes, fragment_bridge::api::ApiError>, high_on: bool) -> Option<Tier> {
        match answer {
            Ok(b) => Some(Tier::of(Some(b), high_on)),
            Err(e) if e.gone() => Some(Tier::of(None, high_on)),
            Err(_) => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Cheap => "cheap",
            Tier::Medium => "medium",
            Tier::High => "high",
        }
    }
}

/// A model of a provider an agent's owner connected, which their account
/// there pays for: its `agent.json`'s `model: {provider, id}` beside its
/// `tier` (docs/computers.md, "An agent's own model").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnModel {
    pub provider: String,
    pub id: String,
}

/// A model base URL the platform names, at most.
pub const MODEL_BASE_MAX_BYTES: usize = 512;

impl OwnModel {
    /// The model `agent.json` names, if it names one well: a provider's
    /// name (`[a-z0-9-]`, 1 to 32) and a model's id (`model_id_ok`).
    pub fn of(agent_json: &[u8]) -> Option<OwnModel> {
        let v: serde_json::Value = serde_json::from_slice(agent_json).ok()?;
        let (provider, id) = (v["model"]["provider"].as_str()?, v["model"]["id"].as_str()?);
        let provider_ok = (1..=32).contains(&provider.len()) && provider.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        (provider_ok && model_id_ok(id)).then(|| OwnModel { provider: provider.into(), id: id.into() })
    }
}

/// How hard an agent's model thinks before it answers (Paul, 2026-10-22:
/// "expose the reasoning level in the model picker … high is a good
/// default"): its `agent.json`'s `reasoning`, beside its `tier` and
/// `model`, which its owner picks in Settings' Agents. Its profile says it
/// as `agent.reasoning_effort`, which Hermes reads from the profile's
/// config at each turn (v0.21.7's `resolve_reasoning_config`) and sends as
/// each call's `reasoning_effort`. Only the efforts the platform's model
/// route passes on (`fragment_core::models::EFFORTS`, `low` and `high`): it
/// takes any other as `low`, as GLM takes `none`, `minimal`, `medium` and
/// `xhigh` for its dearest, `max`. Left unset, Hermes would send its custom
/// provider's default, `medium`, which the route took as `low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reasoning {
    Low,
    High,
}

/// What an agent whose `agent.json` names no reasoning (or one not offered)
/// thinks at.
pub const DEFAULT_REASONING: Reasoning = Reasoning::High;

impl Reasoning {
    /// The reasoning `agent.json` names (`{"reasoning": "low"}`), else the
    /// default: none read, no JSON, none named, or one not offered.
    pub fn of(agent_json: Option<&[u8]>) -> Reasoning {
        let v = agent_json.and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok());
        match v.as_ref().and_then(|v| v["reasoning"].as_str()) {
            Some("low") => Reasoning::Low,
            Some("high") => Reasoning::High,
            _ => DEFAULT_REASONING,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Reasoning::Low => "low",
            Reasoning::High => "high",
        }
    }
}

/// What an agent's main model is: a tier through the platform's model
/// route, or its own (`OwnModel`), sent to its provider's base URL with
/// the provider's placeholder as its key, which the computer's swap
/// replaces with its owner's key on the way out (the guest never holds it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainModel {
    Tier(Tier),
    Own { provider: String, base: String, id: String, placeholder: String },
}

impl From<Tier> for MainModel {
    fn from(t: Tier) -> MainModel {
        MainModel::Tier(t)
    }
}

impl MainModel {
    /// The model `agent` runs on: `own` where its credentials now hold that
    /// provider's placeholder with a model base (its owner connected it and
    /// did not narrow the agent from it), else `tier`; and, when it names
    /// its own and that is not used, why.
    pub fn of(agent: &Agent, tier: Tier, own: Option<&OwnModel>) -> (MainModel, Option<String>) {
        let Some(own) = own else { return (MainModel::Tier(tier), None) };
        let Some(c) = agent.credentials.iter().find(|c| c.provider == own.provider) else {
            return (MainModel::Tier(tier), Some(format!("its owner has not connected {} (or narrowed the agent from it)", own.provider)));
        };
        let base = c.model_base.as_deref().map(|b| b.trim_end_matches('/')).unwrap_or_default();
        let base_ok = (base.starts_with("https://") || base.starts_with("http://")) && base.len() <= MODEL_BASE_MAX_BYTES && base.bytes().all(|b| (0x21..=0x7e).contains(&b) && b != b'"' && b != b'\\');
        if !base_ok {
            return (MainModel::Tier(tier), Some(format!("{} serves no models here", own.provider)));
        }
        (MainModel::Own { provider: own.provider.clone(), base: base.into(), id: own.id.clone(), placeholder: c.placeholder.clone() }, None)
    }

    /// How events name it: a tier, or `<provider>:<id>`.
    pub fn name(&self) -> String {
        match self {
            MainModel::Tier(t) => t.name().into(),
            MainModel::Own { provider, id, .. } => format!("{provider}:{id}"),
        }
    }
}

/// The platform's model route as the guest reaches it: the intercept's base
/// URL (`FRAGMENT_MODEL`), and its models' ids by its names as the
/// computer's view names them now (`GET /api/computer`'s `models`;
/// docs/computers.md, "A model by its id"). A tier agent's profile names
/// its tier's model by its id, and its fallback by the fallback's (Paul,
/// 2026-10-16: "be able to run `/model` with hermes and see the real
/// model"), so Hermes knows the model it runs: `/model` shows it, its
/// prompt's `Model:` line names it, and its model-family guidance (GLM's,
/// DeepSeek's) is the model's. A change to the deployment's `models` is
/// read with the view, every 3 s, and rewrites each profile's config (no
/// new image, nothing restarted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub base: String,
    pub ids: ModelIds,
}

impl Route {
    /// The route at `base`, its ids the view's (`models`), or the route's
    /// own names, which it takes as well, from a platform that names none.
    pub fn new(base: &str, models: Option<&ModelIds>) -> Route {
        let names = || ModelIds { cheap: Tier::Cheap.name().into(), medium: Tier::Medium.name().into(), fallback: FALLBACK_MODEL.into(), vision: VISION_MODEL.into(), routes: None };
        Route { base: base.into(), ids: models.cloned().unwrap_or_else(names) }
    }

    /// The model a tier agent's profile names: its tier's id (`high`, off
    /// and on no slot, by its name).
    pub fn tier(&self, tier: Tier) -> &str {
        match tier {
            Tier::Cheap => &self.ids.cheap,
            Tier::Medium => &self.ids.medium,
            Tier::High => Tier::High.name(),
        }
    }

    /// The model a tier agent's stall falls back to: its route's fallback
    /// by its id (Private's private, Standard's Cloudflare), or, from a
    /// platform that names no routes, the deployment's fallback. None when
    /// that is the tier's own model (#527): an id is one model at one place
    /// (`fragment_core::models::Models::parse`), so the switch would only
    /// ask the same server again.
    pub fn fallback(&self, tier: Tier) -> Option<&str> {
        let routed = self.ids.routes.as_ref().and_then(|r| match tier {
            Tier::Cheap => Some(&r.cheap.fallback),
            Tier::Medium => Some(&r.medium.fallback),
            Tier::High => None,
        });
        Some(routed.unwrap_or(&self.ids.fallback).as_str()).filter(|id| *id != self.tier(tier))
    }
}

/// The header a tier agent's calls say its tier in
/// (`fragment_core::models::TIER_HEADER`): of two slots running the id it
/// names (both tiers on one model), its tier's is the call's.
pub const TIER_HEADER: &str = "x-fragment-tier";
/// The model route's name for the deployment's vision model
/// (`fragment_core::models::VISION`): Hermes' auxiliary vision names it, by
/// this name and not its model's id. A deployment may run vision on a
/// tier's model (every preview: GLM-5.3 Flash), and an id names that
/// tier's slot, whose call falls back to the fallback model, which may read
/// no images (DeepSeek V4 Flash); `vision` has no fallback.
pub const VISION_MODEL: &str = "vision";
/// The model route's name for transcription
/// (`fragment_core::transcribe::WHISPER`): Hermes' speech-to-text names it.
pub const TRANSCRIBE_MODEL: &str = "whisper";
/// The model route's name for the deployment's fallback model
/// (`fragment_core::models::FALLBACK`): a tier agent's `fallback_providers`
/// names its id (`ModelIds::fallback`), or this name when the platform names
/// none.
pub const FALLBACK_MODEL: &str = "fallback";
/// How long a tier agent's call may stream nothing before Hermes kills it
/// (its `providers.<id>.models.<model>.stale_timeout_seconds`; Paul,
/// 2026-10-09): a stalled call is tried once more (`HERMES_STREAM_RETRIES`,
/// `gateway_env`), then the agent switches to `fallback_providers` (its
/// `agent.api_max_retries: 1`), about 40 s after the stall began. GLM-5.3
/// Flash's first bytes are 1 to 5 s; DeepSeek's were 11 to 61 s when
/// Workers AI was short of it.
pub const STALE_TIMEOUT_S: u32 = 20;
/// Hermes' in-place retries of a stream that dropped or went stale
/// (`HERMES_STREAM_RETRIES`, its default 2): one, so a stall reaches the
/// fallback after two stale timeouts.
pub const STREAM_RETRIES: u32 = 1;

fn q(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// How long Hermes waits on a card it asks (a `clarify` with choices, or
/// one of the few approvals it still asks: `managed_config`): the bridge's
/// prompt lifetime, so a card and Hermes' wait on it expire together.
pub const CARD_TIMEOUT_S: u64 = fragment_bridge::limits::PROMPT_TTL_MS_DEFAULT / 1000;

/// The card timeout a boot uses, in seconds: `setting`
/// (`HERMES_BOOT_CARD_TIMEOUT_S`, a test's) within the bounds the bridge
/// holds a prompt's life to, else `CARD_TIMEOUT_S`. The bridge is given the
/// same (`BRIDGE_PROMPT_TTL_MS`), so a card and Hermes' wait on it still
/// expire together.
pub fn card_timeout_s(setting: Option<&str>) -> u64 {
    use fragment_bridge::limits::{PROMPT_TTL_MS_MAX, PROMPT_TTL_MS_MIN};
    setting.and_then(|v| v.trim().parse::<u64>().ok()).map_or(CARD_TIMEOUT_S, |s| s.clamp(PROMPT_TTL_MS_MIN / 1000, PROMPT_TTL_MS_MAX / 1000))
}

/// What every agent is told of the people it talks to, in its system
/// prompt on every turn in our chats (Hermes' `platform_hints.relay`,
/// appended to the relay's hint, which is empty): they are not technical,
/// so it asks them about outcomes in plain words, never about a command,
/// a file or code (Paul, 2026-10-14). The `fragment` skill's first page
/// (`computer.md`, "Asking your owner") says the same at more length.
pub const ASKING: &str = "You are in a Fragment chat, and its people are not technical. Never ask them to approve a command, a file, a path or code: do the technical work yourself, and tell them what changed in plain words. Ask only about outcomes that are theirs to decide, in plain words, with the clarify tool and its choices (its card is your owner's to answer): before you make something of theirs public or share it with someone new, send an email or a message in their name, delete something of theirs, or spend their money, unless they asked for exactly that in this chat.";

/// What every agent is told of the files people attach in our chats, beside
/// `ASKING` on every turn (2026-10-22: nothing told it an attachment is a
/// file it can use). Hermes' relay adapter downloads each attachment before
/// the turn into the scratch it moves to the profile's cache
/// (`relay_media_<random>.<ext>`; Hermes prunes what sits a day unused:
/// its `SCRATCH_MAX_IDLE_HOURS`, 24), and the turn's text names that path
/// only in its note before the message: an image's as `vision_analyze`'s
/// `image_url:`, a document's as where it is "saved at". Its browser tools
/// have no upload: a site's upload button opens the file dialog on its
/// desktop, where `computer_use` types the path. The `fragment` skill's
/// first page (`computer.md`, "Files your owner sends you") says the same
/// at more length.
pub const FILES: &str = "A file someone attaches in this chat is already on your computer: the note before their message names its path (for an image, after image_url:). It is cleared after a day, so copy it into your work to keep it. To put it on a website, click the site's upload button with your browser tools, then type its full path into the file dialog on your desktop with computer_use and press Enter.";

/// What every agent is told of the files it sends in our chats, after
/// `FILES` on every turn (Paul, 2026-10-23: asked for a PDF in the chat,
/// his agent named a path on its computer, then emailed it, and attached
/// it only when told to). Hermes tells every platform it knows that a
/// reply's `MEDIA:` tag sends a file (its `_MEDIA_NATIVE` hint); `relay`
/// has no hint of Hermes', so this is ours. Hermes delivers a tag only
/// outside code and quotes (its `_mask_media_scan_text`), and the bridge
/// posts each file as the reply's attachment (docs/bridge.md). The
/// `fragment` skill's first page (`computer.md`, "Files you send") says
/// the same at more length.
pub const SENDING: &str = "To give someone in this chat a file (a PDF, an image, a document, a spreadsheet, audio or video), put MEDIA: and its absolute path on a line of its own in your reply, outside code and quotes, for example MEDIA:/data/work/report.pdf. It arrives in this chat as an attachment they can open and download. Attach it whenever they ask for a file here, rather than emailing it or naming a path on your computer.";

/// What every agent is told of SimpleX, after `SENDING` on every turn
/// (Paul, 2026-10-26: asked over SimpleX to send its voice reply there, his
/// agent found the bridge's SimpleX core and began setting up a SimpleX of
/// its own): the rule, here, where its system prompt holds it, at a few
/// tokens a turn. Which chat is its owner's SimpleX is that chat's own to
/// say, as its topic (the bridge's relay wire.rs, `SIMPLEX_TOPIC`), and
/// where its owner's home is, its profile's (`told_home`).
pub const SIMPLEX: &str = "Fragment carries your owner's SimpleX for you, with any file you attach: never install, run or set up SimpleX, even when asked to send something there.";

/// The relay's hint, appended to every agent's system prompt on every turn
/// in our chats (Hermes' `platform_hints.relay.append`): how it asks
/// (`ASKING`), then the files it is sent (`FILES`), then how it sends one
/// (`SENDING`), then SimpleX's rule (`SIMPLEX`).
pub fn relay_hint() -> String {
    format!("{ASKING}\n\n{FILES}\n\n{SENDING}\n\n{SIMPLEX}")
}

/// The managed overlay: how every profile streams, shows progress, asks
/// (nothing technical; a card waits `card_timeout_s`), when its desktop is
/// idle (`screen_idle_ms`), and what it never runs.
pub fn managed_config(disabled_plugins: &[String], card_timeout_s: u64, screen_idle_ms: u64) -> String {
    let mut y = String::new();
    y.push_str("# Written by hermes-boot at every boot (images/hermes): Hermes' managed overlay,\n");
    y.push_str("# merged over each profile's own config. Hand edits are lost.\n");
    // One session per chat, shared by everyone in it; each message names its writer.
    y.push_str("group_sessions_per_user: false\n");
    // One gateway for every profile (one profile per agent), said outright.
    y.push_str("gateway:\n  multiplex_profiles: true\n");
    // Hermes' own first-contact profile-building flow is off: an agent's first
    // run is the shell's (decision 10).
    y.push_str("onboarding:\n  profile_build: \"off\"\n");
    // A reply streams as Relay `draft` frames (the chat's drafts); its final is one send.
    y.push_str("streaming:\n  enabled: true\n  transport: \"draft\"\n");
    // The bridge hands one message of a chat at a time; tool progress is one growing
    // message, each line a step. No interim messages: the model's text beside a tool
    // call is no message of its own (Hermes' default for a platform it has no tier
    // for, as `relay`, is to send each as one). Hermes reads `display` for a turn
    // from the profile's config with this overlay merged over it (its
    // `_load_gateway_config`, under the profile's scope). A step's preview is as
    // long as the bridge keeps a step's arguments (Hermes cuts it at 40 when
    // unset: its `_preview_cap`).
    y.push_str(&format!(
        "display:\n  busy_input_mode: \"queue\"\n  tool_progress: \"all\"\n  tool_progress_grouping: \"accumulate\"\n  tool_preview_length: {}\n  long_running_notifications: false\n  interim_assistant_messages: false\n",
        fragment_bridge::limits::STEP_ARGS_MAX_CHARS
    ));
    y.push_str("platforms:\n  relay:\n    gateway_restart_notification: false\n");
    // No technical approvals (Paul, 2026-10-14: its people are not technical,
    // and a question about a command makes no sense to them; finite-mono ran
    // Hermes so). `off` is Hermes' own switch past its approval gate: a
    // terminal command it flags, an execute_code script, a computer_use
    // action, a write to an SSH config, and a plugin's rule all run unasked,
    // with no smart guardian. Its hard floors come before the gate and
    // stand (docs/computers.md, "Approvals in our Hermes image"). What still
    // asks (a card filling a saved payment card) waits as long as the
    // bridge's prompt does. Slash confirmations stay off: `/new` from the
    // menu asks nothing.
    y.push_str(&format!("approvals:\n  mode: \"off\"\n  timeout: {card_timeout_s}\n  destructive_slash_confirm: false\n"));
    // Hermes' own cron is off: an agent's routines are scheduled records on
    // its own agent fragment (decision 38: `fragment schedule`, which the
    // platform skill's page teaches; the image's `hermes` refuses `cron`,
    // images/hermes/Dockerfile). A clarify's card waits as long as the bridge's prompt
    // does, as an approval's (Hermes' default is an hour, the bridge's too).
    y.push_str(&format!("agent:\n  disabled_toolsets: [\"cronjob\"]\n  clarify_timeout: {card_timeout_s}\n"));
    // The agent asks its people about outcomes alone, in plain words
    // (`ASKING`), and knows the files they attach are on its computer
    // (`FILES`).
    y.push_str(&format!("platform_hints:\n  relay:\n    append: {}\n", q(&relay_hint())));
    // A voice memo's transcript is no message of its own: the agent hears it
    // and answers once (one reply a turn, #160). The gateway reads this from
    // its own home's config, with this overlay merged over it; each profile
    // transcribes through the route (`profile_config`, `stt`).
    y.push_str("stt:\n  echo_transcripts: false\n");
    // Hermes' remote model catalogs are off: every profile's model is the
    // platform's route, and they serve only its `/model` picker (which a
    // person never reaches: the bridge keeps a leading `/` from reading as a
    // command) and OpenRouter's and Nous' routes. On, the gateway fetched
    // them from four hosts 30 s after it started and every 20 minutes, and
    // rewrote four caches in its home while the computer was held
    // (docs/durable-computers.md, "What changes under the hold").
    y.push_str("model_catalog:\n  enabled: false\n");
    // Each agent's desktop starts for its screen's first viewer (the
    // bridge's `screen-start`), or at the agent's first computer_use or
    // browser call, never at boot (measured: about 300 MiB more once it
    // runs). Its browser, on that desktop, is each profile's own config's
    // (`profile_config`). Its idle stop is the boot's (desktop.rs: Hermes
    // stops an idle desktop only from its TUI's gateway), said here too so
    // Hermes' own watcher, were it to run, would agree.
    let minutes = screen_idle_ms as f64 / 60_000.0;
    y.push_str(&format!("bot_desktop:\n  auto_start: true\n  idle_stop_minutes: {minutes}\n"));
    // Lazy installs are off (upstream's image turns them on): they fetch
    // Hermes' own optional backends (providers, platforms, speech) into its
    // home, which a computer configures none of (its model is the platform's
    // route, its chat the relay). What our agents use is in the image (Edge's
    // speech for text_to_speech: images/hermes/Dockerfile). Software the
    // agent wants is its terminal's to install.
    // A write to a project's `AGENTS.md`, `CLAUDE.md`, `SOUL.md`,
    // `.cursorrules` or `.hermes/` (a brain's guide, an app's notes, an
    // agent fragment synced to its work) asks no one: Hermes asks for these
    // whatever `approvals.mode` says, and they are the agent's own files to
    // keep.
    y.push_str("security:\n  allow_lazy_installs: false\n  protected_instruction_files: false\n");
    if !disabled_plugins.is_empty() {
        // Messaging platforms this computer never serves (it is reached through its
        // bridge) and the dashboard's auth providers: the gateway imports every one
        // it is not told to skip (spike S3b: 1.4 s).
        y.push_str("plugins:\n  disabled:\n");
        for p in disabled_plugins {
            y.push_str(&format!("    - {}\n", q(p)));
        }
    }
    y
}

/// What tells an agent its name (`name`, its agent fragment's title:
/// docs/api.md, "An agent's name and picture"), at every turn: its profile
/// config's `agent.system_prompt`, which Hermes reads from the profile's
/// config at each turn and adds to that turn's system prompt alone, never
/// to the prompt a session keeps (v0.21.7: its gateway's
/// `_get_system_prompt_for_channel`, the ephemeral system prompt). So a
/// rename reaches the agent's next turn in every chat, its Bot Chat's
/// lasting session too, nothing restarted. Nothing else it reads names it
/// (its `SOUL.md` is its job; Bot Mode tells it its `@handle` alone), and
/// what an earlier message or its memory calls it may be from before a
/// rename. Hermes expands `${VAR}` in a config's strings from its
/// environment, so a `${` in a name is broken apart (`$ {`).
pub fn told_name(name: &str) -> String {
    let name = name.replace("${", "$ {");
    format!("Your name is {name}: your agent fragment's title, as it is now. If an earlier message or your memory calls you something else, you have been renamed since, and {name} is your name.")
}

/// What tells an agent where its owner's home is (docs/api.md, "An agent's
/// home": its `agent.json`'s `home`), beside its name, at every turn: the
/// chat by its label (the session context names a chat so: `group: <label>`),
/// and that a routine scheduled without `--chat` runs there. A change is its
/// next turn's, as a rename is, nothing restarted. Hermes' own home channel
/// is none of it: v0.21.7's multiplexed gateway reads a relay home only from
/// its launch profile, and nothing an agent's tools do delivers there
/// (docs/hermes-relay.md, "Hermes' settings").
pub fn told_home(agent_fragment: &str, home: Home) -> String {
    let label = fragment_bridge::api::label(agent_fragment);
    let at = match home {
        Home::Chat => format!("your chat {label}-chat"),
        Home::Simplex => format!("their SimpleX, your chat {}, which reaches their phone", fragment_bridge::api::simplex_label(agent_fragment)),
    };
    format!("Your owner's home is {at}: a routine you schedule without --chat (a reminder, a check to report on) runs there.")
}

/// An agent's `agent.system_prompt`: its name, then its owner's home.
pub fn told(name: &str, agent_fragment: &str, home: Home) -> String {
    format!("{} {}", told_name(name), told_home(agent_fragment, home))
}

/// An agent's profile config: its name and its owner's home (`told`), its model, through
/// the model intercept, as that agent (`x-fragment-agent` on every call,
/// the main model's and the auxiliary ones'), a tier's by its model's id (`ModelIds`), saying its
/// tier (`TIER_HEADER`), how hard it thinks (`Reasoning`), and its vision model (the route's `vision`); and what
/// its terminal is given: who it is
/// (`PROFILE_ENV`) and every credential's environment variable the
/// deployment may give it (`credential_env`, the guest view's: Hermes reads
/// the list once per gateway, so it names them all, held now or not, and
/// each value is the profile's `.env`'s, read again at every turn). Hermes
/// never passes a name it keeps for its own providers' keys (left out of
/// the passthrough list, which otherwise warns at every turn)
/// (`PERPLEXITY_API_KEY`, `XAI_API_KEY`, `ELEVENLABS_API_KEY`, …: its
/// `_HERMES_PROVIDER_ENV_BLOCKLIST`), so the terminal also sources the
/// profile's credentials file (`credentials_sh`) as its shell starts.
#[allow(clippy::too_many_arguments)]
pub fn profile_config(agent: &Agent, name: &str, home: Home, main: &MainModel, reasoning: Reasoning, route: &Route, credential_env: &[String], credentials_file: &Path) -> String {
    let base = route.base.trim_end_matches('/');
    let mut y = String::new();
    y.push_str(&format!("# Written by hermes-boot for {} (model {}) at every boot, and when its model, its reasoning, its name or its home changes.\n", agent.fragment, main.name()));
    // its name and its home, at every turn; a tier's guidance, a text tier's one retry
    // and its reasoning (below) beside it, in its one `agent` mapping
    y.push_str(&format!("agent:\n  system_prompt: {}\n", q(&told(name, &agent.fragment, home))));
    // A tier agent's prompt is the one its tiers were validated on, named
    // `cheap` and `medium` (canary rounds 1 to 4): Hermes adds guidance for
    // a model's family by a substring of its name, and named by its id a
    // GLM, DeepSeek or Qwen gets two blocks those names never did, its
    // tool-use enforcement (`agent.tool_use_enforcement`, "auto": its
    // `TOOL_USE_ENFORCEMENT_MODELS`) and its execution discipline
    // (`agent.execution_guidance`, "auto": its `EXECUTION_GUIDANCE_MODELS`;
    // v0.21.7's `agent/system_prompt.py`, `_guidance_parts`). Given both,
    // canary round 5's GLM, on OpenRouter, ran every command through
    // `execute_code`. Both are off, so the id changes what `/model` and the
    // prompt's `Model:` line say, and no guidance: every model's own blocks
    // (finishing the job, parallel calls) stand. Revisit after launch. An
    // owner's own model was always named by its id: Hermes' choice for it
    // stands.
    if let MainModel::Tier(tier) = main {
        y.push_str("  tool_use_enforcement: false\n  execution_guidance: false\n");
        if matches!(tier, Tier::Cheap | Tier::Medium) {
            y.push_str("  api_max_retries: 1\n");
        }
    }
    // how hard it thinks, whatever its model: each call's `reasoning_effort`
    y.push_str(&format!("  reasoning_effort: {}\n", q(reasoning.name())));
    match main {
        MainModel::Tier(tier) => {
            let (provider, url) = match tier {
                // The high tier is Anthropic's Messages shape, passed through.
                Tier::High => ("anthropic", format!("{base}/anthropic")),
                Tier::Cheap | Tier::Medium => ("custom", format!("{base}/v1")),
            };
            y.push_str(&format!("model:\n  provider: {}\n  base_url: {}\n  default: {}\n", q(provider), q(&url), q(route.tier(*tier))));
            // The guest holds no credential: the intercept strips auth and adds its own.
            y.push_str("  api_key: \"fragment-model\"\n  context_length: 262144\n");
        }
        // Its owner's model, at its provider (OpenAI's chat-completions
        // shape): its key the provider's placeholder, which the swap
        // replaces on the way out. Its context length is Hermes' to find
        // (the provider's own metadata).
        MainModel::Own { base, id, placeholder, .. } => {
            y.push_str(&format!("model:\n  provider: \"custom\"\n  base_url: {}\n  default: {}\n", q(base), q(id)));
            y.push_str(&format!("  api_key: {}\n", q(placeholder)));
        }
    }
    // On every call, the main model's and the auxiliary ones': the route
    // reads it, and the swap sends no `x-fragment-` header to a provider.
    // A tier agent's says its tier too: an id both tiers run is its tier's.
    y.push_str(&format!("  default_headers:\n    x-fragment-agent: {}\n", q(&agent.fragment)));
    if let MainModel::Tier(tier) = main {
        y.push_str(&format!("    {TIER_HEADER}: {}\n", q(tier.name())));
    }
    // The managed skills and the platform skill's view (skills.rs), below
    // the profile's own `skills/`: an agent's own wins, and a managed one
    // over the platform's, which leaves the view for it.
    let dirs: Vec<String> = crate::skills::EXTERNAL_DIRS.iter().map(|d| q(d)).collect();
    y.push_str(&format!("skills:\n  external_dirs: [{}]\n", dirs.join(", ")));
    // Its eyes: Hermes' auxiliary vision (each computer_use screenshot, and
    // an image a person attaches, described in words for the main model) on
    // the route's `vision`, the deployment's vision model, whatever the
    // agent's tier (the medium tier's GLM-5.3 reads no images). Named
    // outright, Hermes routes every capture and attachment through it (its
    // image routing, `agent/image_routing.py`: in `auto`, an explicit
    // `auxiliary.vision` makes it text, which each capture asks through
    // `tools/vision_tools.py`'s `_native_tool_result_images`) and
    // sends it, as every
    // call to a custom endpoint, with `model.default_headers`: the agent's
    // `x-fragment-agent`, so the intercept meters it to the agent's owner.
    // Always OpenAI's shape, the high tier's agents' too.
    // Memory and skill reviews are on. The image plugin forks our external
    // skills into the active profile before writes; Hermes retains its guards.
    y.push_str(&format!(
        "plugins:\n  enabled: [fragment-skill-fork, fragment-deploy-wait, fragment-activity]\nauxiliary:\n  background_review: {{ enabled: true }}\n  vision:\n    provider: \"custom\"\n    base_url: {}\n    model: {}\n    api_key: \"fragment-model\"\n",
        q(&format!("{base}/v1")),
        q(VISION_MODEL)
    ));
    // Its ears (decision 9: a voice memo is one the agent transcribes
    // itself): Hermes' speech-to-text, which a voice note a person attaches
    // gets before its turn, on the route's `whisper` (Workers AI's Whisper),
    // OpenAI's transcription shape through the intercept, metered to the
    // agent's owner. Hermes' STT client takes a base URL and a key and sends
    // no header of ours, so its key names the agent (`agent:<name>`, which
    // the intercept reads and sends no further). No language: Whisper
    // detects it (Hermes' own default, `en`, mangles every other).
    y.push_str(&format!(
        "stt:\n  provider: \"openai\"\n  language: \"\"\n  openai:\n    base_url: {}\n    api_key: {}\n    model: {}\n",
        q(&format!("{base}/v1")),
        q(&format!("agent:{}", agent.fragment)),
        q(TRANSCRIBE_MODEL)
    ));
    // A text tier's stall (Paul, 2026-10-09: a time-based fallback "only if
    // time-based fallback is built into hermes"; it is): Hermes' own stale
    // detector kills a call that streams nothing for `STALE_TIMEOUT_S`
    // (this model's alone: `vision`, whose answers are whole and slower,
    // keeps Hermes' default), tries it once more (`STREAM_RETRIES`), then,
    // its one retry spent (`api_max_retries`, in `agent` above), switches to its
    // `fallback_providers`: its route's fallback, by its id (`Route::fallback`:
    // the deployment's fallback from a platform naming no routes), through
    // the intercept as the agent (its key names the agent; this call carries
    // no header of ours, so its tier is not said: the id names its slot). The next turn starts on its tier again (Hermes'
    // `restore_primary_runtime`). The route itself falls back when a model
    // answers 429 or a 5xx (cell/src/models.rs); this is for a call that
    // answers nothing. An owner's own model and the high tier have none,
    // and nor has a tier whose fallback is its own model (`Route::fallback`).
    if let MainModel::Tier(tier @ (Tier::Cheap | Tier::Medium)) = main {
        y.push_str(&format!("providers:\n  custom:\n    models:\n      {}:\n        stale_timeout_seconds: {STALE_TIMEOUT_S}\n", q(route.tier(*tier))));
        if let Some(fallback) = route.fallback(*tier) {
            y.push_str(&format!(
                "fallback_providers:\n  - provider: \"custom\"\n    base_url: {}\n    model: {}\n    api_key: {}\n",
                q(&format!("{base}/v1")),
                q(fallback),
                q(&format!("agent:{}", agent.fragment))
            ));
        }
    }
    // Stored selections, never inferred by Hermes from credential presence.
    // Only placeholders actually held by this agent count: the catalog's
    // credential_env also includes providers it cannot currently use.
    let vars = credential_vars(agent);
    let offered = |env: &str| vars.iter().any(|(name, _)| name == env);
    if offered("FIRECRAWL_API_KEY") {
        // Full-page extraction, unlike Perplexity's query-relevant snippets.
        y.push_str("web:\n  backend: \"firecrawl\"\n  search_backend: \"firecrawl\"\n  extract_backend: \"firecrawl\"\n");
    } else if offered("PERPLEXITY_API_KEY") {
        // Search remains usable on a deployment offering only Perplexity;
        // leave extraction's selection untouched.
        y.push_str("web:\n  search_backend: \"perplexity\"\n");
    }
    if offered("FAL_KEY") {
        y.push_str("image_gen:\n  provider: \"fal\"\n  model: \"fal-ai/flux-2/klein/9b\"\n");
    }
    if offered("ELEVENLABS_API_KEY") {
        y.push_str("tts:\n  provider: \"elevenlabs\"\n");
    }
    // XAI_API_KEY enables Hermes' separate x_search, not a web backend.
    // Its browser: Hermes' built-in browser tools (browser_navigate, …),
    // driving the image's own Chromium, headed, on the agent's desktop, so
    // the screen shows it, through agent-browser (in the image: its
    // Dockerfile). Left unset, Hermes picks Browser Use mode (one
    // browser_exec tool, its harness a core dependency of Hermes). Hermes
    // reads `browser` from the profile's own config file alone, never from
    // the managed overlay.
    y.push_str("browser:\n  headed: true\n  backend: \"off\"\n");
    if offered("BROWSER_USE_API_KEY") {
        // Without this Hermes auto-detects the offered key and sends the
        // built-in browser tools to the cloud. Cloud browsing is opt-in
        // through BU_NAME=remote browser-harness; the desktop stays local.
        y.push_str("  cloud_provider: \"local\"\n");
    }
    // Its terminal acts as the agent: the fragment CLI and the skills' helpers
    // read these from the profile's `.env` (`profile_env`), which Hermes passes
    // only to the commands of this profile's turns. Its shell's start files are
    // Hermes' own three, then the agent's credentials.
    let mut passed: Vec<String> = PROFILE_ENV.iter().map(|s| s.to_string()).collect();
    for e in credential_env.iter().filter(|e| env_name_ok(e) && !provider_env_blocked(e)) {
        if !passed.contains(e) {
            passed.push(e.clone());
        }
    }
    y.push_str(&format!("terminal:\n  env_passthrough: [{}]\n", passed.iter().map(|k| q(k)).collect::<Vec<_>>().join(", ")));
    let init = ["~/.profile", "~/.bash_profile", "~/.bashrc"].iter().map(|f| q(f)).chain([q(&credentials_file.display().to_string())]);
    y.push_str(&format!("  shell_init_files: [{}]\n", init.collect::<Vec<_>>().join(", ")));
    // Its commands run in the agent's work directory, which the computer
    // saves on its own (the seam: step 2 of docs/durable-computers.md), never
    // in Hermes' home (left unset, the gateway's own home, /data/hermes)
    y.push_str(&format!("  cwd: {}\n", q(&work_dir(&agent.fragment).display().to_string())));
    y
}

/// Hermes v0.21.7's `_HERMES_PROVIDER_ENV_BLOCKLIST` (v0.21.6's alike), read
/// from the pinned image's `tools/environments/local_env_policy.py`, plus its dynamic
/// `_is_hermes_internal_secret` rule (GHSA-rhgp-j443-p4rf). The Docker lane
/// checks the snapshot against the image, so a new pin cannot silently
/// change the policy. Only sandbox registration is filtered: native tools
/// still read the profile's `.env` through Hermes' secret scope, and the
/// terminal's shell still sources `credentials.sh`.
pub const PROVIDER_ENV_BLOCKLIST: &str = include_str!("provider-env-blocklist.txt");

fn provider_env_blocked(name: &str) -> bool {
    PROVIDER_ENV_BLOCKLIST.lines().any(|n| n == name)
        || (name.starts_with("AUXILIARY_") && (name.ends_with("_API_KEY") || name.ends_with("_BASE_URL")))
        || (name.starts_with("GATEWAY_RELAY_") && ["_SECRET", "_KEY", "_TOKEN"].iter().any(|suffix| name.ends_with(suffix)))
}

/// The flags Chromium needs in a container: Hermes' own
/// (`CHROMIUM_SANDBOX_BYPASS_ARGS`, its `tools/browser_tool_session.py`),
/// which it gives its browser and its desktop's Browser icon only as root
/// or where it sees Docker's marker (`/.dockerenv`). Cloudflare Containers
/// has neither that marker nor a `/dev/shm`, and Chromium without these
/// dies as it starts: no usable sandbox (no user namespaces), or no shared
/// memory (p5, 2026-10-05).
pub const CHROMIUM_FLAGS: [&str; 2] = ["--no-sandbox", "--disable-dev-shm-usage"];

/// The image's Chromium: a script that starts regular Chromium from its pinned base
/// with `CHROMIUM_FLAGS` whatever the runtime. Hermes is pointed at it
/// (`AGENT_BROWSER_EXECUTABLE_PATH`), so its browser and its desktop's
/// Browser icon (Hermes' `bot_desktop.browser.executable()`) both run it.
pub const CHROMIUM: &str = "/opt/fragment/bin/chromium";
/// Regular Chromium, already in the pinned upstream desktop base.
pub const SYSTEM_CHROMIUM: &str = "/usr/bin/chromium";

/// The script at `CHROMIUM`, written at image build: regular distro
/// Chromium, `full` (no headless shell: the full one runs
/// headed on a desktop and headless with none, as its caller asks), its
/// scratch the container's `/tmp` (`CHROMIUM_TMP`).
pub fn chromium_script(full: &Path) -> String {
    let s = full.display().to_string();
    assert!(full.is_absolute() && !s.contains('\''), "a browser's path, absolute, quotable: {s}");
    let flags = CHROMIUM_FLAGS.join(" ");
    format!("#!/bin/sh\n# Regular Chromium from the pinned desktop image, with its container flags.\n# --test-type suppresses the unsupported --no-sandbox infobar, as Hermes' dock does.\nexport TMPDIR={CHROMIUM_TMP}\n# Distro Chromium 154 puts a headless throwaway profile under XDG_CONFIG_HOME,\n# unlike the testing build's TMPDIR. Keep it out of every saved home too.\nfor arg in \"$@\"; do\n  case \"$arg\" in --headless|--headless=*) export XDG_CONFIG_HOME=\"/tmp/fragment-chromium-$(id -u)\" ;; esac\ndone\nexec '{s}' {flags} --test-type \"$@\"\n")
}

/// Chromium's scratch: the container's own `/tmp`, which no save keeps.
/// Hermes points `TMPDIR` under `/data`: an agent's terminal at its
/// profile's scratch (its work's `tmp_dir`, through a link), Hermes' browser
/// tool at the gateway's own (`/data/hermes/cache/scratch`).
/// Chromium keeps there the shared memory `--disable-dev-shm-usage` moves
/// out of `/dev/shm`; a headless call also gets an XDG_CONFIG_HOME under
/// /tmp, where distro Chromium 154 puts its throwaway profile when no
/// user-data directory is named (the testing build 145 used TMPDIR).
/// This is throwaway state any save would carry, and under the
/// gateway's scratch a hold while one runs would find its databases under
/// Hermes' home, locked.
pub const CHROMIUM_TMP: &str = "/tmp";

/// Where `SYSTEM_CHROMIUM` takes the interception CA: Debian Chromium's
/// managed policy directory, rather than Chrome for Testing's. The CA
/// appended to the system bundle is curl's, Python's and Node's; the
/// browser reads `CACertificates` from its own policy directory.
/// Without it, every page whose host is
/// intercepted on HTTPS (a provider's the swap catches: docs/computers.md,
/// "Connections and operator keys") is an interstitial in its browser.
pub const CHROMIUM_CA_POLICY: &str = "/etc/chromium/policies/managed/fragment-ca.json";

/// The managed policy that trusts each certificate of `pem`: a PEM's body
/// is its DER in base64, as the policy names one. `None` for no
/// certificate.
pub fn chromium_ca_policy(pem: &str) -> Option<String> {
    let mut certs = vec![];
    let mut body: Option<String> = None;
    for line in pem.lines().map(str::trim) {
        match line {
            "-----BEGIN CERTIFICATE-----" => body = Some(String::new()),
            "-----END CERTIFICATE-----" => certs.extend(body.take().filter(|b| !b.is_empty())),
            _ => {
                if let Some(b) = body.as_mut() {
                    b.push_str(line);
                }
            }
        }
    }
    (!certs.is_empty()).then(|| serde_json::json!({ "CACertificates": certs }).to_string())
}

/// What Hermes would otherwise decide by guessing whether it runs in a
/// container, pinned in the boot's environment (and so the gateway's, its
/// agents' terminals' and the desktop's) the same on every runtime. Its
/// guess (`hermes_platform/host/runtime.py: is_container()`) is Docker's
/// marker `/.dockerenv`, Podman's, Kubernetes', or a runtime's name in
/// `/proc/1/cgroup` or the root mount: Docker gives every container the
/// marker, Cloudflare Containers likely none of them, so there Hermes would
/// act as on a host. Each pin is what Hermes does in a container, as the
/// Docker rung has always run it (docs/computers.md, "Hermes' container
/// guesses", has every guess and what decides it):
///
/// - `TERMINAL_HOME_MODE=profile`: each agent's terminal, `execute_code`
///   and file tools' `~` have the agent's own `HOME`, its profile's `home`
///   (one of `PROFILE_DIRS`), a link to its home in its work (`home_dir`).
///   Taken for a host, every agent's would be the gateway's own,
///   `/data/hermes`: one `~` for all the computer's agents, among Hermes'
///   own files (its `.env`, `config.yaml`).
/// - `HERMES_SKIP_CHMOD=1`: Hermes leaves the modes of its home's
///   directories and files as the image and the boot make them. Taken for
///   a host, it makes them owner-only (0700, 0600) at each start; the
///   computer has one user and root, and holds no secret, so that guards
///   nothing here.
///
/// The third guess the image's paths reach, Chromium's flags, is pinned by
/// the image's Chromium (`CHROMIUM`).
pub const RUNTIME_ENV: [(&str, &str); 2] = [("TERMINAL_HOME_MODE", "profile"), ("HERMES_SKIP_CHMOD", "1")];

/// What the computer keeps as its guest's tools' work, saved as a record
/// of its own (docs/computers.md, "Data and the restore gate").
pub const WORK: &str = "/data/work";

/// An agent's work directory: its terminal's cwd, its home (`home_dir`),
/// its temp files (`tmp_dir`) and its browser's profile (`BROWSER_PROFILE`).
pub fn work_dir(agent_fragment: &str) -> PathBuf {
    Path::new(WORK).join(wire::profile(agent_fragment))
}

/// An agent's home: its terminal's `HOME` and its file tools' `~`, which
/// Hermes takes to be its profile's `home` (`TERMINAL_HOME_MODE=profile`,
/// `RUNTIME_ENV`), there a link to this. What its tools write under `~` (a
/// browser's default profile and its databases, a CLI's login) is its work,
/// saved with it, never Hermes' home (Paul, 2026-10-07).
pub fn home_dir(agent_fragment: &str) -> PathBuf {
    work_dir(agent_fragment).join("home")
}

/// An agent's temp files: what Hermes points `TMPDIR`, `TMP` and
/// `TEMP` at for each process it runs with the profile's home (its
/// terminal's commands, foreground or background), its profile's
/// `PROFILE_SCRATCH`, there a link to this. So a tool's temp file is its
/// work, saved with it, never Hermes' home.
///
/// Why a link, and not a `TMPDIR` of the image's: Hermes derives each
/// child's temp directory from the home it runs it under
/// (`hermes_constants.apply_subprocess_home_env`, then
/// `apply_scratch_tmp_env`, re-pointing a value it set itself, known by its
/// `HERMES_SCRATCH_DIR` marker), so its own path is the one place every one
/// of its ways of starting a command reads. A profile cannot name one:
/// `terminal.env_passthrough` reads no `TMPDIR` from a profile's `.env`
/// (Hermes keeps it process-wide: `agent/secret_scope.py`,
/// `_GLOBAL_ENV_EXACT`), and an export in `shell_init_files` reaches only
/// the terminal's snapshot, never a background process (a `bash -lic` of
/// the gateway's environment). And the gateway's own `TMPDIR` stays
/// Hermes': one Hermes did not set is passed to every child as it is, so
/// all agents would share it. One way misses: execute_code's scripts get
/// the gateway's own (docs/technical-debt-ledger.md, "An agent's
/// execute_code makes its temp files in Hermes' home").
pub fn tmp_dir(agent_fragment: &str) -> PathBuf {
    work_dir(agent_fragment).join("tmp")
}

/// A profile's `home`, as Hermes names it (one of `PROFILE_DIRS`).
pub const PROFILE_HOME: &str = "home";

/// A profile's scratch directory, relative to it, as Hermes names it
/// (`hermes_constants.get_scratch_dir`: `<home>/cache/scratch`).
pub const PROFILE_SCRATCH: &str = "cache/scratch";

/// What a profile's `home` or scratch is set aside as (after its own name)
/// when it has something in it from before it was the agent's work: a hard
/// cut, so nothing of it is carried over, and nothing deleted.
pub const SET_ASIDE: &str = ".before-work";

/// What `link_into_work` found and did.
#[derive(Debug, PartialEq, Eq)]
pub enum WorkLink {
    /// Already the link.
    Kept,
    /// Linked now: a fresh profile, an empty directory, or a link elsewhere.
    Linked,
    /// A directory with something in it, set aside unmoved (`SET_ASIDE`
    /// after its name, numbered when that is taken), then linked.
    SetAside(PathBuf),
}

/// Makes `profile`'s `at` (`PROFILE_HOME`, `PROFILE_SCRATCH`) a link to
/// `to` (made if missing, as is the link's parent), whatever it was. The
/// caller gives them to the agent's user.
pub fn link_into_work(profile: &Path, at: &str, to: &Path) -> std::io::Result<WorkLink> {
    std::fs::create_dir_all(to)?;
    let link = profile.join(at);
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let found = match std::fs::symlink_metadata(&link) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => WorkLink::Linked,
        Err(e) => return Err(e),
        Ok(m) if m.file_type().is_symlink() => {
            if std::fs::read_link(&link)? == to {
                return Ok(WorkLink::Kept);
            }
            std::fs::remove_file(&link)?;
            WorkLink::Linked
        }
        Ok(m) if m.is_dir() && std::fs::read_dir(&link)?.next().is_none() => {
            std::fs::remove_dir(&link)?;
            WorkLink::Linked
        }
        Ok(_) => {
            // bounded: 100 names
            let aside = (0..100)
                .map(|n| profile.join(if n == 0 { format!("{at}{SET_ASIDE}") } else { format!("{at}{SET_ASIDE}-{n}") }))
                .find(|p| std::fs::symlink_metadata(p).is_err())
                .ok_or_else(|| std::io::Error::other(format!("no name left to set {} aside as", link.display())))?;
            std::fs::rename(&link, &aside)?;
            WorkLink::SetAside(aside)
        }
    };
    std::os::unix::fs::symlink(to, &link)?;
    Ok(found)
}

/// Where Hermes keeps a profile's desktop browser's profile (its
/// `tools/bot_desktop/browser.py`: `<profile>/bot-desktop/browser-profile`),
/// relative to the profile: a link into the agent's work directory, so the
/// cookies and history its tools make are its work, saved with it.
pub const BROWSER_PROFILE: &str = "bot-desktop/browser-profile";

/// What a profile's terminal knows of the agent it runs (cli/GUIDE.md, "As
/// an agent"): the agent fragment its computer signs as, and the person it
/// acts for, its owner. Hermes passes these from the profile's `.env` to its
/// terminal's commands (`terminal.env_passthrough`, scoped per profile under
/// one gateway), so each agent's commands act as that agent alone.
pub const PROFILE_ENV: [&str; 2] = ["FRAGMENT_AS_AGENT", "FRAGMENT_FOR"];

/// The profile's credentials file, which its terminal's shell sources.
pub const CREDENTIALS_FILE: &str = "credentials.sh";

/// An environment variable's name a credential may be given in: upper
/// case, and not one of the profile's own.
fn env_name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit()
        && !name.starts_with("FRAGMENT_")
        && !name.starts_with("HERMES_")
}

/// A placeholder, as the platform makes one: `fcx_`/`fck_`, a provider and
/// a tag, nothing a line or a shell would read as more.
fn placeholder_ok(p: &str) -> bool {
    (p.starts_with("fcx_") || p.starts_with("fck_")) && p.len() <= 128 && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// The agent's credentials as `(name, placeholder)`, each in every
/// environment variable it names, in the platform's order; one the
/// platform sent out of shape is left out (and the name first given wins).
pub fn credential_vars(agent: &Agent) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for c in &agent.credentials {
        if !placeholder_ok(&c.placeholder) {
            continue;
        }
        for e in c.env.iter().filter(|e| env_name_ok(e)) {
            if !out.iter().any(|(n, _)| n == e) {
                out.push((e.clone(), c.placeholder.clone()));
            }
        }
    }
    out
}

/// A profile's `.env`: who the agent is, and its credentials, each a
/// placeholder (the computer holds no credential: the swap fills them on
/// the way to their providers).
pub fn profile_env(agent: &Agent) -> String {
    let line_safe = |v: &str| !v.is_empty() && v.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\'' && b != b'\\' && b != b'#');
    assert!(line_safe(&agent.fragment) && line_safe(&agent.owner), "an agent's fragment and owner are names: {agent:?}");
    let mut e = format!(
        "# The profile of {}; the computer holds no credential, only placeholders.\n{}={}\n{}={}\n",
        agent.fragment, PROFILE_ENV[0], agent.fragment, PROFILE_ENV[1], agent.owner
    );
    for (name, placeholder) in credential_vars(agent) {
        e.push_str(&format!("{name}={placeholder}\n"));
    }
    e
}

/// The profile's credentials for its terminal's shell (`CREDENTIALS_FILE`,
/// sourced as a session's shell starts): each exported under its name.
pub fn credentials_sh(agent: &Agent) -> String {
    let mut s = format!("# Written by hermes-boot: {}'s credentials, placeholders the computer's swap fills.\n", agent.fragment);
    for (name, placeholder) in credential_vars(agent) {
        s.push_str(&format!("export {name}='{placeholder}'\n"));
    }
    s
}

/// The default profile (the gateway's own, no agent's): it runs no turns.
pub fn default_config(model_base: &str) -> String {
    let base = model_base.trim_end_matches('/');
    format!("# Written by hermes-boot: the gateway's own profile runs no agent's turns.\nmodel:\n  provider: \"custom\"\n  base_url: {}\n  default: {}\n  api_key: \"fragment-model\"\n  context_length: 262144\n", q(&format!("{base}/v1")), q(DEFAULT_TIER.name()))
}

/// The gateway's Relay settings (Hermes' `gateway/relay/__init__.py`): with
/// the id and secret pinned it provisions nothing; the URL turns the relay
/// platform on.
pub fn gateway_env(listen: &str, gateway_id: &str, secret: &str) -> String {
    assert!(secret.len() >= 32, "a Relay secret of 32 characters or more");
    // RELAY_HOME_CHANNEL: Hermes asks each new session's first message to
    // `/sethome` (its `_hmwa_first_contact_notes`) unless this is set to
    // anything; v0.21.7 reads no relay home from it (none of its env steps
    // is relay's), and refuses `/sethome` for relay. An agent's home is the
    // platform's (its agent.json's `home`), told to it beside its name
    // (`told_home`): Hermes' own would reach nothing of ours, as its
    // multiplexed gateway reads a relay home from its launch profile alone,
    // and nothing an agent's tools do delivers there (docs/hermes-relay.md).
    // GATEWAY_MULTIPLEX_PROFILES: one gateway serves every profile. Said
    // outright, because left to its default Hermes stays standalone in an s6
    // container with no per-profile gateway slots, which this image has none
    // of by design.
    // HERMES_AUTO_CONTINUE_FRESHNESS: one second, so no message after a
    // restart is wrapped in Hermes' recovery notes (the boot's clean-exit
    // receipt already discards the turns a restart cut short, which the
    // bridge ends: docs/chat-records.md; the boot closes them in their
    // sessions, CLOSE_CUT_TURNS, and the bridge tells the next turn what was
    // cut instead, from the journal).
    // HERMES_GATEWAY_MAX_STARTS: 0, so Hermes' respawn-storm breaker is off.
    // It counts gateway starts in the home, which a save keeps, so six wakes
    // in two minutes would sleep the seventh 10-40 s before it answers; the
    // Computer DO already paces a computer's restarts.
    // HERMES_STREAM_RETRIES: `STREAM_RETRIES`, so a stalled call reaches its
    // fallback after two stale timeouts (`profile_config`).
    format!("GATEWAY_RELAY_URL=http://{listen}\nGATEWAY_RELAY_ID={gateway_id}\nGATEWAY_RELAY_SECRET={secret}\nHERMES_GATEWAY_BUSY_INPUT_MODE=queue\nHERMES_GATEWAY_NO_SUPERVISE=1\nGATEWAY_MULTIPLEX_PROFILES=true\nRELAY_HOME_CHANNEL=none\nHERMES_AUTO_CONTINUE_FRESHNESS=1\nHERMES_GATEWAY_MAX_STARTS=0\nHERMES_STREAM_RETRIES={STREAM_RETRIES}\n")
}

/// A profile's directory, under the Hermes home.
pub fn profile_dir(home: &Path, agent_fragment: &str) -> PathBuf {
    home.join("profiles").join(wire::profile(agent_fragment))
}

/// Closes each turn a restart cut, in Hermes' own sessions, before the
/// gateway starts (docs/durable-computers.md, P5), run by Hermes' Python as
/// the hermes user with each `state.db` to look in as its arguments.
///
/// Why: Hermes persists a turn's message as the turn starts and the rest as
/// it goes, so a turn the container's end cut leaves its session's tail open
/// (its message, or its tool calls, with no answer). v0.21.5 then joins the
/// next message to the open request (two user messages in a row are one:
/// `agent/agent_runtime_helpers.py`, `_merge_consecutive_users`; and
/// `get_messages_as_conversation(repair_alternation=True)` as the gateway
/// loads a transcript), and its model redoes the cut request (F10: seen on
/// the real image, `[paul] do the risky thing\n\n[paul] good morning`). The
/// bridge has already ended that turn as lost (P1), and tells the next turn
/// what was cut, from the journal.
///
/// How, through Hermes' own code and nothing else: the row Hermes itself
/// writes when a turn ends without an answer, its failed-turn boundary
/// (`agent/turn_failure_copy.py`: an assistant row, `display_kind`
/// `failed_turn`, its copy chosen by `failed_turn_notice` from the turn's
/// rows: "Some actions may already have run" when a tool call is among
/// them). Hermes writes it for a turn that fails in its process
/// (`agent/conversation_loop.py`, `_close_durable_failed_turn`;
/// `gateway/run_turn.py`, `_hmwa_close_failed_turn`, keyed on the durable
/// tail); a process that was killed never does, and neither its clean-exit
/// path (it only discards turn markers) nor its unclean one (it resumes the
/// turn under its old message id, the auto-continue kept off above) closes
/// it. No Relay frame does either: an inbound's fields (text, media,
/// context, reply, prompt response) never change how the session's tail is
/// read. So the boot writes the same row for the turns a restart cut:
/// every gateway session of the relay platform (`SessionDB.
/// list_gateway_sessions`, the newest session per chat) whose last message
/// is a user row, a tool result or a tool call, appended with
/// `SessionDB.append_message`. At boot every such tail is a cut turn: the
/// gateway that ran it is gone, and the bridge ended it. Idempotent: a
/// closed tail is an assistant row. Prints `{"sessions": n, "closed": n}`.
pub const CLOSE_CUT_TURNS: &str = r#"import json, sys
from pathlib import Path
from hermes_state import SessionDB
from agent.turn_failure_copy import FAILED_TURN_DISPLAY_KIND, failed_turn_notice
sessions = closed = 0
for p in sys.argv[1:]:
    db = SessionDB(Path(p))
    try:
        for s in db.list_gateway_sessions(platform="relay"):
            sessions += 1
            rows = [m for m in db.get_messages_as_conversation(s["id"]) if m.get("role") in ("user", "assistant", "tool")]
            if not rows or (rows[-1]["role"] == "assistant" and not rows[-1].get("tool_calls")):
                continue
            asked = max((i for i, m in enumerate(rows) if m["role"] == "user"), default=0)
            db.append_message(s["id"], "assistant", failed_turn_notice(rows[asked:]), display_kind=FAILED_TURN_DISPLAY_KIND)
            closed += 1
    finally:
        db.close()
print(json.dumps({"sessions": sessions, "closed": closed}))
"#;

/// What `CLOSE_CUT_TURNS` printed: the sessions it looked at and those it
/// closed (its last line), or why that is not its answer.
pub fn cut_turns_closed(out: &str) -> Result<(u64, u64), String> {
    let line = out.lines().rev().find(|l| !l.trim().is_empty()).ok_or("it printed nothing")?;
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| format!("{line:?}: {e}"))?;
    match (v["sessions"].as_u64(), v["closed"].as_u64()) {
        (Some(sessions), Some(closed)) if closed <= sessions => Ok((sessions, closed)),
        _ => Err(format!("not its answer: {line}")),
    }
}

/// The gateway's control socket (Hermes' `gateway/control_socket.py`):
/// `$HERMES_HOME/gateway.sock`, or the path its pointer file names when the
/// home's is too long for a socket. One JSON line in, one out, per
/// connection.
pub const CONTROL_SOCKET: &str = "gateway.sock";
pub const CONTROL_POINTER: &str = "gateway.sock.path";
/// The control protocol's version (`CONTROL_PROTOCOL_VERSION`).
pub const CONTROL_PROTOCOL: u64 = 1;
/// An answer is at most this many bytes (its `_MAX_RESPONSE_BYTES`).
pub const CONTROL_ANSWER_MAX_BYTES: usize = 512 * 1024;

/// One control request: `verb`, with no arguments.
pub fn control_request(verb: &str) -> String {
    assert!(!verb.is_empty() && verb.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'), "a control verb: {verb}");
    format!("{}\n", serde_json::json!({ "verb": verb, "id": 1, "protocol": CONTROL_PROTOCOL }))
}

/// A control answer's `result`, or why there is none (`ok: false` names
/// its error).
pub fn control_answer(line: &[u8]) -> Result<serde_json::Value, String> {
    if line.len() > CONTROL_ANSWER_MAX_BYTES {
        return Err(format!("an answer of {} bytes", line.len()));
    }
    let v: serde_json::Value = serde_json::from_slice(line).map_err(|e| format!("an answer that is not JSON: {e}"))?;
    if v["ok"] != true {
        return Err(v["error"].as_str().unwrap_or("refused, saying nothing").to_string());
    }
    match &v["result"] {
        r @ serde_json::Value::Object(_) => Ok(r.clone()),
        _ => Err("an answer with no result".into()),
    }
}

/// The directories a profile holds from its start (Hermes' `_PROFILE_DIRS`).
pub const PROFILE_DIRS: [&str; 9] = ["memories", "sessions", "skills", "skins", "logs", "plans", "workspace", "cron", "home"];

/// The plugins to disable: every bundled messaging platform and dashboard
/// auth provider (`<group>/<name>` under Hermes' `plugins/`).
pub fn lean_plugins(plugins_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for group in ["platforms", "dashboard_auth"] {
        let Ok(entries) = std::fs::read_dir(plugins_root.join(group)) else { continue };
        let mut names: Vec<String> = entries.filter_map(Result::ok).filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| n != "__pycache__" && !n.starts_with('.')).collect();
        names.sort();
        out.extend(names.into_iter().map(|n| format!("{group}/{n}")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The closer's answer is its last line, and nothing else is: a line
    /// Hermes logged before it is passed over, a missing or impossible count
    /// is no answer. (Run against the real Hermes in the bridge's
    /// tests/docker.rs.)
    #[test]
    fn the_closers_answer() {
        assert_eq!(cut_turns_closed("{\"sessions\": 2, \"closed\": 1}\n"), Ok((2, 1)));
        assert_eq!(cut_turns_closed("a warning Hermes logged\n{\"sessions\": 0, \"closed\": 0}\n\n"), Ok((0, 0)));
        for bad in ["", "\n", "Traceback (most recent call last):", "{\"sessions\": 1}", "{\"sessions\": 1, \"closed\": 2}", "{\"sessions\": -1, \"closed\": 0}"] {
            assert!(cut_turns_closed(bad).is_err(), "{bad:?}");
        }
        // only Hermes' own session code writes: its boundary row, through its API
        for uses in ["from hermes_state import SessionDB", "list_gateway_sessions(platform=\"relay\")", "failed_turn_notice(", "display_kind=FAILED_TURN_DISPLAY_KIND", "db.append_message("] {
            assert!(CLOSE_CUT_TURNS.contains(uses), "{uses}");
        }
        for never in ["sqlite3", "execute(", "DELETE", "UPDATE", "INSERT"] {
            assert!(!CLOSE_CUT_TURNS.contains(never), "no SQL of ours: {never}");
        }
    }

    fn agent() -> Agent {
        Agent { fragment: "juniper--k3x9".into(), identity: "npub1j".into(), name: "Juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None }
    }

    /// The tiers' models as Finite's names them, but for a medium tier of
    /// its own, so each slot's id is seen where it goes.
    const CHEAP_ID: &str = "z-ai/glm-5.3-flash";
    const MEDIUM_ID: &str = "@cf/zai-org/glm-5.3";
    const FALLBACK_ID: &str = "@cf/zai-org/glm-5.3-flash";

    fn ids() -> ModelIds {
        ModelIds { cheap: CHEAP_ID.into(), medium: MEDIUM_ID.into(), fallback: FALLBACK_ID.into(), vision: FALLBACK_ID.into(), routes: None }
    }

    fn route(base: &str) -> Route {
        Route::new(base, Some(&ids()))
    }

    /// Valid, renamed, replay and invalid: an agent's profile config tells
    /// it its name (its agent fragment's title) in `agent.system_prompt`,
    /// one `agent` mapping whatever its model; a new name is another
    /// config (written again, which Hermes reads at the next turn), the
    /// same name the same bytes (nothing written); a name is one quoted
    /// scalar, and a `${` in it is no variable Hermes would expand.
    #[test]
    fn its_config_tells_the_agent_its_name() {
        let creds = Path::new("/c.sh");
        for main in [Tier::Cheap.into(), Tier::Medium.into(), Tier::High.into()] {
            let c = profile_config(&agent(), "Juniper", Home::Chat, &main, DEFAULT_REASONING, &route("http://m"), &[], creds);
            assert!(c.contains("\nagent:\n  system_prompt: \"Your name is Juniper: your agent fragment's title, as it is now. "), "{c}");
            assert_eq!(c.matches("\nagent:\n").count(), 1, "one `agent` mapping: {c}");
        }
        let named = |name: &str| profile_config(&agent(), name, Home::Chat, &Tier::Medium.into(), DEFAULT_REASONING, &route("http://m"), &[], creds);
        let renamed = named("Frank");
        assert!(renamed.contains("Your name is Frank: ") && renamed.contains("and Frank is your name.") && !renamed.contains("Juniper"), "{renamed}");
        assert_ne!(renamed, named("Juniper"));
        assert_eq!(named("Juniper"), named("Juniper"), "the same name, the same config: nothing written");
        let odd = named("a\"b\n${FRAGMENT_TOKEN}");
        assert!(odd.contains("  system_prompt: \"Your name is a\\\"b\\n$ {FRAGMENT_TOKEN}: "), "one scalar, its `${{` broken apart: {odd}");
        assert!(!told_name("${A}${B}").contains("${"), "{}", told_name("${A}${B}"));
    }

    /// Goal (Paul, 2026-10-16): a profile names each tier's model by the id
    /// the computer's view gives; a platform that names none leaves the
    /// route's names, which it takes too; `high`, on no slot, is its name.
    #[test]
    fn the_routes_ids_are_the_views() {
        let r = route("http://m");
        assert_eq!((r.tier(Tier::Cheap), r.tier(Tier::Medium), r.tier(Tier::High)), (CHEAP_ID, MEDIUM_ID, "high"), "high has no slot");
        let named = Route::new("http://m", None);
        assert_eq!((named.tier(Tier::Cheap), named.tier(Tier::Medium), named.ids.fallback.as_str(), named.ids.vision.as_str()), ("cheap", "medium", "fallback", "vision"));
    }

    #[test]
    fn tiers_read_and_fall_back() {
        assert_eq!(Tier::of(Some(br#"{"tier":"cheap"}"#), false), Tier::Cheap);
        assert_eq!(Tier::of(Some(br#"{"tier":"medium"}"#), false), Tier::Medium, "medium stays selectable");
        assert_eq!(Tier::of(Some(br#"{"tier":"high"}"#), false), Tier::Cheap, "the high tier is off until its limit is raised");
        assert_eq!(Tier::of(Some(br#"{"tier":"high"}"#), true), Tier::High);
        for setting in [None, Some(b"not json".as_slice()), Some(b"{}".as_slice()), Some(br#"{"tier":"unknown"}"#.as_slice())] {
            assert_eq!(Tier::of(setting, true), Tier::Cheap, "{setting:?}");
        }
    }

    /// Goal (Paul, 2026-10-22: "high is a good default"): an agent thinks as
    /// its `agent.json`'s `reasoning` says, `low` or `high`, and hard
    /// (`high`) when it names none, or one the model route does not pass
    /// on. Its profile says so in its one `agent` mapping whatever its
    /// model, never the overlay, and a change is another config (written
    /// again, which Hermes reads at the next turn). Method: read each
    /// agent.json; write each model's config at each level.
    #[test]
    fn reasoning_reads_and_defaults_to_high() {
        assert_eq!(Reasoning::of(Some(br#"{"tier":"cheap","reasoning":"low"}"#)), Reasoning::Low);
        assert_eq!(Reasoning::of(Some(br#"{"tier":"medium","reasoning":"high"}"#)), Reasoning::High);
        let unnamed: [&[u8]; 8] = [b"not json", b"[]", br#"{"tier":"cheap"}"#, br#"{"reasoning":"medium"}"#, br#"{"reasoning":"max"}"#, br#"{"reasoning":"none"}"#, br#"{"reasoning":"LOW"}"#, br#"{"reasoning":1}"#];
        assert_eq!(Reasoning::of(None), Reasoning::High, "no agent.json");
        for setting in unnamed {
            assert_eq!(Reasoning::of(Some(setting)), Reasoning::High, "{}", String::from_utf8_lossy(setting));
        }
        let creds = Path::new("/c.sh");
        let mut own = agent();
        own.credentials.push(fragment_bridge::runtime::Credential { model_base: Some("https://openrouter.ai/api/v1".into()), ..credential("openrouter", &[], "fcx_openrouter_a1") });
        let (own_main, _) = MainModel::of(&own, Tier::Cheap, Some(&OwnModel { provider: "openrouter".into(), id: "anthropic/claude-sonnet-5.5".into() }));
        assert!(matches!(own_main, MainModel::Own { .. }));
        for (main, a) in [(Tier::Cheap.into(), agent()), (Tier::Medium.into(), agent()), (Tier::High.into(), agent()), (own_main, own)] {
            let config = |r: Reasoning| profile_config(&a, "Juniper", Home::Chat, &main, r, &route("http://m"), &[], creds);
            for r in [Reasoning::Low, Reasoning::High] {
                let p = config(r);
                let mapping: Vec<&str> = p.lines().skip_while(|l| *l != "agent:").skip(1).take_while(|l| l.starts_with("  ")).collect();
                assert!(mapping.contains(&format!("  reasoning_effort: \"{}\"", r.name()).as_str()), "the {} model thinks {} in its `agent` mapping: {p}", main.name(), r.name());
                assert_eq!((p.matches("reasoning_effort").count(), p.matches("\nagent:\n").count()), (1, 1), "said once, in its one `agent` mapping: {p}");
                assert_eq!(p, config(r), "the same reasoning, the same config: nothing written");
            }
            assert_ne!(config(Reasoning::Low), config(Reasoning::High), "another reasoning is another config, written again");
        }
        let fresh = profile_config(&agent(), "Juniper", Home::Chat, &Tier::of(None, false).into(), Reasoning::of(None), &route("http://m"), &[], creds);
        assert!(fresh.contains("\n  reasoning_effort: \"high\"\n"), "a new agent, with no agent.json, thinks hard: {fresh}");
        let m = managed_config(&[], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS);
        assert!(!m.contains("reasoning"), "a profile's own, never the overlay's, which would win over it: {m}");
    }

    /// Goal: a new agent without settings runs on Flash, its config naming
    /// the cheap tier's model by its id and saying its tier; with no ids
    /// given, by the tier's name. Method: read no agent.json and check the
    /// config Hermes consumes at its first turn.
    #[test]
    fn a_new_agents_profile_defaults_to_cheap() {
        let tier = Tier::of(None, false);
        let config = profile_config(&agent(), "Juniper", Home::Chat, &tier.into(), DEFAULT_REASONING, &route("http://model.fragment.internal"), &[], Path::new("/c.sh"));
        assert!(config.contains("  default: \"z-ai/glm-5.3-flash\"\n"), "{config}");
        assert!(config.contains("  default_headers:\n    x-fragment-agent: \"juniper--k3x9\"\n    x-fragment-tier: \"cheap\"\n"), "{config}");
        let named = Route::new("http://model.fragment.internal", None);
        let config = profile_config(&agent(), "Juniper", Home::Chat, &tier.into(), DEFAULT_REASONING, &named, &[], Path::new("/c.sh"));
        assert!(config.contains("  default: \"cheap\"\n") && config.contains("    model: \"fallback\"\n"), "{config}");
    }

    /// Goal: naming a tier's model by its id changes what Hermes says the
    /// model is, not the prompt the tier was validated on (named `cheap`
    /// and `medium`, which no family Hermes guides matched). Method: every
    /// tier's profile turns both of Hermes' family gates off, in its one
    /// `agent` section; an owner's own model, always named by its id,
    /// leaves them Hermes' (`auto`), and the overlay, which would win over
    /// it, names neither. (Run against the real Hermes in the bridge's
    /// tests/docker.rs, `the_hermes_image`: a GLM's prompt with neither.)
    #[test]
    fn a_tiers_prompt_is_the_tiers_whatever_its_models_family() {
        let creds = Path::new("/c.sh");
        let off = format!("\nagent:\n  system_prompt: {}\n  tool_use_enforcement: false\n  execution_guidance: false\n", q(&told("Juniper", "juniper--k3x9", Home::Chat)));
        for tier in [Tier::Cheap, Tier::Medium, Tier::High] {
            let p = profile_config(&agent(), "Juniper", Home::Chat, &tier.into(), DEFAULT_REASONING, &route("http://model.fragment.internal"), &[], creds);
            assert!(p.contains(&off), "the {} tier's guidance is its tier's, beside its name: {p}", tier.name());
            assert_eq!(p.matches("\nagent:\n").count(), 1, "one `agent` mapping: {p}");
        }
        let mut own = agent();
        own.credentials.push(fragment_bridge::runtime::Credential { model_base: Some("https://openrouter.ai/api/v1".into()), ..credential("openrouter", &[], "fcx_openrouter_a1") });
        let (own_main, _) = MainModel::of(&own, Tier::Cheap, Some(&OwnModel { provider: "openrouter".into(), id: "z-ai/glm-5.3".into() }));
        let p = profile_config(&own, "Juniper", Home::Chat, &own_main, DEFAULT_REASONING, &route("http://model.fragment.internal"), &[], creds);
        assert!(matches!(own_main, MainModel::Own { .. }) && p.contains("  default: \"z-ai/glm-5.3\"\n"), "{p}");
        assert!(p.contains(&format!("\nagent:\n  system_prompt: {}\n", q(&told("Juniper", "juniper--k3x9", Home::Chat)))) && p.matches("\nagent:\n").count() == 1, "its name alone in its `agent` mapping: {p}");
        assert!(!p.contains("tool_use_enforcement") && !p.contains("execution_guidance"), "an owner's own model's guidance is Hermes' to choose: {p}");
        let m = managed_config(&[], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS);
        assert!(!m.contains("tool_use_enforcement") && !m.contains("execution_guidance"), "a profile's own, never the overlay's: {m}");
    }

    /// An `agent.json` the platform answered for is a tier (none there is
    /// the cheap tier); one it did not answer for is none, never the
    /// default tier in its place.
    #[test]
    fn a_tier_unread_is_no_tier() {
        use fragment_bridge::api::ApiError;
        let refused = |status| Err(ApiError::Refused { status, error: String::new(), message: String::new() });
        assert_eq!(Tier::read(&Ok(bytes::Bytes::from_static(br#"{"tier":"cheap"}"#)), false), Some(Tier::Cheap));
        assert_eq!(Tier::read(&refused(404), false), Some(Tier::Cheap), "no agent.json");
        assert_eq!(Tier::read(&refused(403), false), Some(Tier::Cheap));
        for unread in [refused(500), refused(502), Err(ApiError::Transport("no answer in 15000 ms".into())), Err(ApiError::TooLarge)] {
            assert_eq!(Tier::read(&unread, false), None, "{unread:?}");
        }
    }

    #[test]
    fn configs_say_what_hermes_needs() {
        let m = managed_config(&["platforms/discord".into(), "dashboard_auth/basic".into()], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS);
        for want in [
            "transport: \"draft\"",
            "busy_input_mode: \"queue\"",
            "group_sessions_per_user: false",
            "disabled_toolsets: [\"cronjob\"]",
            "    - \"platforms/discord\"",
            "bot_desktop:\n  auto_start: true\n  idle_stop_minutes: 10\n",
            "\nmodel_catalog:\n  enabled: false\n",
            "\nsecurity:\n  allow_lazy_installs: false\n",
            "\nstt:\n  echo_transcripts: false\n",
        ] {
            assert!(m.contains(want), "managed config has {want}:\n{m}");
        }
        // `display`'s own lines: the model's text beside a tool call is no
        // message of its own, for every platform (none names `relay`)
        let display: Vec<&str> = m.lines().skip_while(|l| *l != "display:").skip(1).take_while(|l| l.starts_with("  ")).collect();
        assert!(display.contains(&"  interim_assistant_messages: false"), "no interim messages, under display: {m}");
        assert!(display.contains(&"  tool_preview_length: 140"), "a step's preview as long as the bridge keeps its arguments: {m}");
        assert!(!m.contains("\n  platforms:"), "no platform's display setting overrides it: {m}");
        // a test's shorter card, held within the bridge's bounds: an
        // approval's wait and a clarify's both
        assert_eq!(card_timeout_s(None), CARD_TIMEOUT_S);
        assert_eq!(CARD_TIMEOUT_S, 3600, "Hermes' own clarify wait, and the bridge's card");
        assert_eq!(card_timeout_s(Some("20")), 20);
        assert_eq!(card_timeout_s(Some("1")), 10, "no shorter than the bridge's shortest prompt");
        assert_eq!(card_timeout_s(Some("not a number")), CARD_TIMEOUT_S);
        let short = managed_config(&[], 20, crate::desktop::IDLE_STOP_MS);
        assert!(short.contains("\napprovals:\n  mode: \"off\"\n  timeout: 20\n") && short.contains("\nagent:\n  disabled_toolsets: [\"cronjob\"]\n  clarify_timeout: 20\n"), "{short}");
        assert!(managed_config(&[], 20, 30_000).contains("  idle_stop_minutes: 0.5\n"), "a test's shorter idle bound, in Hermes' minutes");
        let creds = Path::new("/data/hermes/profiles/juniper--k3x9/credentials.sh");
        let p = profile_config(&agent(), "Juniper", Home::Chat, &Tier::Medium.into(), DEFAULT_REASONING, &route("http://model.fragment.internal/"), &[], creds);
        assert!(p.contains("base_url: \"http://model.fragment.internal/v1\""), "{p}");
        assert!(p.contains("default: \"@cf/zai-org/glm-5.3\"") && p.contains("    x-fragment-tier: \"medium\"\n"), "its tier's model by its id, its tier said: {p}");
        assert!(p.contains("x-fragment-agent: \"juniper--k3x9\""), "every model call names its agent");
        let h = profile_config(&agent(), "Juniper", Home::Chat, &Tier::High.into(), DEFAULT_REASONING, &route("http://model.fragment.internal"), &[], creds);
        assert!(h.contains("provider: \"anthropic\"") && h.contains("/anthropic\"") && h.contains("default: \"high\""), "{h}");
        // its eyes: the route's vision model, OpenAI's shape, whatever its tier
        let vision = "  vision:\n    provider: \"custom\"\n    base_url: \"http://model.fragment.internal/v1\"\n    model: \"vision\"\n    api_key: \"fragment-model\"\n";
        for (tier, config) in [("medium", &p), ("high", &h), ("cheap", &profile_config(&agent(), "Juniper", Home::Chat, &Tier::Cheap.into(), DEFAULT_REASONING, &route("http://model.fragment.internal"), &[], creds))] {
            assert!(config.contains(vision), "the {tier} tier's screenshots go to the route's vision model: {config}");
            assert!(config.contains("plugins:\n  enabled: [fragment-skill-fork, fragment-deploy-wait, fragment-activity]\nauxiliary:\n  background_review: { enabled: true }\n"), "the {tier} tier reviews memory, forks external skills, waits for deployment preflight and holds delegated work: {config}");
            assert!(!config.contains("curator:"), "the separate periodic curator keeps Hermes' defaults: {config}");
        }
        assert!(!m.contains("auxiliary:"), "each profile's own, beside the headers that name its agent: {m}");
        let ears = "stt:\n  provider: \"openai\"\n  language: \"\"\n  openai:\n    base_url: \"http://model.fragment.internal/v1\"\n    api_key: \"agent:juniper--k3x9\"\n    model: \"whisper\"\n";
        for (tier, config) in [("medium", &p), ("high", &h)] {
            assert!(config.contains(ears), "the {tier} tier's voice memos go to the route's whisper, its key naming the agent, no language forced: {config}");
        }
        assert!(!m.contains("api_key") && !m.contains("openai"), "the overlay names no agent, so a call outside a profile names none either: {m}");
        // a text tier's stall: Hermes' own stale detector, one retry, then the route's `fallback` as the agent
        let fallback = "fallback_providers:\n  - provider: \"custom\"\n    base_url: \"http://model.fragment.internal/v1\"\n    model: \"@cf/zai-org/glm-5.3-flash\"\n    api_key: \"agent:juniper--k3x9\"\n";
        let c = profile_config(&agent(), "Juniper", Home::Chat, &Tier::Cheap.into(), DEFAULT_REASONING, &route("http://model.fragment.internal"), &[], creds);
        for (tier, id, config) in [("cheap", CHEAP_ID, &c), ("medium", MEDIUM_ID, &p)] {
            assert!(config.contains(fallback), "the {tier} tier falls back to the route's fallback model, by its id: {config}");
            assert!(config.contains(&format!("providers:\n  custom:\n    models:\n      \"{id}\":\n        stale_timeout_seconds: 20\n")), "its own model's stale timeout alone: {config}");
            assert!(config.contains(&format!("\nagent:\n  system_prompt: {}\n  tool_use_enforcement: false\n  execution_guidance: false\n  api_max_retries: 1\n", q(&told("Juniper", "juniper--k3x9", Home::Chat)))), "one retry, beside its name and its guidance: {config}");
            assert!(!config.contains("\"vision\":\n        stale_timeout_seconds"), "vision keeps Hermes' default: {config}");
        }
        // each tier's stall falls back within its route (Paul, 2026-10-24), by its route's fallback's id
        let mut routed = route("http://model.fragment.internal");
        let tier_route = |fallback: &str| fragment_bridge::api::TierRoute { label: None, private: false, fallback: fallback.into(), vision: FALLBACK_ID.into() };
        routed.ids.routes = Some(Box::new(fragment_bridge::api::TierRoutes { cheap: tier_route("deepseek-v4-1-flash"), medium: tier_route("@cf/example/standard-fallback") }));
        for (tier, want) in [(Tier::Cheap, "deepseek-v4-1-flash"), (Tier::Medium, "@cf/example/standard-fallback")] {
            let config = profile_config(&agent(), "Juniper", Home::Chat, &tier.into(), DEFAULT_REASONING, &routed, &[], creds);
            let stall = format!("fallback_providers:\n  - provider: \"custom\"\n    base_url: \"http://model.fragment.internal/v1\"\n    model: \"{want}\"\n    api_key: \"agent:juniper--k3x9\"\n");
            assert!(config.contains(&stall), "the {tier:?} tier's stall falls back to its route's fallback: {config}");
            assert!(config.contains(vision), "its eyes are still the route's `vision`, its tier said: {config}");
        }
        assert_eq!((routed.fallback(Tier::High), route("http://x").fallback(Tier::Medium)), (Some(FALLBACK_ID), Some(FALLBACK_ID)), "none named: the deployment's");
        // a fallback that is the tier's own model is none (#527): one id, one server, asked again
        let mut own = route("http://model.fragment.internal");
        own.ids.routes = Some(Box::new(fragment_bridge::api::TierRoutes { cheap: tier_route(CHEAP_ID), medium: tier_route(FALLBACK_ID) }));
        let (cheap, medium) = (profile_config(&agent(), "Juniper", Home::Chat, &Tier::Cheap.into(), DEFAULT_REASONING, &own, &[], creds), profile_config(&agent(), "Juniper", Home::Chat, &Tier::Medium.into(), DEFAULT_REASONING, &own, &[], creds));
        assert!(!cheap.contains("fallback_providers") && cheap.contains("stale_timeout_seconds: 20"), "no switch to its own model, its patience kept: {cheap}");
        assert!(medium.contains(&format!("model: {}\n    api_key", q(FALLBACK_ID))), "another model stays its switch: {medium}");
        let mut unrouted = route("http://model.fragment.internal");
        unrouted.ids.fallback = CHEAP_ID.into();
        assert_eq!(unrouted.fallback(Tier::Cheap), None, "from a platform naming no routes too");
        assert!(!h.contains("fallback_providers") && !h.contains("stale_timeout_seconds") && !h.contains("api_max_retries"), "the high tier has none: {h}");
        assert!(!m.contains("fallback_providers") && !m.contains("stale_timeout_seconds") && !m.contains("api_max_retries"), "a profile's own, never the overlay's: {m}");
        assert!(p.contains("skills:\n  external_dirs: [\"/data/hermes/managed-skills\", \"/var/lib/fragment-run/platform-skills\"]\n"), "the managed skills and the platform skill's view, after its own: {p}");
        assert!(p.contains("browser:\n  headed: true\n  backend: \"off\"\n"), "Hermes' built-in browser, headed, in the profile's own config: {p}");
        assert!(!m.contains("browser:"), "Hermes never reads `browser` from the managed overlay: {m}");
        assert!(p.contains("terminal:\n  env_passthrough: [\"FRAGMENT_AS_AGENT\", \"FRAGMENT_FOR\"]\n"), "its terminal acts as the agent: {p}");
        assert!(p.contains("\n  cwd: \"/data/work/juniper--k3x9\"\n"), "its terminal works in its work directory: {p}");
        assert_eq!(work_dir("juniper--k3x9"), PathBuf::from("/data/work/juniper--k3x9"));
        assert!(
            p.contains("  shell_init_files: [\"~/.profile\", \"~/.bash_profile\", \"~/.bashrc\", \"/data/hermes/profiles/juniper--k3x9/credentials.sh\"]\n"),
            "its shell starts as Hermes' does, then reads its credentials: {p}"
        );
        let e = profile_env(&agent());
        assert!(e.contains("\nFRAGMENT_AS_AGENT=juniper--k3x9\n") && e.contains("\nFRAGMENT_FOR=npub1paul\n"), "{e}");
        assert!(!e.contains("KEY") && !e.contains("TOKEN"), "no credential, and none held: {e}");
        let env = gateway_env("127.0.0.1:8650", "computer", &"s".repeat(32));
        assert!(env.contains("GATEWAY_RELAY_URL=http://127.0.0.1:8650\n"));
        assert!(env.contains("HERMES_GATEWAY_BUSY_INPUT_MODE=queue"));
        assert!(env.contains("HERMES_AUTO_CONTINUE_FRESHNESS=1\n"), "a turn a restart cut short is never auto-continued");
        assert!(env.contains("HERMES_GATEWAY_MAX_STARTS=0\n"), "no start is slept for the starts before it");
        assert!(env.contains("HERMES_STREAM_RETRIES=1\n"), "a stalled stream is tried once more, then the fallback");
        assert_eq!(profile_dir(Path::new("/data/hermes"), "juniper--k3x9"), PathBuf::from("/data/hermes/profiles/juniper--k3x9"));
    }

    /// Goal (2026-10-22): every agent is told, on every turn in our chats,
    /// that a file someone attaches is already on its computer, where the
    /// note before the message names its path (an image's after
    /// `image_url:`, as Hermes' note says it), that it is cleared after a
    /// day, and how to put it on a website (a site's upload button with
    /// its browser tools, then the path typed into its desktop's file dialog
    /// with computer_use), in plain words; and the `fragment` skill's first
    /// page says the same at more length. Method: the overlay's relay hint
    /// after `ASKING`, its words, and the page's section.
    #[test]
    fn every_agent_is_told_its_chat_files_are_on_its_computer() {
        let m = managed_config(&[], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS);
        assert_eq!(relay_hint(), format!("{ASKING}\n\n{FILES}\n\n{SENDING}\n\n{SIMPLEX}"));
        assert!(m.contains(&format!("    append: {}\n", q(&relay_hint()))), "its files beside how to ask, in the relay's hint: {m}");
        for said in ["already on your computer", "the note before their message names its path", "image_url:", "cleared after a day", "copy it into your work", "upload button with your browser tools", "full path into the file dialog on your desktop with computer_use", "press Enter"] {
            assert!(FILES.contains(said), "{said}: {FILES}");
        }
        assert!(!FILES.contains(['`', '\n', '$']) && FILES.len() < 500, "one plain paragraph, nothing Hermes would expand: {FILES}");
        let page = include_str!("computer.md");
        let section = page.split("\n## ").find(|s| s.starts_with("Files your owner sends you\n")).expect("computer.md has the section");
        let section = section.split_whitespace().collect::<Vec<_>>().join(" ");
        for said in ["already on your computer", "image_url:", "after a day", "copy it into your work", "upload button with your browser tools", "file dialog on your desktop", "computer_use", "press Enter"] {
            assert!(section.contains(said), "computer.md, \"Files your owner sends you\", says {said}: {section}");
        }
    }

    /// Goal (Paul, 2026-10-23: asked for a PDF in the chat, his agent named
    /// a path, then emailed it): every agent is told, on every turn in our
    /// chats, that a file it sends goes into the chat as an attachment, by
    /// a `MEDIA:` line of its own outside code and quotes (the only tag
    /// Hermes delivers), and to attach one asked for there rather than
    /// email it or name a path; and the `fragment` skill's first page says
    /// the same. Method: the overlay's relay hint after `FILES`, its words,
    /// and the page's section.
    #[test]
    fn every_agent_is_told_it_attaches_files_in_its_chat() {
        let m = managed_config(&[], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS);
        assert!(relay_hint().contains(&format!("{FILES}\n\n{SENDING}\n\n")), "after the files it is sent: {}", relay_hint());
        assert!(m.contains(&format!("    append: {}\n", q(&relay_hint()))), "in the relay's hint: {m}");
        for said in ["a file (a PDF, an image", "put MEDIA: and its absolute path on a line of its own", "outside code and quotes", "MEDIA:/data/work/report.pdf", "arrives in this chat as an attachment", "rather than emailing it or naming a path"] {
            assert!(SENDING.contains(said), "{said}: {SENDING}");
        }
        assert!(!SENDING.contains(['`', '\n', '$']) && SENDING.len() < 500, "one plain paragraph, nothing Hermes would expand: {SENDING}");
        let page = include_str!("computer.md");
        let section = page.split("\n## ").find(|s| s.starts_with("Files you send\n")).expect("computer.md has the section");
        let section = section.split_whitespace().collect::<Vec<_>>().join(" ");
        for said in ["`MEDIA:<absolute-path>`", "a line of its own", "outside code and quotes", "attachment they can open and download", "email it only when they ask for email", "never answer with a path"] {
            assert!(section.contains(said), "computer.md, \"Files you send\", says {said}: {section}");
        }
    }

    /// Goal (Paul, 2026-10-26: asked over SimpleX to send its voice reply
    /// there, his agent began setting up a SimpleX of its own): every agent
    /// is told, on every turn in our chats, that Fragment carries its
    /// owner's SimpleX and never to set SimpleX up. Method: the overlay's
    /// relay hint. Which chat is its owner's SimpleX is that chat's topic
    /// (the bridge's), never this line's.
    #[test]
    fn every_agent_is_told_simplex_is_the_platforms() {
        let m = managed_config(&[], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS);
        assert!(relay_hint().ends_with(&format!("{SENDING}\n\n{SIMPLEX}")), "last: {}", relay_hint());
        assert!(m.contains(&format!("    append: {}\n", q(&relay_hint()))), "in the relay's hint: {m}");
        for said in ["Fragment carries your owner's SimpleX", "never install, run or set up SimpleX"] {
            assert!(SIMPLEX.contains(said), "{said}: {SIMPLEX}");
        }
        assert!(!SIMPLEX.contains(['`', '\n', '$', '"', '|']) && SIMPLEX.len() < 160, "one short plain line: {SIMPLEX}");
    }

    /// Goal (Paul, 2026-10-26: "it has to be clear obvious and reliable where
    /// notifications will go"): every agent is told, beside its name at every
    /// turn, where its owner's home is (its chat by its label, or their
    /// SimpleX chat, which reaches their phone) and that a routine without
    /// `--chat` runs there; its agent.json's `home` picks it, and a change is
    /// another config (its next turn's), the same home the same bytes.
    #[test]
    fn its_config_tells_the_agent_where_its_owner_is_reached() {
        let creds = Path::new("/c.sh");
        let config = |home: Home| profile_config(&agent(), "Juniper", home, &Tier::Cheap.into(), DEFAULT_REASONING, &route("http://m"), &[], creds);
        assert_eq!(told_home("juniper--k3x9", Home::Chat), "Your owner's home is your chat juniper-chat: a routine you schedule without --chat (a reminder, a check to report on) runs there.");
        assert_eq!(told_home("juniper--k3x9", Home::Simplex), "Your owner's home is their SimpleX, your chat juniper-simplex, which reaches their phone: a routine you schedule without --chat (a reminder, a check to report on) runs there.");
        for home in [Home::Chat, Home::Simplex] {
            let c = config(home);
            assert!(c.contains(&format!("\nagent:\n  system_prompt: {}\n", q(&format!("{} {}", told_name("Juniper"), told_home("juniper--k3x9", home))))), "its name, then its home: {c}");
            assert_eq!(c, config(home), "the same home, the same config: nothing written");
        }
        assert_ne!(config(Home::Chat), config(Home::Simplex), "a new home is another config, its next turn's");
        assert_eq!(Home::of(Some(br#"{"tier":"cheap","home":"simplex"}"#)), Home::Simplex, "agent.json picks it");
        assert_eq!(Home::of(Some(br#"{"tier":"cheap"}"#)), Home::Chat);
    }

    /// Goal (Paul, 2026-10-14: "if hermes asks them a technical approval it
    /// makes no sense to them"): nothing technical asks the agent's owner, on
    /// any model. Method: the overlay, merged over every profile, turns
    /// Hermes' approval gate off (its terminal's flagged commands, its
    /// execute_code scripts, its desktop, an SSH config's writes) and its
    /// protected files' writes, and tells every agent to ask its people only
    /// about outcomes, in plain words, with its clarify tool's choices; no
    /// profile says otherwise, nor holds a standing grant. (Run against the
    /// real Hermes in the bridge's tests/docker.rs,
    /// `nothing_technical_asks_its_owner`: each with no card.)
    #[test]
    fn nothing_technical_asks_its_owner() {
        let m = managed_config(&[], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS);
        assert!(m.contains("\napprovals:\n  mode: \"off\"\n  timeout: 3600\n  destructive_slash_confirm: false\n"), "{m}");
        assert!(m.contains("\nsecurity:\n  allow_lazy_installs: false\n  protected_instruction_files: false\n"), "{m}");
        assert!(!m.contains("smart") && !m.contains("yolo") && !m.contains("command_allowlist"), "{m}");
        assert!(m.contains(&format!("\nplatform_hints:\n  relay:\n    append: {}\n", q(&relay_hint()))) && relay_hint().starts_with(ASKING), "every agent in our chats is told how to ask: {m}");
        for said in ["not technical", "Never ask them to approve a command, a file, a path or code", "in plain words", "the clarify tool and its choices", "public", "someone new", "an email", "spend their money"] {
            assert!(ASKING.contains(said), "{said}: {ASKING}");
        }
        assert!(!ASKING.contains(['`', '\n']) && ASKING.len() < 700, "one plain paragraph: {ASKING}");
        let creds = Path::new("/c.sh");
        let mut own = agent();
        own.credentials.push(fragment_bridge::runtime::Credential { model_base: Some("https://openrouter.ai/api/v1".into()), ..credential("openrouter", &[], "fcx_openrouter_a1") });
        let (own_main, _) = MainModel::of(&own, Tier::Cheap, Some(&OwnModel { provider: "openrouter".into(), id: "anthropic/claude-sonnet-5.5".into() }));
        assert!(matches!(own_main, MainModel::Own { .. }));
        for (main, a) in [(Tier::Cheap.into(), agent()), (Tier::Medium.into(), agent()), (Tier::High.into(), agent()), (own_main, own)] {
            let p = profile_config(&a, "Juniper", Home::Chat, &main, DEFAULT_REASONING, &route("http://model.fragment.internal"), &[], creds);
            for overlaid in ["command_allowlist", "approvals:", "security:", "platform_hints:", "clarify_timeout"] {
                assert!(!p.contains(overlaid), "the overlay's alone, never the {} profile's: {p}", main.name());
            }
            let falls_back = matches!(main, MainModel::Tier(Tier::Cheap | Tier::Medium));
            assert_eq!(p.contains("fallback_providers:"), falls_back, "only a text tier falls back to the route's fallback (an owner's own model is theirs): {p}");
        }
        assert!(!default_config("http://model.fragment.internal").contains("approvals"), "the gateway's own profile runs no turns");
    }

    /// The interception CA's policy names each certificate's DER, as the
    /// PEM carries it; a PEM with none names none.
    #[test]
    fn the_interception_ca_is_the_browsers_by_policy() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIBAAAA\nBBBB==\n-----END CERTIFICATE-----\n\n-----BEGIN CERTIFICATE-----\nCCCC\n-----END CERTIFICATE-----\n";
        let policy: serde_json::Value = serde_json::from_str(&chromium_ca_policy(pem).unwrap()).unwrap();
        assert_eq!(policy, serde_json::json!({ "CACertificates": ["MIIBAAAABBBB==", "CCCC"] }));
        assert_eq!(chromium_ca_policy(""), None);
        assert_eq!(chromium_ca_policy("-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n"), None);
    }

    /// A browser change must also select that browser's managed policy
    /// directory: Chrome for Testing's directory is ignored by Debian Chromium.
    #[test]
    fn the_interception_ca_policy_follows_the_launched_browser() {
        let managed = match SYSTEM_CHROMIUM {
            "/usr/bin/chromium" => Path::new("/etc/chromium/policies/managed"),
            browser => panic!("define the managed policy directory for the launched browser: {browser}"),
        };
        assert_eq!(Path::new(CHROMIUM_CA_POLICY).parent(), Some(managed), "the interception CA must be a policy of {SYSTEM_CHROMIUM}");
    }

    /// The image's Chromium starts Hermes' pinned one with the container's
    /// flags, on a desktop or with none: run here by `sh`, as Hermes runs it.
    #[test]
    fn the_images_chromium_carries_the_containers_flags() {
        let s = chromium_script(Path::new(SYSTEM_CHROMIUM));
        assert!(s.starts_with("#!/bin/sh\n"), "{s}");
        assert_eq!(CHROMIUM_FLAGS, ["--no-sandbox", "--disable-dev-shm-usage"], "Hermes' CHROMIUM_SANDBOX_BYPASS_ARGS");
        let dir = std::env::temp_dir().join(format!("hermes-chromium-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // the browser, saying what it was started with, and its scratch
        let full = dir.join("full");
        std::fs::write(&full, "#!/bin/sh\necho full \"$@\" \"tmp=$TMPDIR\"\n").unwrap();
        std::fs::set_permissions(&full, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let script = dir.join("chromium");
        std::fs::write(&script, chromium_script(&full)).unwrap();
        let run = |display: Option<&str>| {
            let mut c = std::process::Command::new("sh");
            // Hermes' scratch, under its home, as its terminal has it
            c.arg(&script).args(["--user-data-dir=/p", "https://example.com"]).env_remove("DISPLAY").env("TMPDIR", "/data/hermes/profiles/p/cache/scratch");
            if let Some(d) = display {
                c.env("DISPLAY", d);
            }
            String::from_utf8(c.output().unwrap().stdout).unwrap()
        };
        let started = "full --no-sandbox --disable-dev-shm-usage --test-type --user-data-dir=/p https://example.com tmp=/tmp\n";
        assert_eq!(run(Some(":20")), started, "on a desktop");
        assert_eq!(run(None), started, "with none: its caller asks for headless");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Hermes' container guesses, pinned: each named once, as Hermes reads
    /// it, and nothing the boot writes names one again (Hermes' config
    /// overrides its environment); `profile` needs each profile's `home`,
    /// without which Hermes keeps the gateway's own `HOME`.
    #[test]
    fn hermes_container_guesses_are_pinned() {
        let pins: std::collections::BTreeMap<&str, &str> = RUNTIME_ENV.into_iter().collect();
        assert_eq!(pins.len(), RUNTIME_ENV.len(), "each named once: {RUNTIME_ENV:?}");
        assert_eq!(pins.get("TERMINAL_HOME_MODE"), Some(&"profile"), "one of Hermes' auto, real, profile");
        assert_eq!(pins.get("HERMES_SKIP_CHMOD"), Some(&"1"));
        assert!(PROFILE_DIRS.contains(&"home"), "every profile has its home: {PROFILE_DIRS:?}");
        let creds = Path::new("/data/hermes/profiles/juniper--k3x9/credentials.sh");
        let written = [managed_config(&["platforms/discord".into()], CARD_TIMEOUT_S, crate::desktop::IDLE_STOP_MS), default_config("http://m"), profile_config(&agent(), "Juniper", Home::Chat, &Tier::Medium.into(), DEFAULT_REASONING, &route("http://m"), &[], creds), gateway_env("127.0.0.1:1", "c", &"s".repeat(32))];
        for (name, _) in RUNTIME_ENV {
            let key = name.strip_prefix("TERMINAL_").unwrap_or(name).to_ascii_lowercase();
            assert!(written.iter().all(|w| !w.contains(name) && !w.contains(&format!("{key}:"))), "{name} is the boot's environment's alone: {written:#?}");
        }
    }

    /// An agent's home and its temp files are in its work, and its
    /// profile's `home` and scratch the links to them: made for a fresh
    /// profile (the scratch's `cache` with it); kept when each is the link
    /// (each boot); an empty one or a link elsewhere replaced; one with
    /// something in it set aside whole, and nothing of it carried over.
    #[test]
    fn a_profiles_home_and_scratch_are_links_into_its_work() {
        assert_eq!(home_dir("juniper--k3x9"), PathBuf::from("/data/work/juniper--k3x9/home"));
        assert_eq!(tmp_dir("juniper--k3x9"), PathBuf::from("/data/work/juniper--k3x9/tmp"));
        assert!(PROFILE_DIRS.contains(&PROFILE_HOME), "Hermes' name for it: {PROFILE_DIRS:?}");
        assert_eq!(format!("{PROFILE_HOME}{SET_ASIDE}"), "home.before-work", "the name a home from before was set aside as since 2026-10-07");
        let root = std::env::temp_dir().join(format!("hermes-work-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let profile = root.join("hermes/profiles/juniper--k3x9");
        std::fs::create_dir_all(&profile).unwrap();
        let linked = |l: &Path| std::fs::read_link(l).ok();
        for (at, to) in [(PROFILE_HOME, root.join("work/juniper--k3x9/home")), (PROFILE_SCRATCH, root.join("work/juniper--k3x9/tmp"))] {
            let link = profile.join(at);
            // valid: a fresh profile, its directory in the work made
            assert_eq!(link_into_work(&profile, at, &to).unwrap(), WorkLink::Linked, "{at}");
            assert_eq!(linked(&link), Some(to.clone()));
            assert!(to.is_dir());
            std::fs::write(to.join("kept"), "x").unwrap();
            // replay: the next boot keeps it, and what is in it
            assert_eq!(link_into_work(&profile, at, &to).unwrap(), WorkLink::Kept, "{at}");
            assert!(link.join("kept").is_file(), "written through the link, kept in the work");
            // a link elsewhere is pointed at the work
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(root.join("elsewhere"), &link).unwrap();
            assert_eq!(link_into_work(&profile, at, &to).unwrap(), WorkLink::Linked, "{at}");
            assert_eq!(linked(&link), Some(to.clone()));
            // an empty one (Hermes' own, made before the link) goes
            std::fs::remove_file(&link).unwrap();
            std::fs::create_dir(&link).unwrap();
            assert_eq!(link_into_work(&profile, at, &to).unwrap(), WorkLink::Linked, "{at}");
            assert!(!profile.join(format!("{at}{SET_ASIDE}")).exists(), "nothing to set aside");
            // one from before, with files: set aside unmoved, twice numbered
            for (n, aside) in [format!("{at}{SET_ASIDE}"), format!("{at}{SET_ASIDE}-1")].iter().enumerate() {
                std::fs::remove_file(&link).unwrap();
                std::fs::create_dir_all(link.join("from-before")).unwrap();
                std::fs::write(link.join("from-before/file"), format!("{n}")).unwrap();
                assert_eq!(link_into_work(&profile, at, &to).unwrap(), WorkLink::SetAside(profile.join(aside)), "{at}");
                assert_eq!(std::fs::read_to_string(profile.join(aside).join("from-before/file")).unwrap(), format!("{n}"), "set aside whole");
                assert_eq!(linked(&link), Some(to.clone()));
                assert!(!to.join("from-before").exists(), "nothing of it carried into the work");
            }
        }
        assert!(profile.join("cache").is_dir() && !profile.join("cache").is_symlink(), "the scratch's parent is Hermes' own");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[should_panic(expected = "a browser's path, absolute, quotable")]
    fn a_browser_path_a_shell_would_misread_is_a_bug() {
        chromium_script(Path::new("/opt/it's/chrome"));
    }

    fn credential(provider: &str, env: &[&str], placeholder: &str) -> fragment_bridge::runtime::Credential {
        fragment_bridge::runtime::Credential {
            provider: provider.into(),
            kind: "operator".into(),
            env: env.iter().map(|e| e.to_string()).collect(),
            placeholder: placeholder.into(),
            hosts: vec!["api.perplexity.ai".into()],
            model_base: None,
        }
    }

    /// Goal: only an agent's currently held placeholders select paid tool
    /// backends. Method: each category offered, removed, and merely named
    /// in credential_env; another profile remains on Hermes' defaults.
    #[test]
    fn tool_categories_select_only_offered_credentials() {
        for (provider, env, selection) in [
            ("firecrawl", "FIRECRAWL_API_KEY", "web:\n  backend: \"firecrawl\"\n  search_backend: \"firecrawl\"\n  extract_backend: \"firecrawl\"\n"),
            ("fal", "FAL_KEY", "image_gen:\n  provider: \"fal\"\n  model: \"fal-ai/flux-2/klein/9b\"\n"),
            ("elevenlabs", "ELEVENLABS_API_KEY", "tts:\n  provider: \"elevenlabs\"\n"),
            ("browser-use", "BROWSER_USE_API_KEY", "  cloud_provider: \"local\"\n"),
            ("perplexity", "PERPLEXITY_API_KEY", "web:\n  search_backend: \"perplexity\"\n"),
        ] {
            let mut a = agent();
            let write = |a: &Agent| profile_config(a, "Juniper", Home::Chat, &Tier::Cheap.into(), DEFAULT_REASONING, &route("http://m"), &[env.into()], Path::new("/c.sh"));
            let absent = write(&a);
            assert!(!absent.contains(selection), "catalog alone never selects {provider}");
            a.credentials.push(credential(provider, &[env], &format!("fck_{provider}_{}", "a".repeat(32))));
            let present = write(&a);
            assert!(present.contains(selection), "offered {provider}: {present}");
            assert!(present.contains("browser:\n  headed: true\n  backend: \"off\"\n"));
            a.credentials.clear();
            assert_eq!(write(&a), absent, "revocation restores {provider}'s defaults");
        }
        let mut a = agent();
        for (provider, env) in [("perplexity", "PERPLEXITY_API_KEY"), ("firecrawl", "FIRECRAWL_API_KEY"), ("xai", "XAI_API_KEY")] {
            a.credentials.push(credential(provider, &[env], &format!("fck_{provider}_{}", "a".repeat(32))));
        }
        let config = profile_config(&a, "Juniper", Home::Chat, &Tier::Cheap.into(), DEFAULT_REASONING, &route("http://m"), &[], Path::new("/c.sh"));
        assert!(config.contains("  search_backend: \"firecrawl\"\n"), "Firecrawl wins over Perplexity");
        assert!(!config.contains("\"xai\""), "X search is a separate native tool, never a web backend");
        assert!(profile_env(&a).contains("XAI_API_KEY=fck_xai_"));
        assert!(profile_env(&a).contains("PERPLEXITY_API_KEY=fck_perplexity_"));
    }

    /// Valid: an agent.json that names a model of a provider its owner
    /// connected runs on it: the provider's base, the model's id, the
    /// provider's placeholder as its key (the swap adds the key), and its
    /// eyes and ears stay on the route. Invalid: a model named badly, or of
    /// a provider its credentials do not hold (not connected, narrowed) or
    /// that serves none, is its tier, saying why.
    #[test]
    fn an_agent_runs_on_its_owners_model_when_it_holds_it() {
        let tag = "0123456789abcdef0123456789abcdef";
        let json = br#"{"tier":"cheap","model":{"provider":"openrouter","id":"anthropic/claude-sonnet-5.5"},"color":"blue"}"#;
        let own = OwnModel::of(json).unwrap();
        assert_eq!(own, OwnModel { provider: "openrouter".into(), id: "anthropic/claude-sonnet-5.5".into() });
        for bad in [&br#"{"model":{"provider":"OpenRouter","id":"x"}}"#[..], br#"{"model":{"provider":"openrouter","id":"a b"}}"#, br#"{"model":{"provider":"openrouter"}}"#, br#"{"model":"openrouter:x"}"#, b"{}", b"not json"] {
            assert_eq!(OwnModel::of(bad), None, "{}", String::from_utf8_lossy(bad));
        }
        let mut a = agent();
        let (main, why) = MainModel::of(&a, Tier::Cheap, Some(&own));
        assert_eq!(main, MainModel::Tier(Tier::Cheap));
        assert!(why.unwrap().contains("not connected openrouter"));
        let mut c = credential("openrouter", &["OPENROUTER_API_KEY"], &format!("fck_openrouter_{tag}"));
        a.credentials = vec![c.clone()];
        let (main, why) = MainModel::of(&a, Tier::Cheap, Some(&own));
        assert!(main == MainModel::Tier(Tier::Cheap) && why.unwrap().contains("serves no models"), "a provider with no model base");
        c.model_base = Some("https://openrouter.ai/api/v1/".into());
        a.credentials = vec![c];
        let (main, why) = MainModel::of(&a, Tier::Medium, Some(&own));
        assert_eq!(why, None);
        assert_eq!(main.name(), "openrouter:anthropic/claude-sonnet-5.5");
        assert_eq!(MainModel::of(&a, Tier::Medium, None), (MainModel::Tier(Tier::Medium), None), "none named: its tier");
        let p = profile_config(&a, "Juniper", Home::Chat, &main, DEFAULT_REASONING, &route("http://model.fragment.internal"), &["OPENROUTER_API_KEY".into()], Path::new("/c.sh"));
        let want = format!("model:\n  provider: \"custom\"\n  base_url: \"https://openrouter.ai/api/v1\"\n  default: \"anthropic/claude-sonnet-5.5\"\n  api_key: \"fck_openrouter_{tag}\"\n  default_headers:\n    x-fragment-agent: \"juniper--k3x9\"\n");
        assert!(p.contains(&want), "{p}");
        assert!(!p.contains("context_length"), "its provider says its context: {p}");
        assert!(p.contains("    base_url: \"http://model.fragment.internal/v1\"\n    model: \"vision\"\n"), "its eyes stay the route's: {p}");
        assert!(p.contains("    model: \"whisper\"\n"), "its ears too: {p}");
    }

    /// Valid: each credential's placeholder is in its environment
    /// variables, in the profile's `.env` (for Hermes and its tools) and in
    /// the terminal's credentials file; sandbox passthrough keeps allowed
    /// names and leaves Hermes' provider credentials out.
    #[test]
    fn a_profile_holds_its_credentials_placeholders() {
        let mut a = agent();
        let tag = "0123456789abcdef0123456789abcdef";
        a.credentials = vec![
            credential("perplexity", &["PERPLEXITY_API_KEY"], &format!("fck_perplexity_{tag}")),
            credential("google", &["GOOGLE_OAUTH_ACCESS_TOKEN"], &format!("fcx_google_{tag}")),
        ];
        let e = profile_env(&a);
        assert!(e.contains(&format!("\nPERPLEXITY_API_KEY=fck_perplexity_{tag}\n")) && e.contains(&format!("\nGOOGLE_OAUTH_ACCESS_TOKEN=fcx_google_{tag}\n")), "{e}");
        let s = credentials_sh(&a);
        assert!(s.contains(&format!("\nexport PERPLEXITY_API_KEY='fck_perplexity_{tag}'\n")), "{s}");
        let env = ["GOOGLE_OAUTH_ACCESS_TOKEN".to_string(), "PERPLEXITY_API_KEY".to_string(), "XAI_API_KEY".to_string()];
        let p = profile_config(&a, "Juniper", Home::Chat, &Tier::Medium.into(), DEFAULT_REASONING, &route("http://model.fragment.internal"), &env, Path::new("/c.sh"));
        assert!(p.contains("env_passthrough: [\"FRAGMENT_AS_AGENT\", \"FRAGMENT_FOR\", \"GOOGLE_OAUTH_ACCESS_TOKEN\"]"), "{p}");
    }

    /// Every pinned provider name, plus dynamic internal secrets, is
    /// refused; the skills' Google and X API placeholders still pass.
    #[test]
    fn no_blocklisted_name_is_in_profile_passthrough() {
        let mut env: Vec<String> = PROVIDER_ENV_BLOCKLIST.lines().map(str::to_string).collect();
        assert!(env.len() > 300, "the full pinned policy, not just today's offered providers");
        env.extend(["AUXILIARY_TEST_API_KEY", "AUXILIARY_TEST_BASE_URL", "GATEWAY_RELAY_TEST_TOKEN", "GATEWAY_RELAY_TEST_SECRET", "GATEWAY_RELAY_TEST_KEY", "GOOGLE_OAUTH_ACCESS_TOKEN", "X_API_BEARER_TOKEN"].map(str::to_string));
        let p = profile_config(&agent(), "Juniper", Home::Chat, &Tier::Medium.into(), DEFAULT_REASONING, &route("http://model.fragment.internal"), &env, Path::new("/c.sh"));
        let passed = p.lines().find(|l| l.starts_with("  env_passthrough:")).unwrap();
        assert_eq!(passed, "  env_passthrough: [\"FRAGMENT_AS_AGENT\", \"FRAGMENT_FOR\", \"GOOGLE_OAUTH_ACCESS_TOKEN\", \"X_API_BEARER_TOKEN\"]");
    }

    /// Invalid: a credential the platform sent out of shape (a name an image
    /// relies on, a value a line or a shell would read as more) is left out;
    /// a name given twice keeps the first.
    #[test]
    fn a_credential_out_of_shape_is_left_out() {
        let mut a = agent();
        let tag = "0123456789abcdef0123456789abcdef";
        a.credentials = vec![
            credential("a", &["FRAGMENT_AS_AGENT", "A_KEY"], &format!("fck_a_{tag}")),
            credential("b", &["B_KEY"], "fck_b_x'\nrm -rf /"),
            credential("c", &["lower"], &format!("fck_c_{tag}")),
            credential("d", &["A_KEY"], &format!("fck_d_{tag}")),
        ];
        assert_eq!(credential_vars(&a), vec![("A_KEY".to_string(), format!("fck_a_{tag}"))]);
        let p = profile_config(&a, "Juniper", Home::Chat, &Tier::Medium.into(), DEFAULT_REASONING, &route("http://m"), &["FRAGMENT_FOR".into(), "HERMES_HOME".into(), "A_KEY".into()], Path::new("/c.sh"));
        assert!(p.contains("env_passthrough: [\"FRAGMENT_AS_AGENT\", \"FRAGMENT_FOR\", \"A_KEY\"]"), "{p}");
        assert!(!credentials_sh(&a).contains("rm -rf"));
    }

    /// The gateway's control wire as `gateway/control_socket.py` speaks it:
    /// a request is one line; an answer's result, or its refusal.
    #[test]
    fn the_control_socket_wire() {
        let r: serde_json::Value = serde_json::from_str(control_request("rescan-profiles").trim_end()).unwrap();
        assert_eq!(r, serde_json::json!({ "verb": "rescan-profiles", "id": 1, "protocol": 1 }));
        assert!(control_request("status").ends_with('\n'), "one line");
        let ok = br#"{"ok": true, "protocol": 1, "result": {"multiplex": true, "added": ["maple--k3x9"], "served_profiles": ["default", "maple--k3x9"]}, "id": 1}"#;
        assert_eq!(control_answer(ok).unwrap()["added"][0], "maple--k3x9");
        assert_eq!(control_answer(br#"{"ok": false, "error": "unknown verb: 'x'", "protocol": 1}"#).unwrap_err(), "unknown verb: 'x'");
        assert!(control_answer(b"not json").is_err());
        assert!(control_answer(br#"{"ok": true, "result": null}"#).is_err());
        assert!(control_answer(&vec![b' '; CONTROL_ANSWER_MAX_BYTES + 1]).is_err());
    }

    #[test]
    #[should_panic(expected = "a control verb")]
    fn a_verb_out_of_shape_is_a_bug() {
        control_request("rescan profiles\n{");
    }

    #[test]
    #[should_panic(expected = "an agent's fragment and owner are names")]
    fn a_profile_env_of_no_names_is_a_bug() {
        let mut a = agent();
        a.owner = "npub1paul\nOPENAI_API_KEY=x".into();
        profile_env(&a);
    }

    #[test]
    #[should_panic(expected = "a Relay secret of 32 characters or more")]
    fn a_short_secret_is_refused() {
        gateway_env("127.0.0.1:1", "c", "short");
    }
}
