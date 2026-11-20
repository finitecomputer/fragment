//! The calls each operator key is lent for (hardening, 2026-11-19): an
//! operator key is the platform's one shared key at its vendor, so the
//! swap puts it only on a request whose method and path one of the
//! platform's own features makes (Hermes' native tools, the skills and
//! helpers the images carry, and the calls the price book meters,
//! `price::key_usage`). Any other placeholder for it is refused, reaching
//! no provider (`swap::SwapError::NotLent`), as one sent to a host not its
//! provider's is: an account-wide call (its history, its lists, a delete)
//! is no call of an agent's.
//!
//! The lists are code, not the catalog's: a deployment's row names the
//! vendor's hosts and where its key goes, and these are what the
//! platform's own callers send there, which change with those callers.
//! An operator row this table does not name is lent for nothing (a hard
//! cut: its calls are refused until its routes are listed here).
//!
//! Each route's source is beside it; the Hermes ones are of the image's
//! pinned Hermes (`images/hermes/Dockerfile`, rc.13-v0.23.2) and the SDKs
//! its lock pins.

/// One kind of call: its methods, the hosts it goes to (none: any of its
/// row's), and its path, segment by segment, a `*` one segment of an id's
/// characters (`id_segment`). A path is matched whole; its query is not
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub methods: &'static [&'static str],
    pub hosts: &'static [&'static str],
    pub path: &'static str,
}

const GET: &[&str] = &["GET"];
const POST: &[&str] = &["POST"];
const PATCH: &[&str] = &["PATCH"];
const ANY_HOST: &[&str] = &[];

const fn route(methods: &'static [&'static str], path: &'static str) -> Route {
    Route { methods, hosts: ANY_HOST, path }
}

const fn at(methods: &'static [&'static str], hosts: &'static [&'static str], path: &'static str) -> Route {
    Route { methods, hosts, path }
}

/// Firecrawl (api.firecrawl.dev): Hermes' `web_search`, `web_extract` and
/// its scanned-PDF reading (hermes.rs sets Firecrawl as the web backend).
const FIRECRAWL: &[Route] = &[
    // hermes plugins/web/firecrawl/provider.py:361 → firecrawl-py 4.53.2 v2/methods/search.py:32
    route(POST, "/v2/search"),
    // hermes plugins/web/firecrawl/provider.py:306 → firecrawl-py v2/methods/scrape.py:76; metered (price::key_usage)
    route(POST, "/v2/scrape"),
    // hermes tools/read_extract.py:160,206 → firecrawl-anydoc 0.2.4 __init__.py:112
    route(POST, "/v2/parse"),
    // metered (price::key_usage: v1 and v2 scrape and map, by their credits)
    route(POST, "/v1/scrape"),
    route(POST, "/v1/map"),
    route(POST, "/v2/map"),
];

/// Perplexity (api.perplexity.ai): Hermes' web search, and its extract,
/// on a deployment that offers Perplexity alone.
const PERPLEXITY: &[Route] = &[
    // hermes plugins/web/perplexity/provider.py:107,234
    route(POST, "/search"),
    // hermes plugins/web/perplexity/provider.py:255; tools/web_tools.py:122
    route(POST, "/sdk/content/snippets"),
];

const FAL_QUEUE: &[&str] = &["queue.fal.run"];
const FAL_RUN: &[&str] = &["queue.fal.run", "fal.run"];

