//! Session import on a real deployment (Paul's top priority, 2026-11-01): a
//! release check, run by name, that an export goes in as a person's move
//! would, with the deployment's own vendors (`cargo xtask e2e --hosted
//! --config <preview's config> --branch preview --only import-release
//! --max-paid-calls 100`); locally, on fakes, it proves itself.
//!
//! An e2e person, lent paid calls for their archive's search (embeddings
//! only: no chat model, no computer), logs the CLI in as `fragment login`
//! does and imports a synthetic old-Finite export (`export`: invented
//! throughout, one conversation long enough to cross an embeddings call's
//! 32 inputs). Its archive is made first under the run's label, so the
//! import files into it (`fragment session import` takes the person's own)
//! and `--sweep <run>` deletes it. Then, as History asks: the archive's
//! chats listed with their first words, and found by words inside their
//! replies (History's own search: lexical, chats); `fragment search`
//! finding a line by its meaning; and History in Chrome, drawn and
//! searched (`/history`, which makes no agent: Continue needs one, and so a
//! computer, which a release check of the import leaves out).
//!
//! It times the import (wall clock), counts the waits a rate limit made it
//! take ("Waiting N s for the search service…"), and writes both, with
//! History's screenshots, in its scratch directory (`import-release/`).

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::api::Api;
use crate::{Need, Suite};

const SECTION: &str = "import-release";
/// Paid calls the person is lent: their archive's embeddings (an
/// embeddings call takes at most 32 inputs, so about 10 for this export)
/// and the few questions asked by meaning, with room for a burst past the
/// ledger's gate.
const PAID_CALLS: u64 = 40;
/// The person the export is of, and its agent.
const PERSON: &str = "Robin";
const AGENT: &str = "Juniper";
/// The long conversation's exchanges: past one embeddings call's inputs.
const LONG_EXCHANGES: usize = 40;