/// fal (queue.fal.run, fal.run): Hermes' `image_generate` on FLUX.2
/// Klein, its edit and its upscaler, through the queue (submit, status,
/// result), and the music skill's MiniMax. Nothing at rest.fal.ai.
const FAL: &[Route] = &[
    // hermes tools/image_generation_catalog.py:41 (queue: image_generation_tool.py:128 → fal-client 0.14.1 client.py:2303);
    // fal.run's synchronous run is metered (price::FAL_KLEIN_RUN)
    at(POST, FAL_RUN, "/fal-ai/flux-2/klein/9b"),
    // hermes tools/image_generation_catalog.py:49; image_generation_tool.py:483
    at(POST, FAL_RUN, "/fal-ai/flux-2/klein/9b/edit"),
    // hermes tools/image_generation_catalog.py:390; plugins/image_gen/fal/__init__.py:59
    at(POST, FAL_RUN, "/fal-ai/clarity-upscaler"),
    // templates/skills/skills/music-generation-finite/SKILL.md:96 (fal.run), :71 (its client, the queue)
    at(POST, FAL_RUN, "/fal-ai/minimax-music/v2.5"),
    // a queued request's status and result: hermes tools/fal_common.py:50 → fal-client client.py:1515,1546
    at(GET, FAL_QUEUE, "/fal-ai/*/requests/*/status"),
    at(GET, FAL_QUEUE, "/fal-ai/*/requests/*"),
];

/// Browser Use (api.browser-use.com): `browser-harness`'s remote daemon
/// (computer.md, its start and stop), and the starts the meter knows.
const BROWSER_USE: &[Route] = &[
    // browser-harness 0.1.13 admin.py:1019 (start_remote_daemon); metered (price::key_usage)
    route(POST, "/api/v3/browsers"),
    // browser-harness admin.py:903, daemon.py:356 (stop_remote_daemon: {"action":"stop"})
    route(PATCH, "/api/v3/browsers/*"),
    // metered (price::key_usage: a task on a session it has, v4's)
    route(POST, "/api/v4/runs"),
];

/// Google Places (New) (places.googleapis.com): no caller in the images
/// yet; the reads the meter prices (price::key_usage), each of public
/// places, never the account's.
const GOOGLE_PLACES: &[Route] = &[
    route(POST, "/v1/places:searchText"),
    route(POST, "/v1/places:searchNearby"),
    route(POST, "/v1/places:autocomplete"),
    route(GET, "/v1/places/*"),
    route(GET, "/v1/places/*/photos/*/media"),
];

/// xAI (api.x.ai): Hermes' `x_search`, and the calls the meter prices.
const XAI: &[Route] = &[
    // hermes tools/x_search_tool.py:239; metered (price::key_usage)
    route(POST, "/v1/responses"),
    // metered (price::key_usage: an x_search chat, a plain one nothing)
    route(POST, "/v1/chat/completions"),
    // metered (price::key_usage: grok-imagine-image)
    route(POST, "/v1/images/generations"),
];

/// X (api.x.com, api.twitter.com): the `x-api-finite` skill's helper.
const X: &[Route] = &[
    // templates/skills/skills/social-media/x-api-finite/x-api.py:232
    route(GET, "/2/tweets"),
    // x-api.py:248,276; metered (price::key_usage)
    route(GET, "/2/tweets/search/recent"),
    // metered (price::key_usage)
    route(GET, "/2/tweets/search/all"),
    // x-api.py:266
    route(GET, "/2/tweets/*"),
    // x-api.py:294
    route(GET, "/2/users/by/username/*"),
];

/// ElevenLabs (api.elevenlabs.io): Hermes' `text_to_speech` and its voice
/// search, and the music skill.
const ELEVENLABS: &[Route] = &[
    // hermes tools/tts_tool_providers.py:223, tts_streaming.py:245 → elevenlabs 1.59.0 text_to_speech/client.py:168
    route(POST, "/v1/text-to-speech/*"),
    // metered with it (price::key_usage: its streaming and with-timestamps calls)
    route(POST, "/v1/text-to-speech/*/stream"),
    route(POST, "/v1/text-to-speech/*/with-timestamps"),
    route(POST, "/v1/text-to-speech/*/stream/with-timestamps"),
    // hermes tools/tts_elevenlabs_voices.py:105,138 (find_voices, the library)
    route(GET, "/v1/shared-voices"),
    // hermes tools/tts_elevenlabs_voices.py:96 (find_voices, source "account")
    route(GET, "/v2/voices"),
    // templates/skills/skills/music-generation-finite/SKILL.md:124,135,148
    route(POST, "/v1/music"),
    route(POST, "/v1/music/plan"),
];