/// The month-long move: a question and its answer an exchange, each a turn
/// (a section) of its own.
const MOVE: [Exchange; LONG_EXCHANGES] = [
    ("We signed the lease! Moving on the 30th. Where do I start?", "Start with what needs lead time: book movers, give notice, and schedule internet installation, which often takes two weeks."),
    ("Movers or rent a van?", "For a two-bedroom across town, movers save your back and a weekend. Get three quotes and ask about stairs and long carries."),
    ("Got quotes: 980, 1,250, 1,400.", "The 980 one is hourly and could end up higher. The 1,250 flat rate with insurance included is usually the safer pick."),
    ("Booked the 1,250 one.", "Put the date in the calendar and ask them to confirm by email the week before."),
    ("When should I start packing?", "Two weeks out for most things: books and out-of-season clothes first, the kitchen in the last three days."),
    ("How many boxes?", "Fifty to seventy for a two-bedroom: small ones for books, medium for most things, large for bedding."),
    ("What about the cat?", "Keep Miso in a closed bathroom with her bed, litter and water on moving day, with a sign on the door."),
    ("Utilities?", "Transfer electricity and gas for the 30th and shut off the old place on the 1st, so you always have power on moving day."),
    ("Internet install is only available on the 6th.", "Book it anyway; your phone's hotspot can carry you for a week."),
    ("Should I change my address everywhere at once?", "Set up mail forwarding first, then your bank, employer, insurance and subscriptions."),
    ("What goes in the essentials box?", "Toilet paper, a towel, chargers, basic tools, a kettle and mugs, snacks, the cat's food, medications and sheets."),
    ("The old place has to be spotless for the deposit.", "Photograph every room after cleaning, with timestamps, and do the oven and fridge the day before."),
    ("The landlord wants a walkthrough on the 31st.", "Bring your move-in photos and ask for the deposit return date in writing."),
    ("The new kitchen is tiny.", "Go vertical: a magnetic knife strip, hooks under the cabinets, a rolling cart beside the fridge."),
    ("What should I unpack first?", "Beds and the bathroom on night one, then the kitchen basics; everything else can wait."),
    ("The movers arrived late, but it's done!", "Order takeout, make the bed, and leave the boxes for tomorrow."),
    ("Miso is hiding under the bed.", "Normal for a cat after a move: keep food and litter near and she'll come out in a day or two."),
    ("Found a crack in the bathroom tile.", "Photograph it now and email the landlord today, so it's on record before you."),
    ("Internet is finally on.", "Run a speed test and put the router up high and central, not in a cupboard."),
    ("The building has a recycling room.", "Flatten the boxes and take them down in batches, or offer them to a neighbour who's moving."),
    ("The deposit came back in full!", "Those timestamped photos paid off."),
    ("How do I make it feel like home faster?", "Hang a few pictures and put up curtains in the first week; bare walls feel temporary."),
    ("Any housewarming ideas?", "Snacks, a few bottles and a two-hour window on a weekend afternoon; people love a tour."),
    ("How do I find a good doctor nearby?", "Check which practices near the new address are in your insurance network, then read reviews."),
    ("And a vet for Miso?", "Look for a cats-only clinic nearby: they're calmer for nervous cats."),
    ("The bedroom radiator clanks at night.", "Bleed it: with the heating off, open the valve at the top with a radiator key until water appears."),
    ("Bled it and it's quiet now.", "Fixing small things is the mark of a settled home."),
    ("Bike storage is in the basement; is that safe?", "Use a U-lock through the frame and back wheel even indoors, and photograph the serial number."),
    ("Should I get renter's insurance here?", "Yes: it's cheap and covers your belongings and liability. Update the address if you have a policy."),
    ("The smoke alarm chirps every minute.", "That's a low battery: swap it for a fresh nine-volt and press test."),
    ("Where do I put the spare keys?", "With a trusted neighbour or a lockbox, never under the mat."),
    ("Parking permit?", "Apply with the city using the lease as proof of address; it usually takes a week."),
    ("The shower pressure is weak.", "Unscrew the shower head and soak it in vinegar overnight to clear the scale."),
    ("Noisy neighbours upstairs.", "Introduce yourself first; a friendly word works better than a complaint to the landlord."),
    ("Which room for the home office?", "The one with daylight and a door you can close: calls go better behind a door."),
    ("Plants for a north-facing window?", "Pothos, snake plants and ZZ plants take low light well."),
    ("How do I get rid of the cardboard smell?", "Air the rooms daily and leave a bowl of baking soda in the closets."),
    ("Last box unpacked!", "A month to the day. Congratulations on the move."),
    ("What should I do with the leftover packing paper?", "Keep a stack for the next move or gifts; the rest goes to recycling."),
    ("Thanks for helping all month.", "It was a pleasure. Enjoy the new place, and give Miso a scratch from me."),
];

/// A question and its answer.
type Exchange = (&'static str, &'static str);

/// The short chats: an id, a title, and its exchanges.
const CHATS: [(&str, &str, &[Exchange]); 5] = [
    ("20260709_190233_a41d", "Weeknight dinners", &[
        ("I need three dinners I can make in 30 minutes after work. My partner is lactose intolerant.", "Chickpea curry made creamy with a tin of coconut milk, sheet-pan gnocchi with tomatoes, and peanut noodles with lime."),
        ("Can I make the curry ahead?", "Yes, it's better the next day; stir the spinach in when you reheat."),
    ]),
    ("20260714_072010_5be2", "Sourdough starter help", &[
        ("My starter smells like nail polish remover and isn't rising.", "It's hungry, not dead: feed it twice a day for a few days, with some rye flour to wake it."),
        ("It doubled in six hours today!", "That's a healthy starter: bake with it at its peak."),
    ]),
    ("20260718_201540_c9f0", "Trip to Lisbon in May", &[
        ("Where should we stay for a week in Lisbon?", "Alfama, the old quarter: steep, full of fado bars, quiet at night."),
        ("Getting around?", "Walk and take tram 28 early in the morning, before it fills."),
        ("Food we shouldn't miss?", "Warm pastéis de nata, grilled sardines and a seafood rice at a tasca."),
    ]),
    ("20260805_120745_e210", "Building an emergency fund", &[
        ("How much should we keep for emergencies?", "Three to six months of essential expenses, in a high-yield savings account apart from checking."),
        ("We have 6,000 now.", "Automate 600 a month and you'll reach three months' worth in about nine months."),
    ]),
    ("20260708_101233_ab01", "Borrow checker trouble", &[
        ("Rust won't let me call self.update while iterating over self.items.", "The loop borrows self.items while update wants all of self: split the cache into its own struct."),
    ]),
];