/// Each operator key's routes, by its catalog row's name.
const ROUTES: [(&str, &[Route]); 8] = [
    ("firecrawl", FIRECRAWL),
    ("perplexity", PERPLEXITY),
    ("fal", FAL),
    ("browser-use", BROWSER_USE),
    ("google-places", GOOGLE_PLACES),
    ("xai", XAI),
    ("x", X),
    ("elevenlabs", ELEVENLABS),
];

/// The longest id a `*` segment matches.
pub const ID_SEGMENT_MAX: usize = 256;

/// Whether `segment` is one an id may be: 1 to `ID_SEGMENT_MAX` letters,
/// digits, `-` and `_`. No `%` (an escaped `/` or `.` a vendor might read
/// as a separator), no `.`, nothing empty.
fn id_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.len() <= ID_SEGMENT_MAX && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Route {
    /// Whether a `method` request to `host` at `path` is this call.
    pub fn matches(&self, host: &str, method: &str, path: &str) -> bool {
        if !self.methods.contains(&method) || !(self.hosts.is_empty() || self.hosts.contains(&host)) {
            return false;
        }
        let (mut want, mut got) = (self.path.split('/'), path.split('/'));
        loop {
            match (want.next(), got.next()) {
                (None, None) => return true,
                (Some("*"), Some(segment)) if id_segment(segment) => {}
                (Some(w), Some(g)) if w != "*" && w == g => {}
                _ => return false,
            }
        }
    }
}

/// The routes operator key `provider` is lent for; none for an operator
/// row this table does not name.
pub fn routes(provider: &str) -> Option<&'static [Route]> {
    ROUTES.iter().find(|(name, _)| *name == provider).map(|(_, routes)| *routes)
}

/// Whether operator key `provider` is lent for a `method` request to
/// `host` at `path` (the host is its row's: the swap checks that first).
pub fn lent(provider: &str, host: &str, method: &str, path: &str) -> bool {
    routes(provider).is_some_and(|routes| routes.iter().any(|r| r.matches(host, method, path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid: each feature's own call is lent the key, an id in its place.
    #[test]
    fn a_features_own_call_is_lent() {
        for (provider, host, method, path) in [
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb"),
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/text-to-speech/e2e-voice/stream"),
            ("elevenlabs", "api.elevenlabs.io", "GET", "/v1/shared-voices"),
            ("browser-use", "api.browser-use.com", "POST", "/api/v3/browsers"),
            ("browser-use", "api.browser-use.com", "PATCH", "/api/v3/browsers/0b6a3c2e-1f00-4c8e-9d1a-5f8e0c7b2a11"),
            ("fal", "queue.fal.run", "POST", "/fal-ai/flux-2/klein/9b"),
            ("fal", "fal.run", "POST", "/fal-ai/flux-2/klein/9b"),
            ("fal", "queue.fal.run", "GET", "/fal-ai/flux-2/requests/764cabcf-b745-4b3e-ae38-1200304cf45b/status"),
            ("fal", "queue.fal.run", "GET", "/fal-ai/flux-2/requests/764cabcf-b745-4b3e-ae38-1200304cf45b"),
            ("firecrawl", "api.firecrawl.dev", "POST", "/v2/scrape"),
            ("perplexity", "api.perplexity.ai", "POST", "/search"),
            ("google-places", "places.googleapis.com", "GET", "/v1/places/ChIJe2e/photos/AUe2e/media"),
            ("xai", "api.x.ai", "POST", "/v1/responses"),
            ("x", "api.x.com", "GET", "/2/tweets/search/recent"),
            ("x", "api.twitter.com", "GET", "/2/users/by/username/finite_chat"),
        ] {
            assert!(lent(provider, host, method, path), "{provider} {method} {host}{path}");
        }
    }

    /// Invalid: an account-wide call, another method, a path past or short
    /// of a route's, an id that is no id, a host a route does not name,
    /// and a provider the table does not name are lent nothing.
    #[test]
    fn anything_else_is_lent_nothing() {
        for (provider, host, method, path) in [
            ("elevenlabs", "api.elevenlabs.io", "GET", "/v1/history"),
            ("elevenlabs", "api.elevenlabs.io", "DELETE", "/v1/history/abc"),
            ("elevenlabs", "api.elevenlabs.io", "GET", "/v1/user"),
            ("elevenlabs", "api.elevenlabs.io", "GET", "/v1/voices"),
            ("elevenlabs", "api.elevenlabs.io", "DELETE", "/v1/text-to-speech/abc"),
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/text-to-speech/"),
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/text-to-speech"),
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/text-to-speech/a%2F..%2Fhistory"),
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/text-to-speech/a.b"),
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/text-to-speech/abc/stream/extra"),
            ("elevenlabs", "api.elevenlabs.io", "POST", "/v1/music/"),
            ("browser-use", "api.browser-use.com", "GET", "/api/v2/tasks"),
            ("browser-use", "api.browser-use.com", "GET", "/api/v2/sessions"),
            ("browser-use", "api.browser-use.com", "GET", "/api/v3/browsers"),
            ("browser-use", "api.browser-use.com", "GET", "/api/v3/browsers/abc"),
            ("browser-use", "api.browser-use.com", "GET", "/api/v3/profiles"),
            ("fal", "rest.fal.ai", "GET", "/tokens/"),
            ("fal", "fal.run", "GET", "/fal-ai/flux-2/requests/abc"),
            ("fal", "queue.fal.run", "PUT", "/fal-ai/flux-2/requests/abc/cancel"),
            ("fal", "queue.fal.run", "POST", "/fal-ai/some-other-model"),
            ("firecrawl", "api.firecrawl.dev", "GET", "/v1/team/credit-usage"),
            ("firecrawl", "api.firecrawl.dev", "GET", "/v2/crawl/active"),
            ("perplexity", "api.perplexity.ai", "GET", "/async/chat/completions"),
            ("xai", "api.x.ai", "GET", "/v1/files"),
            ("xai", "api.x.ai", "GET", "/v1/api-key"),
            ("x", "api.x.com", "DELETE", "/2/tweets/20"),
            ("x", "api.x.com", "GET", "/2/usage/tweets"),
            ("x", "api.x.com", "GET", "/2/tweets/search/stream/rules"),
            ("nonesuch", "api.nonesuch.test", "GET", "/"),
        ] {
            assert!(!lent(provider, host, method, path), "{provider} {method} {host}{path}");
        }
        assert!(!id_segment(&"a".repeat(ID_SEGMENT_MAX + 1)) && id_segment(&"a".repeat(ID_SEGMENT_MAX)));
    }

    /// Each route is well formed: a path from `/` with no empty segment, a
    /// method, and a host only among its row's (deploy/example.jsonc).
    #[test]
    fn every_route_is_well_formed() {
        let hosts = [
            ("firecrawl", &["api.firecrawl.dev"][..]),
            ("perplexity", &["api.perplexity.ai"]),
            ("fal", &["queue.fal.run", "fal.run", "rest.fal.ai"]),
            ("browser-use", &["api.browser-use.com"]),
            ("google-places", &["places.googleapis.com"]),
            ("xai", &["api.x.ai"]),
            ("x", &["api.x.com", "api.twitter.com"]),
            ("elevenlabs", &["api.elevenlabs.io"]),
        ];
        for (name, routes) in ROUTES {
            let row_hosts = hosts.iter().find(|(n, _)| *n == name).map(|(_, h)| *h).expect("each listed provider is an example row");
            for r in routes {
                assert!(r.path.starts_with('/') && !r.path[1..].split('/').any(str::is_empty), "{name} {}", r.path);
                assert!(!r.methods.is_empty() && r.hosts.iter().all(|h| row_hosts.contains(h)), "{name} {}", r.path);
            }
        }
    }
}