/// A month into the move, the turn the long conversation's second
/// embeddings call carries (past its first 32 inputs).
const LATE_TURN: usize = 33;
const _: () = assert!(LONG_EXCHANGES > 32 && LATE_TURN >= 32 && LATE_TURN < LONG_EXCHANGES, "the long conversation crosses an embeddings call, its late turn in the second");

/// A conversation as the old Finite's export writes it.
fn chat(id: &str, title: &str, at: &str, said: &[Exchange]) -> Value {
    let start = chrono_like(at);
    let messages: Vec<Value> = said
        .iter()
        .enumerate()
        .flat_map(|(i, (q, a))| {
            let t = start + i as i64 * 3600;
            [json!({ "role": "user", "at": iso(t), "text": q }), json!({ "role": "assistant", "at": iso(t + 60), "text": a })]
        })
        .collect();
    let end = messages.last().and_then(|m| m["at"].as_str()).unwrap_or(at).to_string();
    json!({ "format": "finite-chat-archive/1", "id": id, "file": format!("chats/sessions/{id}.json"), "title": title, "kind": "chat", "source": "finitechat",
        "with": PERSON, "started_at": at, "ended_at": end, "messages": messages, "sessions": [id] })
}

/// `YYYY-MM-DDTHH:MM:SS.000Z` of a second since the epoch.
fn iso(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The second since the epoch of a conversation id's `YYYYMMDD_HHMMSS`.
fn chrono_like(id_or_at: &str) -> i64 {
    let digits: String = id_or_at.chars().filter(char::is_ascii_digit).take(14).collect();
    let n = |a: usize, b: usize| digits[a..b].parse::<i64>().unwrap_or(0);
    let (y, m, d) = (n(0, 4), n(4, 6), n(6, 8));
    // days from civil (the inverse of `iso`'s)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400 + n(8, 10) * 3600 + n(10, 12) * 60 + n(12, 14)
}

/// The synthetic export (`chats/`, `finite-chat-archive/1`), one agent's:
/// the long move and the short chats. Answers how many chats it holds.
fn export(root: &Path) -> Result<usize> {
    let chats = root.join("chats");
    std::fs::create_dir_all(chats.join("sessions"))?;
    let mut all = vec![chat("20260828_190500_mv01", "Moving to the new apartment", &iso(chrono_like("20260828_190500")), &MOVE)];
    all.extend(CHATS.iter().map(|(id, title, said)| chat(id, title, &iso(chrono_like(id)), said)));
    let mut listed = Vec::new();
    for c in &all {
        let id = c["id"].as_str().context("an id")?;
        std::fs::write(chats.join(format!("sessions/{id}.json")), c.to_string())?;
        listed.push(json!({ "id": id, "file": c["file"], "title": c["title"], "kind": "chat" }));
    }
    std::fs::write(chats.join("index.json"), json!({ "format": "finite-chat-archive/1", "agent": AGENT, "conversations": listed }).to_string())?;
    Ok(all.len())
}

/// `fragment login` in `home` for an e2e person lent `paid` paid calls: the
/// CLI's pending login, the levers' sign-in of the email it names (hosted;
/// the WorkOS fake's locally), its code typed and its key added, then the
/// login finishing. Answers the person's browser session.
fn login_paying(s: &mut Suite, api: &Api, home: &Path, paid: u64) -> Result<String> {
    let pending = s.cli_json(api, home, &["login", "--no-wait", "--json"])?;
    let npub = pending["npub"].as_str().context("a pending login names its key")?;
    let code = pending["code"].as_str().context("a pending login answers its code")?;
    let email = crate::cli_email(npub)?;
    let session = match api.signs_in_by_levers() {
        true => api.e2e_sign_in(&email, paid)?.0,
        false => api.sign_in(&email)?,
    };
    let typed = api.type_code(&session, code)?;
    anyhow::ensure!(typed.status == 200, "typing the code: {typed}");
    let added = api.add_code(&session, code)?;
    anyhow::ensure!(added.status == 200, "adding its key: {added}");
    let done = s.cli(api, home, &["login", "--no-browser"]);
    anyhow::ensure!(done.status.success(), "fragment login: {}", String::from_utf8_lossy(&done.stderr));
    Ok(session)
}

/// The seconds of each wait a rate limit made the import take, as it said
/// them ("Waiting 37 s for the search service…").
fn waits(stderr: &str) -> Vec<u64> {
    stderr.lines().filter_map(|l| l.trim().strip_prefix("Waiting ")?.split_once(" s for the search service")?.0.parse().ok()).collect()
}

pub fn import_release(s: &mut Suite, api: &Api) -> Result<()> {
    let why = format!("it imports a synthetic export with the deployment's own vendors (its search's embeddings, at most {PAID_CALLS} paid calls) and times it");
    if !s.section_by_name(SECTION, &[Need::Levers, Need::Models, Need::Chrome], &why) {
        return Ok(());
    }
    let out = s.dir(SECTION);
    let home = s.dir("import-release-home");
    let root = s.dir("import-release-export");
    let chats = export(&root)?;
    let session = login_paying(s, api, &home, PAID_CALLS)?;
    let keys = s.cli_keys(&home).context("the CLI logged in")?;
    // its archive, the run's: the import files into it, the sweep deletes it
    let made = api.create_with(&keys, json!({ "label": s.name("sessions"), "template": "sessions", "title": "Session Archive" }))?;
    anyhow::ensure!(made.status == 200, "the archive made: {made}");
    s.owned(&made.body, &keys);
    let archive = made.body["name"].as_str().context("its name")?.to_string();

    // ---- the import, timed, its rate limit's waits counted
    let t0 = Instant::now();
    let run = s.cli(api, &home, &["session", "import", root.to_str().context("a utf-8 path")?, "--json"]);
    let took = t0.elapsed();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let waited = waits(&stderr);
    let imported: Value = serde_json::from_slice(&run.stdout).with_context(|| format!("the import's answer: {}{stderr}", String::from_utf8_lossy(&run.stdout)))?;
    let r = &imported["data"];
    s.ok(
        "an export is imported into its person's archive, every chat filed and written to search, none refused",
        imported["ok"] == true && r["archive"] == archive.as_str() && r["added"] == chats && r["refused"] == json!([])
            && r["search"]["state"] == "written" && r["search"]["written"] == chats && r["search"]["refused_count"] == 0 && r["search"]["unresolved"] == 0,
        json!({ "answer": imported, "stderr": stderr }),
    );
    let sections: usize = LONG_EXCHANGES + CHATS.iter().map(|(_, _, said)| said.len()).sum::<usize>();
    let timings = json!({ "chats": chats, "sections": sections, "import_ms": took.as_millis() as u64, "rate_limit_waits": waited.len(), "rate_limit_waited_s": waited.iter().sum::<u64>(), "deployment": api.base });
    std::fs::write(out.join("timings.json"), serde_json::to_vec_pretty(&timings)?)?;
    println!("      imported {chats} chats ({sections} sections) in {:.1?}; {} rate-limit waits ({} s)", took, waited.len(), waited.iter().sum::<u64>());

    // ---- as History asks: the chats with their first words, and found by
    // words inside their replies (lexical, chats: session-archive.js)
    let index = api.signed(&keys, "GET", &format!("/api/f/{archive}/file?path={}", "index%2Fchat.json"), None)?;
    let entries = index.body["sessions"].as_array().cloned().unwrap_or_default();
    s.ok(
        "History's list: the archive's index names every chat, each with its first words",
        index.status == 200 && entries.len() == chats && entries.iter().all(|e| e["preview"].as_str().is_some_and(|p| !p.is_empty())),
        json!({ "status": index.status, "entries": entries.iter().map(|e| json!([e["title"], e["preview"]])).collect::<Vec<_>>() }),
    );
    let found = api.signed(&keys, "POST", &format!("/api/f/{archive}/search"), Some(&json!({ "q": "coconut", "limit": 50, "mode": "lexical", "fields": { "kind": "chat" } })))?;
    let hit = found.body["results"][0].clone();
    s.ok(
        "History's search: a word only inside a reply finds its chat, with a snippet",
        found.status == 200 && hit["source"] == "sessions/hermes-20260709_190233_a41d.json" && hit["snippet"].as_str().is_some_and(|t| t.contains("coconut")),
        &found.body,
    );

    // ---- `fragment search` by meaning: a dinner with no dairy finds the
    // curry; a late turn of the long move (its second embeddings call) is
    // found by what it means too
    let by_meaning = |q: &str| s.cli_json(api, &home, &["search", q, "--in", &archive, "--mode", "semantic", "--limit", "3", "--json"]);
    let dinner = by_meaning("a weeknight dinner with no milk or cheese")?;
    let sources = |v: &Value| v["results"].as_array().into_iter().flatten().map(|r| r["source"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
    let late = by_meaning(MOVE[LATE_TURN].0)?;
    let late_hit = late["results"].as_array().into_iter().flatten().any(|r| r["source"] == "sessions/hermes-20260828_190500_mv01.json" && r["section"] == format!("t{LATE_TURN}").as_str());
    // a fake's embeddings mean nothing: there, that it answered by meaning
    let meant = |v: &Value| v["semantic"] == "on" && v["fragments"][0]["state"] == "searched";
    s.ok(
        "fragment search finds a line by its meaning, a late turn of a long conversation (its second embeddings call) included",
        meant(&dinner) && meant(&late) && (s.on_fakes() || (sources(&dinner).contains(&"sessions/hermes-20260709_190233_a41d.json".to_string()) && late_hit)),
        json!({ "dinner": sources(&dinner), "late": late["results"].as_array().map(|r| r.iter().map(|x| json!([x["source"], x["section"]])).collect::<Vec<_>>()) }),
    );

    // ---- History in Chrome: drawn and searched (no agent made: `/history`)
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for the import's History (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let wait = Duration::from_secs(30);
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/history", api.base))?;
    b.viewport(&page, 1400, 900, false)?;
    b.until(&page, "!!document.querySelector('.history-filter[data-filter=\"imported\"]')?.offsetParent", wait);
    b.click(&page, ".history-filter[data-filter=\"imported\"]")?;
    let imported_rows = "document.querySelectorAll('.history-row[data-kind=\"imported\"]')";
    let listed = b.until(&page, &format!("{imported_rows}.length === {chats}"), wait);
    b.screenshot(&page, &out.join("history.png"))?;
    b.click(&page, ".history-search input")?;
    b.type_text(&page, "Miso")?;
    let searched = b.until(&page, &format!("{imported_rows}.length === 1 && !!document.querySelector('.history-row[data-key=\"imported:hermes-20260828_190500_mv01\"] .history-preview.found')"), wait);
    b.screenshot(&page, &out.join("history-search.png"))?;
    let drawn = b.eval(&page, &format!("[...{imported_rows}].map((r) => r.querySelector('.history-line .label')?.textContent + ' | ' + (r.querySelector('.history-preview')?.textContent ?? ''))"))?;
    s.ok("History in Chrome lists the imported chats, and its search finds one by a word in its replies, with the snippet", listed && searched, &drawn);
    println!("      History's screenshots and timings: {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_dates_go_there_and_back() {
        assert_eq!(iso(chrono_like("20260828_190500")), "2026-08-28T19:05:00.000Z");
        assert_eq!(iso(chrono_like("20240229_235959")), "2024-02-29T23:59:59.000Z");
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_rate_limits_waits_are_read_as_said() {
        assert_eq!(waits("Waiting 37 s for the search service…\nother\n  Waiting 1 s for the search service…"), [37, 1]);
        assert!(waits("Search: 6 written").is_empty());
    }
}
