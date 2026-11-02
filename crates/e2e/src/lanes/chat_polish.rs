//! Rendering and history on a quiet chat, so pagination fixtures start no
//! agent turns. Uses real channel writes, blobs, and the released page.
use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;
use crate::api::{Api, Call};
use crate::browser::Browser;
use crate::Suite;
use fragment_nip98::Keys;

/// The PDF the media message attaches.
const PREVIEW_PDF: &[u8] = include_bytes!("../../fixtures/chat-preview.pdf");

/// The chat with its PDF in the platform's page, framed as the shell frames
/// it: through the platform's mint (`/auth/frame`), signed in by the
/// frame's own partitioned cookie, with no cookie of the chat's origin
/// (a person who never opened the chat in a tab of its own: Paul,
/// 2026-10-23, whose PDF showed the browser's refused frame, and whose
/// Download failed, "file wasn't available on site", while opening the
/// file in a tab worked). Valid: the attachment's view shows the PDF, in
/// a frame of the chat's own; its Download, pressed there, saves the PDF
/// byte for byte.
fn in_the_platforms_frame(s: &mut Suite, api: &Api, chrome: &mut Browser, name: &str) -> Result<()> {
    let page = chrome.open(&format!("{}/settings", api.base))?;
    chrome.viewport(&page, 1280, 860, false)?;
    let loaded = format!("location.origin === {} && document.readyState === 'complete'", super::js(&api.base));
    anyhow::ensure!(super::shows(chrome, &page, &loaded), "the platform's page loads");
    let src = format!("{}/auth/frame?name={name}&return=/", api.base);
    let frame = format!(
        "(() => {{ const f = document.createElement('iframe'); f.src = {}; f.allow = 'clipboard-write; microphone'; \
         f.style.cssText = 'position: fixed; inset: 0; width: 100%; height: 100%; border: 0; z-index: 2147483647; background: #fff'; \
         document.body.append(f); return true; }})()",
        super::js(&src)
    );
    chrome.eval(&page, &frame)?;
    let ready = "document.getElementById('say')?.dataset.ready === '1' && !!document.querySelector('.attachment-download[aria-label=\"Download preview.pdf\"]')";
    let wait = Duration::from_secs(30);
    if !s.eventually(wait, || chrome.eval_in_frame(&page, name, ready).ok() == Some(Value::Bool(true))) {
        let seen = chrome.eval_in_frame(&page, name, "location.href + ' ' + (document.body?.innerText ?? '').slice(0, 300)").unwrap_or_default();
        anyhow::bail!("the chat in the platform's frame is ready: {seen}");
    }

    // its view: the PDF in a frame of the chat's own (a refused frame is
    // the browser's error page, of no origin the chat may read)
    let pdf = "(() => { const f = document.querySelector('.attachment-pdf[title=\"preview.pdf\"]'); if (!f) return 'none'; const d = f.contentDocument; return d ? d.contentType : 'refused'; })()";
    let shown = s.eventually(wait, || chrome.eval_in_frame(&page, name, &format!("{pdf} === 'application/pdf'")).ok() == Some(Value::Bool(true)));
    let seen = chrome.eval_in_frame(&page, name, pdf)?;
    chrome.screenshot(&page, &s.dir("chat-files").join("pdf-in-the-platforms-frame.png"))?;
    s.ok("in the platform's frame, as the shell shows a chat, a PDF attachment's view shows the PDF", shown, &seen);

    // its Download, as a person presses it (the frame fills the page)
    let downloads = s.dir("chat-files-downloads");
    chrome.allow_downloads(&downloads)?;
    let at = chrome.eval_in_frame(
        &page,
        name,
        "(() => { const a = document.querySelector('.attachment-download[aria-label=\"Download preview.pdf\"]'); a.scrollIntoView({ block: 'nearest' }); const r = a.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()",
    )?;
    let (Some(x), Some(y)) = (at[0].as_f64(), at[1].as_f64()) else { anyhow::bail!("no Download to press: {at}") };
    chrome.click_at(&page, (x, y))?;
    let saved = s.eventually(wait, || std::fs::read(downloads.join("preview.pdf")).ok().as_deref() == Some(PREVIEW_PDF));
    let there: Vec<String> = std::fs::read_dir(&downloads).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    s.ok("in the platform's frame, as the shell shows a chat, a file's Download saves it byte for byte, signed in by the frame's cookie alone", saved, json!({ "saved": there }));
    chrome.close(page)?;
    Ok(())
}

pub(super) fn check(s: &mut Suite, api: &Api, owner: &Keys, session: &str, chrome: &mut Browser, label: &str) -> Result<()> {
    let name = s.named(api, owner, "chat-polish")?;
    let made = api.create_with(owner, json!({ "name": name, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the rendering chat: {made}");
    s.owned(&made.body, owner);
    // the chat's agent, which runs nowhere: its records are its own
    let (hand, agent) = quiet_agent(api, owner, &name)?;
    let agent = agent.as_str();
    let mut attachments = vec![];
    for (filename, mime, bytes) in [
        ("clip.mp4", "video/mp4", include_bytes!("../../fixtures/chat-video.mp4").as_slice()),
        ("preview.pdf", "application/pdf", PREVIEW_PDF),
    ] {
        use sha2::{Digest, Sha256};
        let sha = hex::encode(Sha256::digest(bytes));
        let uploaded = api.call(Call {
            method: "PUT", url: format!("{}/api/f/{name}/blobs/{sha}", api.base),
            body: Some(bytes.to_vec()), content_type: Some(mime), keys: Some(owner), ..Call::default()
        })?;
        anyhow::ensure!(uploaded.status == 200, "uploading {filename}: {uploaded}");
        attachments.push(json!({ "sha256": sha, "type": mime, "size": bytes.len(), "name": filename }));
    }
    let posted = api.signed(owner, "POST", &format!("/api/f/{name}/channels/chat"), Some(&json!({ "id": "media", "body": { "text": "Video and PDF", "attachments": attachments } })))?;
    anyhow::ensure!(posted.status == 200, "posting media: {posted}");
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", session)?;
    // (before any page of the chat's own: its origin has no cookie yet)
    in_the_platforms_frame(s, api, chrome, &name)?;
    let page = chrome.open(&api.site_url(&name, "__signin?return=/"))?;
    super::fit(chrome, &page, 1280, 860, false)?;
    s.ok("a video attachment is a labelled inline player with loaded metadata", super::shows(chrome, &page, "!!document.querySelector('.attachment-video video[controls][aria-label=\"clip.mp4\"]') && document.querySelector('video').readyState >= 1"), "");
    s.ok("a PDF attachment has an inline view and open/download/share actions", super::shows(chrome, &page, "!!document.querySelector('.attachment-pdf[title=\"preview.pdf\"]') && document.querySelectorAll('.attachment-download[download]').length === 2 && document.querySelectorAll('.attachment-share').length === 2"), "");
    let shots = s.dir("chat-polish");
    chrome.screenshot(&page, &shots.join("media-desktop.png"))?;
    super::fit(chrome, &page, 375, 812, true)?;
    s.ok("video and PDF attachments fit a phone without sideways page scrolling", chrome.eval(&page, "document.documentElement.scrollWidth <= innerWidth && document.getElementById('scroll').scrollWidth === document.getElementById('scroll').clientWidth")? == true, "");
    chrome.screenshot(&page, &shots.join("media-phone.png"))?;
    // More than the initial 400 records, then a fresh page: the browser's
    // live cursor keeps its end while a separate cursor pages backwards.
    chrome.close(page)?;
    for i in 0..1005 {
        let body = match i {
            0 => json!({ "kind": "turn.start", "turn": "old-progress", "agent": agent }),
            1 => json!({ "kind": "turn.end", "turn": "old-progress", "agent": agent, "outcome": "stopped" }),
            _ => json!({ "kind": "history.marker", "turn": "work-marker", "agent": agent }),
        };
        let r = api.signed(&hand, "POST", &format!("/api/f/{name}/channels/work"), Some(&json!({ "id": format!("work-history-{i}"), "body": body })))?;
        anyhow::ensure!(r.status == 200, "posting work history {i}: {r}");
    }
    for i in 0..415 {
        let mut body = json!({ "text": format!("History {i:03}") });
        if i == 414 { body["reply_to"] = json!(2); }
        let r = api.signed(owner, "POST", &format!("/api/f/{name}/channels/chat"), Some(&json!({ "id": format!("history-{i}"), "body": body })))?;
        anyhow::ensure!(r.status == 200, "posting history {i}: {r}");
    }
    let backwards = api.signed(owner, "GET", &format!("/api/f/{name}/channels/chat?before=20&limit=3"), None)?;
    let seqs: Vec<_> = backwards.body["records"].as_array().into_iter().flatten().filter_map(|r| r["seq"].as_i64()).collect();
    s.ok("backwards channel reads are exclusive, nearest first, returned in ascending display order", backwards.status == 200 && seqs == [17, 18, 19] && backwards.body["next"] == 17, &backwards);
    let empty = api.signed(owner, "GET", &format!("/api/f/{name}/channels/chat?before=1"), None)?;
    s.ok("an empty backwards page keeps its cursor", empty.body["records"] == json!([]) && empty.body["next"] == 1, &empty);
    let bad = api.signed(owner, "GET", &format!("/api/f/{name}/channels/chat?after=0&before=20"), None)?;
    let malformed = api.signed(owner, "GET", &format!("/api/f/{name}/channels/chat?before=no"), None)?;
    s.ok("ambiguous and malformed history cursors are refused", bad.status == 400 && malformed.status == 400, format!("{bad} | {malformed}"));
    let page = chrome.open(&api.site_url(&name, ""))?;
    super::fit(chrome, &page, 1280, 860, false)?;
    s.ok("the initial backlog is bounded to 400 and offers earlier messages", super::shows(chrome, &page, "document.querySelectorAll('.msg.user').length === 400 && !document.getElementById('earlier').hidden"), "");
    s.ok("a quote outside the initial history has an earlier-message placeholder", chrome.eval(&page, "document.querySelector('.msg[data-seq=\"416\"] .quote')?.disabled === true")? == true, "");
    // The reader's place is the anchor's offset from the top of the scroll
    // box, so the header above it settling (the chat's agent, its name and
    // picture, read after the page opens) moves no one's place; and the
    // anchor is taken once that has settled.
    let settled = super::shows(chrome, &page, "document.getElementById('head-label').textContent !== 'Chat' && [...document.querySelectorAll('#head-marks img')].every((i) => i.complete)");
    anyhow::ensure!(settled, "the chat's header shows its agent");
    // The reader's message is found by its seq each time it is measured:
    // a message is drawn again as a new node when what it shows changes
    // (its poster's name come, say), and a node held from before is then
    // off the page, measured at 0 (CI's "moved: 32.5" was that: the scroll
    // box's top less the message's offset)
    chrome.eval(&page, "(() => { const s = document.getElementById('scroll'); window.__offset = (n) => n.getBoundingClientRect().top - s.getBoundingClientRect().top; window.__placed = () => window.__offset(document.querySelector('.msg[data-seq=\"' + window.__historySeq + '\"]')); s.scrollTop = 100; return true; })()")?;
    super::shows(chrome, &page, "document.getElementById('scroll').scrollTop === 100");
    let anchor = chrome.eval(&page, "(() => { const s = document.getElementById('scroll'); const m = [...document.querySelectorAll('.msg[data-seq]')].find(m => m.getBoundingClientRect().bottom > s.getBoundingClientRect().top); window.__historySeq = m.dataset.seq; window.__historyTop = window.__offset(m); document.getElementById('earlier').click(); return m.dataset.seq; })()")?;
    let loaded = super::shows(chrome, &page, "document.querySelectorAll('.msg.user').length === 416 && !document.getElementById('earlier').disabled");
    let position = chrome.eval(&page, "Math.abs(window.__placed() - window.__historyTop)")?;
    let ordered = chrome.eval(&page, "(() => { const seqs = [...document.querySelectorAll('.msg[data-seq]')].map(m => Number(m.dataset.seq)); return seqs.every((seq, i) => !i || seq > seqs[i-1]); })()")?;
    s.ok("loading earlier messages preserves the visible message, ascending order, and no duplicates", loaded && position.as_f64().is_some_and(|n| n < 2.0) && ordered == true, json!({ "anchor": anchor, "moved": position, "ordered": ordered }));
    s.ok("loading earlier history fills the quote above its message", super::shows(chrome, &page, "document.querySelector('.msg[data-seq=\"416\"] .quote .quote-text')?.textContent === 'History 000' && !document.querySelector('.msg[data-seq=\"416\"] .quote').disabled"), "");
    s.ok("the earlier control disappears at the beginning of retained history", chrome.eval(&page, "document.getElementById('earlier').hidden")? == true, "");
    s.ok("earlier messages also bring their older work records beyond the initial 1000", super::shows(chrome, &page, "!!document.querySelector('.notice.stopped[data-turn=\"old-progress\"]')"), "");
    chrome.screenshot(&page, &shots.join("history-desktop.png"))?;
    s.ok("jump to latest appears while reading earlier messages", super::shows(chrome, &page, "!document.getElementById('latest').hidden"), "");
    for i in 0..2 {
        let r = api.signed(owner, "POST", &format!("/api/f/{name}/channels/chat"), Some(&json!({ "id": format!("new-{i}"), "body": { "text": format!("New message {i}") } })))?;
        anyhow::ensure!(r.status == 200, "posting a new message: {r}");
    }
    let counted = super::shows(chrome, &page, "document.getElementById('latest').textContent.includes('2 new messages')");
    let still = chrome.eval(&page, "Math.abs(window.__placed() - window.__historyTop)")?;
    s.ok("new live messages are counted without moving the reader", counted && still.as_f64().is_some_and(|n| n < 2.0), json!({ "moved": still }));
    let command = api.signed(owner, "POST", &format!("/api/f/{name}/channels/chat"), Some(&json!({ "id": "new-command", "body": { "kind": "command", "command": "usage", "args": "", "to": [agent] } })))?;
    s.ok("live command messages also count toward jump to latest", command.status == 200 && super::shows(chrome, &page, "document.getElementById('latest').textContent.includes('3 new messages') && !!document.querySelector('.bubble.command .command-name')"), &command);
    chrome.click(&page, "#latest")?;
    s.ok("jump to latest reaches the end and clears the count", super::shows(chrome, &page, "document.getElementById('latest').hidden && (() => { const s = document.getElementById('scroll'); return s.scrollHeight - s.scrollTop - s.clientHeight < 2; })()"), "");
    // Render the fixture through the released markdown module. This also
    // checks incomplete streamed fences and quotes without HTML parsing.
    let fixture = super::js(include_str!("../../fixtures/chat-markdown.md"));
    chrome.eval(&page, &format!("import('./markdown.js').then(m => {{ const node = document.createElement('div'); node.id = 'markdown-fixture'; node.className = 'msg agent md'; node.append(m.renderMarkdown({fixture})); document.getElementById('messages').replaceChildren(node); return true; }})"))?;
    s.ok("markdown renders headings, strikethrough, tasks, nested lists, blockquotes and rules", chrome.eval(&page, "(() => { const m = document.getElementById('markdown-fixture'); return !!m.querySelector('h1') && !!m.querySelector('del') && m.querySelectorAll('input[type=checkbox][disabled]').length === 2 && !!m.querySelector('input:checked') && !!m.querySelector('ul ul ol') && !!m.querySelector('blockquote h2') && !!m.querySelector('blockquote ul') && !!m.querySelector('hr'); })()")? == true, "");
    s.ok("tables preserve escaped/code pipes and autolinks use safe URLs", chrome.eval(&page, "(() => { const m = document.getElementById('markdown-fixture'); return m.querySelectorAll('tbody tr').length === 2 && m.querySelector('tbody td code').textContent === 'a|b' && m.querySelectorAll('tbody td')[1].textContent === 'a|b' && !!m.querySelector('a[href=\"mailto:paul@example.com\"]') && !!m.querySelector('a[href=\"https://example.com/docs\"]') && m.querySelector('table th:last-child').style.textAlign === 'right'; })()")? == true, "");
    s.ok("raw HTML, script URLs and remote images remain inert text", chrome.eval(&page, "(() => { const m = document.getElementById('markdown-fixture'); return !m.querySelector('img,script,a[href^=\"javascript:\"]') && !window.__injected && m.textContent.includes('<img src=x') && m.textContent.includes('<script>window.__injected'); })()")? == true, "");
    s.ok("fenced code is literal and carries its language label", chrome.eval(&page, "document.querySelector('.md-language').textContent === 'rust' && document.querySelector('pre code').dataset.language === 'rust' && document.querySelector('pre code').textContent.includes('<script>') && document.querySelectorAll('.md-code-copy').length === 2")? == true, "");
    chrome.eval(&page, "(() => { navigator.clipboard.writeText = async text => { window.__codeCopied = text; }; document.querySelector('.md-code-copy').click(); return true; })()")?;
    s.ok("a code block's copy button copies only its literal source", super::shows(chrome, &page, "window.__codeCopied === document.querySelector('pre code').textContent && document.querySelector('.md-code-copy').textContent === 'Copied'"), "");
    let streamed = chrome.eval(&page, "import('./markdown.js').then(m => { const n = document.createElement('div'); n.append(m.renderMarkdown('```js\\n<script>')); const deep = document.createElement('div'); deep.append(m.renderMarkdown('> '.repeat(100) + 'safe')); return n.querySelector('pre code').textContent === '<script>' && !n.querySelector('script') && deep.textContent === '> '.repeat(68) + 'safe'; })")?;
    s.ok("incomplete streamed fences and deeply nested quotes remain safe", streamed == true, "");
    super::fit(chrome, &page, 375, 812, true)?;
    chrome.eval(&page, "(() => { const t = document.querySelector('.md-table'); t.querySelector('th').textContent = 'wide '.repeat(80); t.querySelector('table').style.width = '900px'; return true; })()")?;
    s.ok("wide tables scroll within the reply on a phone", chrome.eval(&page, "(() => { const t = document.querySelector('.md-table'); t.scrollLeft = 100; return t.scrollLeft > 0 && t.scrollWidth > t.clientWidth && document.documentElement.scrollWidth <= innerWidth; })()")? == true, "");
    chrome.screenshot(&page, &shots.join("markdown-wide-table-phone.png"))?;
    chrome.eval(&page, "(() => { const t = document.querySelector('.md-table'); t.querySelector('th').textContent = 'Language'; t.querySelector('table').style.removeProperty('width'); t.scrollLeft = 0; document.getElementById('scroll').scrollTop = 0; return true; })()")?;
    chrome.screenshot(&page, &shots.join("markdown-phone.png"))?;
    super::fit(chrome, &page, 1280, 860, false)?;
    chrome.eval(&page, "(() => { document.getElementById('scroll').scrollTop = 0; return true; })()")?;
    chrome.screenshot(&page, &shots.join("markdown-desktop.png"))?;
    // Reopen to discard only the markdown fixture. A real network outage
    // exercises the library's retries, then the message's own Retry.
    chrome.close(page)?;
    // (with no page of this chat open beside it: its presence redraws them)
    place_through_redraw(s, api, chrome, &name)?;
    let page = chrome.open(&api.site_url(&name, ""))?;
    anyhow::ensure!(super::shows(chrome, &page, "document.getElementById('say')?.dataset.ready === '1'"), "the failure page is ready");
    anyhow::ensure!(super::shows(chrome, &page, "[...document.querySelectorAll('.msg.user')].some(m => m.querySelector('.bubble')?.textContent === 'History 414' && !!m.querySelector('.reply'))"), "the quoted message has loaded");
    // Sending, then sent (Paul, 2026-10-13: nothing looks sent before the
    // platform has it): its post held, the message shows as sending, with
    // no time and no Reply, and the platform has no record of it; let go,
    // it is sent, once.
    chrome.eval(&page, "(() => { const real = window.fetch; window.fetch = async (url, o) => { if (String(url).includes('__op/channels/chat') && !window.__held) { window.__held = true; await new Promise((r) => { window.__release = r; }); } return real(url, o); }; return true; })()")?;
    chrome.eval(&page, "(() => { const t = document.getElementById('text'); t.value = 'Held for a moment'; t.dispatchEvent(new Event('input')); document.getElementById('say').requestSubmit(); return true; })()")?;
    let held = "(() => { const m = [...document.querySelectorAll('.msg.user.mine')].filter((m) => m.querySelector('.bubble')?.textContent === 'Held for a moment'); return m.length === 1 && m[0].classList.contains('sending') && m[0].querySelector('.time')?.textContent === 'Sending…' && !m[0].querySelector('.reply') && typeof window.__release === 'function'; })()";
    let sending = super::shows(chrome, &page, held);
    let unsent = super::super::jobs::records(api, owner, &name, "chat").iter().all(|r| r["body"]["text"] != "Held for a moment");
    chrome.screenshot(&page, &shots.join("sending.png"))?;
    s.ok("a message the platform has not answered shows as sending (dimmed, Sending… for its time, no Reply), and it has no record yet", sending && unsent, "");
    chrome.eval(&page, "(window.__release(), true)")?;
    let sent = super::shows(chrome, &page, "(() => { const m = [...document.querySelectorAll('.msg.user.mine')].filter((m) => m.querySelector('.bubble')?.textContent === 'Held for a moment'); return m.length === 1 && !m[0].classList.contains('sending') && /\\d/.test(m[0].querySelector('.time')?.textContent ?? '') && !!m[0].querySelector('.reply') && !!m[0].dataset.post; })()");
    let stored = super::super::jobs::records(api, owner, &name, "chat").iter().filter(|r| r["body"]["text"] == "Held for a moment").count();
    s.ok("its record come, it shows as sent (its time, Reply), the same post, stored once", sent && stored == 1, stored);
    let quoted_seq = chrome.eval(&page, "(() => { const m = [...document.querySelectorAll('.msg.user')].find(m => m.querySelector('.bubble')?.textContent === 'History 414'); m.querySelector('.reply').click(); return Number(m.dataset.seq); })()")?;
    chrome.offline(&page, true)?;
    chrome.eval(&page, "(() => { document.getElementById('text').value = 'Retry my message'; document.getElementById('say').requestSubmit(); return true; })()")?;
    s.ok("a failed channel write shows its error and Retry on that message", super::shows(chrome, &page, "!![...document.querySelectorAll('.msg.user.mine')].find(m => m.textContent.includes('Retry my message') && m.querySelector('.message-error .message-retry'))"), "");
    s.ok("a failed quoted message keeps its quote and is not marked queued or offered Reply", chrome.eval(&page, "(() => { const m = document.querySelector('.message-error').closest('.msg'); return m.querySelector('.quote .quote-text')?.textContent === 'History 414' && !m.querySelector('.queued, .reply'); })()")? == true, "");
    chrome.screenshot(&page, &shots.join("post-error-desktop.png"))?;
    chrome.offline(&page, false)?;
    chrome.eval(&page, "(() => { document.querySelector('.message-retry').click(); return true; })()")?;
    s.ok("retry sends the same message successfully and clears its local error", super::shows(chrome, &page, "!document.querySelector('.message-error') && [...document.querySelectorAll('.msg.user.mine .bubble')].filter(m => m.textContent === 'Retry my message').length === 1"), "");
    let retried = super::super::jobs::records(api, owner, &name, "chat").iter().filter(|r| r["body"]["text"] == "Retry my message").count();
    s.ok("the retried message is stored once", retried == 1, retried);
    let quote_kept = super::super::jobs::records(api, owner, &name, "chat").iter().any(|r| r["body"]["text"] == "Retry my message" && r["body"]["reply_to"] == quoted_seq);
    s.ok("retry preserves the quoted message's reply_to", quote_kept, &quoted_seq);

    // A completed record fixture of the chat's agent, its lead, which no
    // computer runs, so this chat stays quiet. The frame receives the
    // existing shell roster seam, and the agent's own work records end its
    // waiting status; no new platform test levers are needed.
    for body in [json!({ "kind": "turn.start", "turn": "fixture-old", "agent": agent }), json!({ "kind": "turn.end", "turn": "fixture-old", "outcome": "idle" })] {
        let r = api.signed(&hand, "POST", &format!("/api/f/{name}/channels/work"), Some(&json!({ "id": body["kind"], "body": body })))?;
        anyhow::ensure!(r.status == 200, "posting a completed work fixture: {r}");
    }
    chrome.eval(&page, "(() => { const frame = document.createElement('iframe'); frame.id = 'polish-frame'; frame.src = './'; frame.style = 'width:100%;height:100%;border:0'; document.body.replaceChildren(frame); window.__frame = frame; return true; })()")?;
    anyhow::ensure!(super::shows(chrome, &page, "window.__frame.contentDocument?.getElementById('say')?.dataset.ready === '1'"), "the status frame is ready");
    let roster = json!({ "fragment": "agents", "agents": [{ "identity": agent, "name": label, "title": "Bob", "phase": "starting" }] });
    chrome.eval(&page, &format!("(() => {{ window.__frame.contentWindow.postMessage({roster}, location.origin); return true; }})()"))?;
    chrome.eval(&page, "(() => { const d = window.__frame.contentDocument; d.getElementById('text').value = 'A slow first reply'; d.getElementById('say').requestSubmit(); return true; })()")?;
    s.ok("the shell's optional starting phase quietly explains a slow first reply", super::shows(chrome, &page, "window.__frame.contentDocument.querySelector('.working')?.textContent.includes('Bob is starting up…')"), "");
    chrome.screenshot(&page, &shots.join("starting-desktop.png"))?;
    chrome.eval(&page, &format!("new Promise(resolve => {{ const w = window.__frame.contentWindow; window.__bootNow = w.Date.now; w.Date.now = () => window.__bootNow() + 91000; w.postMessage({roster}, location.origin); w.requestAnimationFrame(() => w.requestAnimationFrame(() => resolve(true))); }})"))?;
    s.ok("a known computer startup remains explained after ninety seconds", super::shows(chrome, &page, "window.__frame.contentDocument.querySelector('.working')?.textContent.includes('Bob is starting up…')"), "");
    chrome.eval(&page, "(() => { window.__frame.contentWindow.Date.now = window.__bootNow; return true; })()")?;
    for (phase, expected) in [
        ("asleep", "Bob is starting up…"),
        ("awake", "Waiting for Bob…"),
        ("sleeping", "Bob's computer is going to sleep…"),
        ("wont_wake", "Bob's computer could not start. Open the computer settings to restart it."),
    ] {
        let roster = json!({ "fragment": "agents", "agents": [{ "identity": agent, "name": label, "title": "Bob", "phase": phase }] });
        chrome.eval(&page, &format!("(() => {{ window.__frame.contentWindow.postMessage({roster}, location.origin); return true; }})()"))?;
        s.ok(&format!("the shell's {phase} roster phase has the matching quiet status"), super::shows(chrome, &page, &format!("window.__frame.contentDocument.querySelector('.working')?.textContent === {}", super::js(expected))), "");
    }
    let absent = json!({ "fragment": "agents", "agents": [{ "identity": agent, "name": label, "title": "Bob" }] });
    chrome.eval(&page, &format!("(() => {{ window.__frame.contentWindow.postMessage({absent}, location.origin); return true; }})()"))?;
    s.ok("without a phase the page waits plainly, without claiming the agent is working", super::shows(chrome, &page, "window.__frame.contentDocument.querySelector('.working')?.textContent === 'Waiting for Bob…'"), "");
    chrome.eval(&page, "(() => { const w = window.__frame.contentWindow; window.__now = w.Date.now; w.Date.now = () => window.__now() + 91000; w.postMessage({fragment:'agents',agents:[]}, location.origin); return true; })()")?;
    let late = super::shows(chrome, &page, "window.__frame.contentDocument.querySelector('.working')?.textContent.includes('has not started a reply yet')");
    // what the frame shows when it does not say so (one CI run failed with
    // nothing to read, 41271812848): its waiting lines, its composer, its
    // clock against the page's, and the person's last messages there
    let shown = if late {
        Value::Null
    } else {
        chrome
            .eval(&page, "(() => { const w = window.__frame.contentWindow; const d = w.document; return { working: [...d.querySelectorAll('.working')].map((l) => l.textContent), placeholder: d.getElementById('text')?.placeholder ?? null, ahead: w.Date.now() - window.__now(), mine: [...d.querySelectorAll('.msg.user.mine')].slice(-2).map((m) => [m.dataset.seq ?? null, m.querySelector('.bubble')?.textContent ?? null, m.className]), url: w.location.href }; })()")
            .unwrap_or(Value::Null)
    };
    s.ok("after ninety seconds without a turn the page says a reply has not started", late, shown);
    chrome.eval(&page, "(() => { window.__frame.contentWindow.Date.now = window.__now; return true; })()")?;
    let cause = super::super::jobs::records(api, owner, &name, "chat").into_iter().find(|r| r["body"]["text"] == "A slow first reply").unwrap_or_default();
    let started = api.signed(&hand, "POST", &format!("/api/f/{name}/channels/work"), Some(&json!({ "id": "fixture-new", "body": { "kind": "turn.start", "turn": "fixture-new", "agent": agent, "asker": api.identity(owner)?, "cause": { "channel": "chat", "seq": cause["seq"] } } })))?;
    anyhow::ensure!(started.status == 200, "starting the fixture turn: {started}");
    s.ok("a turn.start replaces startup or delay status with the agent's working status", super::shows(chrome, &page, "window.__frame.contentDocument.querySelectorAll('.working').length === 1 && window.__frame.contentDocument.querySelector('.working').textContent.includes('is working')"), "");
    println!("      (polish screenshots in {})", shots.display());
    chrome.close(page)?;
    forged_turn(s, api, owner, chrome, &name, &hand, agent)
}

/// The reader's place holds through a draw that makes every message's
/// node again while the messages above it load: their poster's name come
/// in that moment (a slow first answer of `__people`, on a cold page),
/// every message of theirs shows it, as a new node. The place is the
/// reader's message, found by its key, not the node it had: kept by its
/// node, a node gone was no place, and the earlier messages pushed the
/// reader down by their height. Method: the chat's page with its
/// `__people` held, and its earlier messages' read once asked; Load
/// earlier pressed, the names let come (the reader's message is a new
/// node), then the earlier messages. As Safari, which has no scroll
/// anchoring of its own (`overflow-anchor: none`), so what holds the place
/// is the page's keeping.
fn place_through_redraw(s: &mut Suite, api: &Api, chrome: &mut Browser, name: &str) -> Result<()> {
    let hold = "if (window === window.top) { const real = window.fetch.bind(window); window.__held = { people: [], earlier: [] }; \
        const hold = (k) => new Promise((r) => window.__held[k].push(r)); \
        window.fetch = async (url, o) => { const u = String(url); \
          if (u.includes('__people') && !window.__peopleLet) await hold('people'); \
          if (window.__holdEarlier && u.includes('__channel/chat') && u.includes('before=')) { window.__holdEarlier = false; await hold('earlier'); } \
          return real(url, o); }; }";
    let page = chrome.open_with_script(&api.site_url(name, ""), hold)?;
    super::fit(chrome, &page, 1280, 860, false)?;
    let ready = super::shows(chrome, &page, "document.querySelectorAll('.msg.user').length === 400 && !document.getElementById('earlier').hidden && window.__held.people.length > 0");
    chrome.eval(&page, "(() => { const s = document.getElementById('scroll'); s.style.overflowAnchor = 'none'; s.scrollTop = 100; return true; })()")?;
    super::shows(chrome, &page, "document.getElementById('scroll').scrollTop === 100");
    chrome.eval(
        &page,
        "(() => { const s = document.getElementById('scroll'); const off = (n) => n.getBoundingClientRect().top - s.getBoundingClientRect().top; \
         const m = [...document.querySelectorAll('.msg[data-seq]')].find((m) => m.getBoundingClientRect().bottom > s.getBoundingClientRect().top); \
         window.__placed = () => off(document.querySelector('.msg[data-seq=\"' + m.dataset.seq + '\"]')); window.__top = off(m); window.__node = m; \
         window.__holdEarlier = true; document.getElementById('earlier').click(); return true; })()",
    )?;
    let held = super::shows(chrome, &page, "window.__held.earlier.length === 1");
    chrome.eval(&page, "(window.__peopleLet = true, window.__held.people.forEach((r) => r()), true)")?;
    let redrawn = super::shows(chrome, &page, "!window.__node.isConnected && !!document.querySelector('.msg[data-seq=\"' + window.__node.dataset.seq + '\"]')");
    chrome.eval(&page, "(window.__held.earlier.forEach((r) => r()), true)")?;
    let loaded = super::shows(chrome, &page, "document.getElementById('earlier').hidden && !document.getElementById('earlier').disabled && document.querySelectorAll('.msg.user').length > 400");
    let moved = chrome.eval(&page, "Math.abs(window.__placed() - window.__top)")?;
    s.ok(
        "the reader's place holds while earlier messages load though every message is drawn again meanwhile (its poster's name come), with no scroll anchoring of the browser's (as Safari)",
        ready && held && redrawn && loaded && moved.as_f64().is_some_and(|n| n < 2.0),
        json!({ "ready": ready, "held": held, "redrawn": redrawn, "loaded": loaded, "moved": moved }),
    );
    chrome.close(page)?;
    Ok(())
}

/// An agent of `owner`'s that runs nowhere (no computer runs it, so no
/// turn starts in its chat), an editor of chat `name`: a quiet chat's
/// agent records, posted as itself, since only a chat's agents post on
/// `work` (`"from": "agent"`).
fn quiet_agent(api: &Api, owner: &Keys, name: &str) -> Result<(Keys, String)> {
    let keys = Keys::generate();
    let reg = "/api/identities";
    let r = api.signed(owner, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&keys, "POST", reg, owner) })))?;
    anyhow::ensure!(r.status == 200, "registering a quiet agent: {r}");
    let id = r.body["id"].as_str().unwrap_or("").to_string();
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/members/{id}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "adding the quiet agent to {name}: {r}");
    Ok((keys, id))
}

/// Audit M7: an editor once re-posted an agent's pending approval card
/// with its own words, and the chat's owner read the forger's text on
/// the agent's card. Goal: no person posts on `work`, and on the page a
/// turn's records count only from the agent whose `turn.start` began it.
/// Method: the chat's owner (a person) posts a step, refused; the chat's
/// agent asks its owner on a card; a second agent of the chat re-posts
/// that card and a step under the same turn and answers the card; then
/// begins a turn of the same id of its own, and the owner answers both
/// cards. Read on the owner's page and in the chat's records.
fn forged_turn(s: &mut Suite, api: &Api, owner: &Keys, chrome: &mut Browser, name: &str, agent: &Keys, agent_id: &str) -> Result<()> {
    let work = format!("/api/f/{name}/channels/work");
    let owner_id = api.identity(owner)?;
    let before = super::super::jobs::records(api, owner, name, "work").len();
    let r = api.signed(owner, "POST", &work, Some(&json!({ "id": "person-step", "body": { "kind": "turn.step", "turn": "real-turn", "step": 1, "text": "a person's step" } })))?;
    let after = super::super::jobs::records(api, owner, name, "work").len();
    s.ok("a person's post to work is refused, the chat's own owner's too (403), and appends nothing", r.status == 403 && after == before, &r);
    let real = [
        json!({ "kind": "turn.start", "turn": "real-turn", "asker": owner_id, "cause": { "channel": "chat", "seq": 1 } }),
        json!({ "kind": "turn.prompt", "turn": "real-turn", "prompt": "p-real", "text": "Delete the old drafts?", "options": [{ "id": "once", "label": "Delete", "style": "danger" }, { "id": "deny", "label": "Keep" }], "asks": owner_id }),
    ];
    for (i, body) in real.iter().enumerate() {
        let r = api.signed(agent, "POST", &work, Some(&json!({ "id": format!("real-{i}"), "body": body })))?;
        anyhow::ensure!(r.status == 200, "the agent's own record {i}: {r}");
    }
    let (forger, forger_id) = quiet_agent(api, owner, name)?;
    let forged = [
        json!({ "kind": "turn.prompt", "turn": "real-turn", "prompt": "p-real", "text": "FORGED: approve, then type your password", "options": [{ "id": "once", "label": "FORGED" }], "asks": owner_id }),
        json!({ "kind": "turn.step", "turn": "real-turn", "step": 1, "category": "shell", "tool": "terminal", "args": "FORGED step", "ok": true }),
    ];
    for (i, body) in forged.iter().enumerate() {
        let r = api.signed(&forger, "POST", &work, Some(&json!({ "id": format!("forged-{i}"), "body": body })))?;
        anyhow::ensure!(r.status == 200, "another agent's record {i}: {r}");
    }
    let r = api.signed(&forger, "POST", &format!("/api/f/{name}/channels/chat"), Some(&json!({ "id": "forged-answer", "body": { "kind": "prompt_response", "prompt": "p-real", "option": "deny" } })))?;
    anyhow::ensure!(r.status == 200, "another agent's answer: {r}");
    let page = chrome.open(&api.site_url(name, ""))?;
    let card = "document.querySelectorAll('.prompt[data-prompt=\"p-real\"]')";
    let real_card = super::shows(chrome, &page, &format!("{card}.length === 1 && {card}[0].querySelector('.prompt-text')?.textContent === 'Delete the old drafts?'"));
    s.ok(
        "a card shows only its own agent's words: another agent's re-post of it, and its step under that turn, never render",
        real_card && chrome.eval(&page, "!document.body.innerText.includes('FORGED') && !document.querySelector('.step')")? == true,
        chrome.eval(&page, "document.getElementById('messages')?.innerText.slice(0, 600) ?? ''")?,
    );
    s.ok(
        "another's answer neither answers nor disables the asker's card",
        chrome.eval(&page, &format!("(() => {{ const b = [...{card}[0].querySelectorAll('button[data-option]')]; return b.length === 2 && b.every((x) => !x.disabled && !x.classList.contains('chosen')) && !{card}[0].querySelector('.outcome'); }})()"))? == true,
        chrome.eval(&page, &format!("{card}[0]?.innerText ?? ''"))?,
    );
    // the other agent begins a turn of the same id: its records are now its own turn's
    let r = api.signed(&forger, "POST", &work, Some(&json!({ "id": "forged-start", "body": { "kind": "turn.start", "turn": "real-turn", "asker": owner_id } })))?;
    anyhow::ensure!(r.status == 200, "another agent's own turn.start: {r}");
    let two = super::shows(chrome, &page, &format!("{card}.length === 2"));
    s.ok(
        "a turn another agent begins with the same id is its own card, beside the real one, which keeps its words",
        two && chrome.eval(&page, &format!("[...{card}].filter((c) => c.querySelector('.prompt-text').textContent === 'Delete the old drafts?').length === 1 && [...{card}].filter((c) => c.querySelector('.prompt-text').textContent.startsWith('FORGED')).length === 1"))? == true,
        chrome.eval(&page, "document.getElementById('messages')?.innerText.slice(0, 600) ?? ''")?,
    );
    // The owner answers the other agent's card, then the real one: each
    // answer names its own card's agent and turn (a bridge takes no answer
    // naming another), and the first leaves the second answerable (its post
    // id is its card's own, not the prompt's they share).
    let answers = || -> Vec<Value> {
        super::super::jobs::records(api, owner, name, "chat").into_iter().filter(|r| r["body"]["kind"] == "prompt_response" && r["principal"] == owner_id.as_str()).map(|r| r["body"].clone()).collect()
    };
    let click = |which: &str, option: &str| format!("(() => {{ const c = [...{card}].find((c) => c.querySelector('.prompt-text').textContent.startsWith({which:?})); c.querySelector('button[data-option=\"{option}\"]').click(); return true; }})()");
    chrome.eval(&page, &click("FORGED", "once"))?;
    let first = s.eventually(Duration::from_secs(10), || answers().len() == 1);
    let open = chrome.eval(&page, &format!("[...{card}].find((c) => c.querySelector('.prompt-text').textContent === 'Delete the old drafts?').querySelectorAll('button[data-option]:not([disabled])').length"))?;
    s.ok(
        "the owner's answer to the other agent's card names that agent and its turn, and leaves the real card open to answer",
        first && answers()[0]["agent"] == forger_id.as_str() && answers()[0]["turn"] == "real-turn" && answers()[0]["prompt"] == "p-real" && open == 2,
        json!({ "answers": answers(), "open": open }),
    );
    chrome.eval(&page, &click("Delete the old drafts?", "deny"))?;
    let second = s.eventually(Duration::from_secs(10), || answers().len() == 2);
    s.ok(
        "and their answer to the real card names the real agent: only that agent's card is answered by it",
        second && answers()[1]["agent"] == agent_id && answers()[1]["turn"] == "real-turn" && answers()[1]["option"] == "deny",
        json!({ "answers": answers() }),
    );
    chrome.close(page)?;
    Ok(())
}

// Real work records, including an older bridge's step and the words older
// bridges kept on steps (`text`), on a quiet chat: no computer joins it, so
// the eight-step turn stays open for inspection.
pub(super) fn working_card(s: &mut Suite, api: &Api, owner: &Keys, chrome: &mut Browser, agent: &str, agent_name: &str) -> Result<()> {
    let name = s.named(api, owner, "working-card")?;
    let made = api.create_with(owner, json!({ "name": name, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the working-card chat: {made}");
    s.owned(&made.body, owner);
    // The chat's agent posts its records itself (only a chat's agents post
    // on `work`): its owner adds a key of its for this (an owner manages
    // its agent's keys), and makes it the chat's. No person's message
    // comes, so its computer starts no turn here.
    let hand = Keys::generate();
    let add = format!("/api/identities/{agent}/keys");
    let r = api.signed(owner, "POST", &add, Some(&json!({ "proof": api.proof(&hand, "POST", &add, owner) })))?;
    anyhow::ensure!(r.status == 200, "a key of the agent's for the fixture: {r}");
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/members/{agent}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent joins the working-card chat: {r}");
    let home = format!("/data/work/{agent_name}/home/");
    let bodies = [
        json!({ "kind": "turn.start", "turn": "working-card", "agent": agent }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 1, "category": "read", "tool": "read_file", "args": format!("{home}apps-finite/README.md"), "ok": true, "text": "It works **partially.** I’ll check `apps-finite`.\n\n> **Design standards** — keep the card readable.\n\n- Read the files\n- [Check the docs](https://example.com/docs)\n\n<img src=x onerror=\"window.__noteInjected=true\">" }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 2, "category": "write", "tool": "patch", "args": format!("{home}apps-finite/src/components/chat/working-card/variants/desktop/theme/layout/WorkingCard.tsx"), "ok": true }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 3, "tool": "terminal", "args": "ls -R /data/hermes/managed-skills/software-development/references/engineering-style.md", "ok": true, "text": "I’ll keep the filename visible and shorten the directories. This sentence is deliberately long enough to make the note fold at a sentence boundary while retaining its markdown and the full explanation when expanded. **The final sentence stays available.**" }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 4, "category": "shell", "tool": "terminal", "args": format!("ls /data/work/{agent_name}/apps-finite"), "ok": true, "excerpt": "README.md\nsrc", "text": "Installed cleanly. Now listing the app:" }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 5, "category": "shell", "tool": "terminal", "args": "cargo test", "ok": false, "excerpt": "error: expected a closing delimiter" }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 6, "category": "read", "tool": "read_file", "args": "/data/work/another-agent/home/SOUL.md", "ok": true }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 7, "category": "web", "tool": "web_search", "args": "working card design", "ok": true }),
        json!({ "kind": "turn.step", "turn": "working-card", "step": 8, "category": "shell", "tool": "terminal", "args": "cargo check" }),
    ];
    for (i, body) in bodies.iter().enumerate() {
        let r = api.signed(&hand, "POST", &format!("/api/f/{name}/channels/work"), Some(&json!({ "id": format!("fixture-{i}"), "body": body })))?;
        anyhow::ensure!(r.status == 200, "posting working-card fixture {i}: {r}");
    }
    let page = chrome.open(&api.site_url(&name, "__signin?return=/"))?;
    super::fit(chrome, &page, 1000, 1100, false)?;
    // the words an older bridge kept on steps 1, 3 and 4 are the agent's
    // replies, between the turn's cards: three cards, the live one last
    let card = "[...document.querySelectorAll('details.tools')].at(-1)";
    let lines = format!("[...{card}.querySelectorAll(':scope > .step')]");
    let shown = format!("{lines}.filter((n) => n.getClientRects().length)");
    let said = "[...document.querySelectorAll('.msg.agent[data-step]')]";
    anyhow::ensure!(super::shows(chrome, &page, &format!("document.querySelectorAll('details.tools .step').length === 8 && document.querySelectorAll('details.tools').length === 3 && {card}.matches('.rolling[open]') && {lines}.length === 5 && {said}.length === 3")), "eight-step working turn rendered");
    s.ok(
        "the agent's words before a step show as its reply, whole, just before that step's card (no Reply: no record holds them), and no card holds any of its words",
        chrome.eval(&page, &format!("(() => {{ const said = {said}; const cards = [...document.querySelectorAll('details.tools')]; return said.map((m) => m.dataset.step).join() === '1,3,4' && said.every((m, i) => m.nextElementSibling === cards[i] && !m.closest('details') && !!m.querySelector('.who') && !!m.querySelector('.copy') && !m.querySelector('.reply')) && said[1].querySelector('.md').textContent.includes('This sentence is deliberately long') && said[1].querySelector('.md strong')?.textContent === 'The final sentence stays available.' && said[2].querySelector('.md').textContent === 'Installed cleanly. Now listing the app:' && cards.every((d) => !d.querySelector('.md')); }})()"))? == true,
        chrome.eval(&page, "[...document.querySelectorAll('#messages > *')].map((n) => `${n.tagName} ${n.className} ${n.dataset.step ?? ''}`)")?,
    );
    s.ok(
        "the cards before it are folded, each to one line: how long it worked and how many steps",
        chrome.eval(&page, "(() => { const [a, b] = document.querySelectorAll('details.tools'); return !a.open && !b.open && /^Worked for \\d+(s|m \\d\\ds|h \\d+m)· 2 steps$/.test(a.querySelector('summary').textContent) && /^Worked for \\d+(s|m \\d\\ds|h \\d+m)· 1 step$/.test(b.querySelector('summary').textContent); })()")? == true,
        chrome.eval(&page, "[...document.querySelectorAll('details.tools summary')].map((h) => h.textContent)")?,
    );
    s.ok(
        "while its turn works, its last card's head says for how long and how many steps, and no working line shows under it",
        super::shows(chrome, &page, &format!("(() => {{ const h = {card}.querySelector('summary'); return !!h.querySelector('.step-dot') && /^Working for \\d+(s|m \\d\\ds|h \\d+m)· 5 steps$/.test(h.textContent) && !document.querySelector('.msg.working'); }})()")),
        chrome.eval(&page, &format!("{card}?.querySelector('summary')?.textContent ?? ''"))?,
    );
    s.ok(
        "only its last three steps show, the newest whole and each older one fainter (.6, then .3)",
        super::shows(chrome, &page, &format!("(() => {{ const n = {shown}; return n.length === 3 && n.map((x) => getComputedStyle(x).opacity).join() === '0.3,0.6,1' && n[2] === {lines}.at(-1); }})()")),
        chrome.eval(&page, &format!("{shown}.map((x) => [x.textContent, getComputedStyle(x).opacity])"))?,
    );
    s.ok(
        "the step it is doing now shimmers, its icon named for it in the present tense, its verb not in words",
        chrome.eval(&page, "(() => { const s = [...document.querySelectorAll('.step')].at(-1); return s.classList.contains('running') && !s.querySelector('.step-verb') && s.querySelector('.step-name > svg[role=\"img\"]')?.getAttribute('aria-label') === 'Running' && s.querySelector('.step-args').textContent === 'cargo check' && s.querySelector('.step-args').classList.contains('shimmer'); })()")? == true,
        chrome.eval(&page, "[...document.querySelectorAll('.step')].at(-1)?.outerHTML ?? ''")?,
    );
    chrome.eval(&page, &format!("(() => {{ {card}.dispatchEvent(new MouseEvent('click', {{ bubbles: true }})); return true; }})()"))?;
    s.ok(
        "clicking its steps opens the card whole: every step, at full strength",
        super::shows(chrome, &page, &format!("(() => {{ const d = {card}; const n = {shown}; return d.open && !d.classList.contains('rolling') && n.length === 5 && n.every((x) => getComputedStyle(x).opacity === '1'); }})()")),
        "",
    );
    s.ok("the agent's words render bold, code, quotes, lists and safe links with the reply renderer", chrome.eval(&page, "(() => { const n = document.querySelector('.msg.agent[data-step=\"1\"] .md'); return n.querySelector('strong').textContent === 'partially.' && n.querySelector('code').textContent === 'apps-finite' && !!n.querySelector('blockquote strong') && n.querySelectorAll('li').length === 2 && n.querySelector('a').getAttribute('href') === 'https://example.com/docs' && n.querySelector('a').rel === 'noopener noreferrer'; })()")? == true, "");
    s.ok("HTML in the agent's words is escaped", chrome.eval(&page, "(() => { const n = document.querySelector('.msg.agent[data-step=\"1\"]'); return !n.querySelector('.md img, .md script') && !window.__noteInjected && n.innerText.includes('<img src=x onerror='); })()")? == true, "");
    s.ok(
        "a done step's icon says what it did, its verb in the past tense the icon's name for assistive technology and not in words beside it",
        chrome.eval(&page, "(() => { const steps = document.querySelectorAll('.step'); return [[0, 'Read'], [1, 'Wrote'], [3, 'Ran'], [6, 'Searched the web']].every(([i, verb]) => { const g = steps[i].querySelector('.step-name > svg'); return !steps[i].querySelector('.step-verb') && g.getAttribute('role') === 'img' && g.getAttribute('aria-label') === verb && !g.hasAttribute('aria-hidden') && !!steps[i].querySelector('.step-label > .step-args:only-child') && !steps[i].querySelector('.shimmer'); }); })()")? == true,
        chrome.eval(&page, "[...document.querySelectorAll('.step')].map((s) => s.querySelector('.step-name').outerHTML)")?,
    );
    // (the agent's profile read first: its paths shorten once it is known)
    let full = format!("{home}apps-finite/README.md");
    s.ok("the agent's own home becomes ~/ with the full original path in its title", super::shows(chrome, &page, &format!("document.querySelector('.step-args')?.textContent === '~/apps-finite/README.md' && document.querySelector('.step-args').title === {}", super::js(&full))), &full);
    let opened = chrome.eval(&page, "(() => { const s = document.querySelectorAll('.step')[3]; const line = s.querySelector('.step-name'); const d = s.querySelector('.step-detail'); const folded = d.hidden && line.getAttribute('aria-expanded') === 'false'; line.click(); const open = !d.hidden && line.getAttribute('aria-expanded') === 'true'; const call = d.querySelector('.step-call')?.textContent ?? null; const result = d.querySelector('.step-result')?.textContent ?? null; line.click(); return { folded, open, call, result, closed: d.hidden }; })()")?;
    s.ok(
        "a step's result stays folded until its line is opened: then its whole arguments, and what came of it",
        opened == json!({ "folded": true, "open": true, "call": "ls work/apps-finite", "result": "README.md\nsrc", "closed": true }),
        &opened,
    );
    s.ok("workspace and managed-skill prefixes shorten, including an older bridge's tool fallback, named in words by its tool (its wrench names none)", chrome.eval(&page, "(() => { const steps = document.querySelectorAll('.step'); return steps[2].dataset.category === 'other' && steps[2].querySelector('.step-verb').textContent === 'terminal:' && steps[2].querySelector('.step-name > svg').getAttribute('aria-hidden') === 'true' && steps[2].querySelector('.step-args').textContent === 'ls -R skills/software-development/references/engineering-style.md' && steps[3].querySelector('.step-args').textContent === 'ls work/apps-finite' && steps[5].querySelector('.step-args').textContent.startsWith('/data/work/another-agent/home/'); })()")? == true, "");
    s.ok("a failed step says so in words, alone in colour, its result in that colour too", chrome.eval(&page, "(() => { const s = document.querySelector('.step.error'); const done = getComputedStyle(document.querySelector('.step:not(.error):not(.running) .step-name')).color; const red = getComputedStyle(s.querySelector('.step-name')).color; return s.querySelector('.step-verb').textContent === 'Failed running' && s.querySelector('.step-name > svg').getAttribute('aria-hidden') === 'true' && s.querySelector('.step-result').textContent === 'error: expected a closing delimiter' && red !== done && getComputedStyle(s.querySelector('.step-result')).color === red; })()")? == true, "");
    chrome.eval(&page, "(() => { document.querySelector('.msg.agent[data-step]').scrollIntoView({block:'start'}); return true; })()")?;
    let shots = s.dir("working-card");
    chrome.screenshot(&page, &shots.join("desktop.png"))?;
    // the folded cards opened, as a person opens one: their steps are measured once shown
    chrome.eval(&page, "(() => { const [a, b] = document.querySelectorAll('details.tools'); a.querySelector('summary').click(); b.querySelector('summary').click(); return true; })()")?;
    super::fit(chrome, &page, 375, 1000, true)?;
    s.ok("a narrow step stays one line, shortening directories in the middle while keeping the filename", super::shows(chrome, &page, "(() => { const a = document.querySelectorAll('.step-args')[1]; const h = Number.parseFloat(getComputedStyle(a).lineHeight); return a.textContent.startsWith('~/') && a.textContent.includes('…/') && a.textContent.endsWith('/WorkingCard.tsx') && a.scrollWidth <= a.clientWidth && a.clientHeight <= h + 1 && a.title.endsWith('/layout/WorkingCard.tsx'); })()"), chrome.eval(&page, "document.querySelectorAll('.step-args')[1].textContent")?);
    s.ok("the older bridge's path also keeps its filename visible when shortened on a phone", super::shows(chrome, &page, "(() => { const a = document.querySelectorAll('.step-args')[2]; return a.textContent.startsWith('ls -R skills/') && a.textContent.includes('…/') && a.textContent.endsWith('/engineering-style.md') && a.title.startsWith('ls -R /data/hermes/managed-skills/') && a.scrollWidth <= a.clientWidth; })()"), chrome.eval(&page, "document.querySelectorAll('.step-args')[2].textContent")?);
    s.ok("the working turn fits the phone without horizontal page scrolling", chrome.eval(&page, "document.documentElement.scrollWidth <= innerWidth && document.getElementById('scroll').scrollWidth === document.getElementById('scroll').clientWidth")? == true, "");
    chrome.eval(&page, "(() => { document.querySelector('.msg.agent[data-step]').scrollIntoView({block:'start'}); return true; })()")?;
    chrome.screenshot(&page, &shots.join("phone.png"))?;
    super::fit(chrome, &page, 1000, 1100, false)?;
    s.ok("resizing back restores the full shortened path when one line fits", super::shows(chrome, &page, "document.querySelectorAll('.step-args')[1].textContent === '~/apps-finite/src/components/chat/working-card/variants/desktop/theme/layout/WorkingCard.tsx'"), chrome.eval(&page, "document.querySelectorAll('.step-args')[1].textContent")?);
    // a step opened before its turn ends is open when its card is opened again
    chrome.eval(&page, "(() => { document.querySelectorAll('.step')[3].querySelector('.step-name').click(); return true; })()")?;
    chrome.eval(&page, &format!("(() => {{ {card}.querySelector('summary').click(); return true; }})()"))?;
    s.ok("its head closes it back to the last three steps while its turn works", super::shows(chrome, &page, &format!("{card}.classList.contains('rolling') && {shown}.length === 3")), "");
    let ended = api.signed(&hand, "POST", &format!("/api/f/{name}/channels/work"), Some(&json!({ "id": "fixture-end", "body": { "kind": "turn.end", "turn": "working-card", "outcome": "idle" } })))?;
    anyhow::ensure!(ended.status == 200, "ending working-card fixture: {ended}");
    s.ok(
        "once its turn ends its last card folds to one line, how long it worked and an icon for each kind of work, and opened again keeps a step opened before",
        super::shows(chrome, &page, &format!("(() => {{ const d = {card}; const h = d.querySelector('summary'); if (d.open || !/^Worked for \\d+(s|m \\d\\ds|h \\d+m)· 5 steps$/.test(h.textContent) || h.querySelectorAll('.step-kinds svg').length !== 3 || h.querySelector('.step-dot')) return false; h.click(); return d.open && !d.classList.contains('rolling') && !document.querySelectorAll('.step')[3].querySelector('.step-detail').hidden; }})()")),
        chrome.eval(&page, &format!("{card}?.querySelector('summary')?.textContent ?? ''"))?,
    );
    s.ok("cards opened while the turn worked stay open once it ends", chrome.eval(&page, "(() => { const [a, b] = document.querySelectorAll('details.tools'); return a.open && b.open; })()")? == true, "");
    println!("      (working-card screenshots in {})", shots.display());
    chrome.close(page)?;
    let r = api.signed(owner, "DELETE", &format!("{add}/{}", fragment_core::npub::encode(hand.pubkey_hex())), None)?;
    anyhow::ensure!(r.status == 200, "the fixture's key revoked: {r}");
    Ok(())
}

/// Goal (#725, FIN-262): a tool call shows while it runs, from its intent
/// (`turn.call`, recorded before it runs), and its step takes its place;
/// real state only. Live: its latest response's call is the card's live
/// line, what it does and for how long, under a head saying the turn works,
/// with no working line under it; an earlier response's call no step ended
/// may have run (a runtime ends a response's calls before it asks again).
/// Its step (renamed by its runtime, `call_0_d2`: paired by what it does,
/// within its response) takes its place. Once the turn ends (cut by a
/// restart), nothing reads as running: a call no step ended may have run.
pub(super) fn calls_card(s: &mut Suite, api: &Api, owner: &Keys, chrome: &mut Browser, agent: &str) -> Result<()> {
    let name = s.named(api, owner, "calls-card")?;
    let made = api.create_with(owner, json!({ "name": name, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the calls-card chat: {made}");
    s.owned(&made.body, owner);
    // the chat's agent posts its own records, as in `working_card`
    let hand = Keys::generate();
    let add = format!("/api/identities/{agent}/keys");
    let r = api.signed(owner, "POST", &add, Some(&json!({ "proof": api.proof(&hand, "POST", &add, owner) })))?;
    anyhow::ensure!(r.status == 200, "a key of the agent's for the fixture: {r}");
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/members/{agent}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent joins the calls-card chat: {r}");
    let post = |i: usize, body: Value| -> Result<()> {
        let r = api.signed(&hand, "POST", &format!("/api/f/{name}/channels/work"), Some(&json!({ "id": format!("calls-{i}"), "body": body })))?;
        anyhow::ensure!(r.status == 200, "posting calls-card fixture {i}: {r}");
        Ok(())
    };
    let call = |response: &str, args: &str| json!({ "kind": "turn.call", "turn": "calls", "response": response, "tool_call_id": "call_0", "tool": "terminal", "category": "shell", "args": args });
    post(0, json!({ "kind": "turn.start", "turn": "calls", "agent": agent }))?;
    post(1, call("r1", "make build"))?;
    post(2, call("r2", "npm test"))?;
    let page = chrome.open(&api.site_url(&name, "__signin?return=/"))?;
    super::fit(chrome, &page, 1000, 900, false)?;
    let shots = s.dir("calls-card");
    let card = "document.querySelector('details.tools')";
    let line = |args: &str| format!("[...{card}.querySelectorAll(':scope > .step')].find((s) => s.querySelector('.step-args')?.textContent === {})", super::js(args));
    let (build, test) = (line("make build"), line("npm test"));
    s.ok(
        "while its turn works, its latest call is the card's live line: what it does (its icon named \"Running\"), shimmering, and for how long; the head says the turn works, and no working line shows",
        super::shows(chrome, &page, &format!("(() => {{ const t = {test}; const h = {card}?.querySelector('summary'); return !!t && t.classList.contains('running') && t.querySelector('.step-name > svg[role=\"img\"]')?.getAttribute('aria-label') === 'Running' && t.querySelector('.step-args').classList.contains('shimmer') && /^\\d+s$/.test(t.querySelector('.step-since')?.textContent ?? '') && !!h.querySelector('.step-dot') && /^Working for \\d+s· 2 steps$/.test(h.textContent) && !document.querySelector('.msg.working'); }})()")),
        chrome.eval(&page, &format!("{card}?.outerHTML ?? ''"))?,
    );
    s.ok(
        "an earlier response's call with no step may have run: said in words, never running",
        chrome.eval(&page, &format!("(() => {{ const b = {build}; return !!b && b.classList.contains('maybe') && !b.classList.contains('running') && b.querySelector('.step-verb')?.textContent === 'May have run' && !b.querySelector('.step-since') && !b.querySelector('.shimmer'); }})()"))? == true,
        chrome.eval(&page, &format!("{build}?.outerHTML ?? ''"))?,
    );
    // its frames, a second apart as its time counts (for a person: the
    // lifecycle, stitched as they like)
    for i in 1..=3 {
        chrome.screenshot(&page, &shots.join(format!("frame-{i}-running.png")))?;
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    chrome.screenshot(&page, &shots.join("desktop-running.png"))?;
    super::fit(chrome, &page, 375, 800, true)?;
    s.ok("on a phone the live line stays one line, its time beside it", super::shows(chrome, &page, &format!("(() => {{ const t = {test}; const a = t.querySelector('.step-args'); const h = Number.parseFloat(getComputedStyle(a).lineHeight); return a.clientHeight <= h + 1 && t.querySelector('.step-since').getClientRects().length === 1; }})()")), "");
    chrome.screenshot(&page, &shots.join("phone-running.png"))?;
    super::fit(chrome, &page, 1000, 900, false)?;
    // its step: renamed by its runtime (`call_0_d2`), paired by what it does
    post(3, json!({ "kind": "turn.step", "turn": "calls", "step": 1, "tool": "terminal", "category": "shell", "args": "npm test", "ok": true, "excerpt": "Exit code: 0", "tool_call_id": "call_0_d2", "status": "completed", "duration_ms": 900, "response": "r2" }))?;
    s.ok(
        "its step takes its place: one line, done, with no time of its own",
        super::shows(chrome, &page, &format!("(() => {{ const steps = [...{card}.querySelectorAll(':scope > .step')].filter((s) => s.querySelector('.step-args')?.textContent === 'npm test'); return steps.length === 1 && !steps[0].classList.contains('running') && !steps[0].querySelector('.step-since') && steps[0].querySelector('.step-name > svg')?.getAttribute('aria-label') === 'Ran'; }})()")),
        chrome.eval(&page, &format!("{card}?.outerHTML ?? ''"))?,
    );
    chrome.screenshot(&page, &shots.join("frame-4-step.png"))?;
    // cut by a restart: nothing reads as running once it ended
    post(4, call("r3", "make deploy"))?;
    super::shows(chrome, &page, &format!("!!{} && {}.classList.contains('running')", line("make deploy"), line("make deploy")));
    chrome.screenshot(&page, &shots.join("frame-5-next-call.png"))?;
    post(5, json!({ "kind": "turn.end", "turn": "calls", "outcome": "error", "error": "lost when the computer restarted" }))?;
    let deploy = line("make deploy");
    s.ok(
        "once its turn ended (cut by a restart), a call no step ended may have run: nothing reads as running, and the head says how long it worked",
        super::shows(chrome, &page, &format!("(() => {{ const d = {deploy}; const h = {card}.querySelector('summary'); return !!d && d.classList.contains('maybe') && d.querySelector('.step-verb')?.textContent === 'May have run' && !{card}.querySelector('.step.running, .step-since, .shimmer, .step-dot') && /^Worked for /.test(h.textContent); }})()")),
        chrome.eval(&page, &format!("{card}?.outerHTML ?? ''"))?,
    );
    // folded once it ended, as any card: opened, as a person opens it
    chrome.eval(&page, &format!("(() => {{ {card}.querySelector('summary').click(); return true; }})()"))?;
    super::shows(chrome, &page, &format!("{card}.open"));
    chrome.screenshot(&page, &shots.join("desktop-ended.png"))?;
    chrome.screenshot(&page, &shots.join("frame-6-cut.png"))?;
    println!("      (calls-card screenshots in {})", shots.display());
    chrome.close(page)?;
    let r = api.signed(owner, "DELETE", &format!("{add}/{}", fragment_core::npub::encode(hand.pubkey_hex())), None)?;
    anyhow::ensure!(r.status == 200, "the fixture's key revoked: {r}");
    Ok(())
}

/// The shapes of a production chat's records (2026-10-20: a chat that did
/// not load in the shell): prompt answers from before answers named their
/// card (`{kind: "prompt_response", option, prompt}`, no agent, no turn),
/// messages whose files' blobs are not here, an owner's `steer`, a step
/// with long commentary (an older bridge's `text`), a turn whose answer was
/// both a step's words and its reply (an older bridge's answer said beside a
/// housekeeping call); and an agent's reply whose markdown made the
/// renderer throw (a row of one cell over a separator with no pipe: `a |`,
/// then `---`), which kept every redraw of its chat from drawing anything.
/// Goal: the page draws every one of them, the commentary as the agent's
/// reply before its step and the answer once, and an item it cannot draw
/// (here a card of steps, in a browser without `Object.hasOwn`, as Safari
/// before 15.4) is a quiet line in its place, said to the console, the rest
/// of the chat drawn. Method: a quiet chat's owner and agent post them; the
/// owner's page read, then read again with `Object.hasOwn` taken away.
pub(super) fn record_shapes(s: &mut Suite, api: &Api, owner: &Keys, session: &str, chrome: &mut Browser) -> Result<()> {
    use sha2::{Digest, Sha256};
    let name = s.named(api, owner, "chat-shapes")?;
    let made = api.create_with(owner, json!({ "name": name, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the shapes chat: {made}");
    s.owned(&made.body, owner);
    let (hand, agent) = quiet_agent(api, owner, &name)?;
    let me = api.identity(owner)?;
    let post = |by: &Keys, channel: &str, id: &str, body: Value| -> Result<i64> {
        let r = api.signed(by, "POST", &format!("/api/f/{name}/channels/{channel}"), Some(&json!({ "id": id, "body": body })))?;
        anyhow::ensure!(r.status == 200, "posting {id} on {channel}: {r}");
        Ok(r.body["seq"].as_i64().unwrap_or(0))
    };
    // a file named by a hash no blob of the chat's has
    let missing = |name: &str, kind: &str, size: u64| json!({ "name": name, "sha256": hex::encode(Sha256::digest(name.as_bytes())), "size": size, "type": kind });
    let asked = post(owner, "chat", "ask", json!({ "text": "My couch broke. How do I get a replacement?" }))?;
    post(&hand, "work", "start", json!({ "kind": "turn.start", "turn": "t1", "asker": me, "agent": agent, "cause": { "channel": "chat", "seq": asked } }))?;
    let commentary = "The warranty covers a spring that broke from ordinary sitting, so this should be a free replacement rather than a repair you pay for. I will look up the claim form, then ask you two short questions before I fill it in for you.";
    post(&hand, "work", "step", json!({ "kind": "turn.step", "turn": "t1", "step": 1, "category": "web", "tool": "web_search", "args": "sofa spring warranty", "ok": true, "excerpt": "", "text": commentary }))?;
    for (i, prompt) in ["289a31.dcc130a7", "289a31.623765bc"].iter().enumerate() {
        let options = json!([{ "id": "c0", "label": "Yes", "style": "primary" }, { "id": "c1", "label": "No" }, { "id": "other", "label": "Something else", "words": true }]);
        post(&hand, "work", &format!("p{i}"), json!({ "kind": "turn.prompt", "turn": "t1", "prompt": prompt, "text": format!("Question {}?", i + 1), "options": options, "asks": me, "expiresAt": 4_102_444_800_000u64 }))?;
        post(owner, "chat", &format!("answer-{i}"), json!({ "kind": "prompt_response", "option": "c0", "prompt": prompt }))?;
        post(&hand, "work", &format!("pc{i}"), json!({ "kind": "turn.prompt.closed", "turn": "t1", "prompt": prompt, "outcome": "answered", "option": "c0", "by": me }))?;
    }
    post(&hand, "chat", "rp:t1:1", json!({ "text": "Your new address |\n---\nIt is on file now.", "turn": "t1" }))?;
    post(&hand, "work", "end", json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }))?;
    // an older bridge's answer said beside a housekeeping call: its step's
    // words (cut at 300 characters) and then its reply, whole
    let answer = "Your claim is filed, and the store will call you to book the delivery. ".repeat(5);
    let cut = format!("{}…", answer.trim().chars().take(299).collect::<String>().trim_end());
    post(&hand, "work", "start-2", json!({ "kind": "turn.start", "turn": "t2", "asker": me, "agent": agent }))?;
    post(&hand, "work", "step-2", json!({ "kind": "turn.step", "turn": "t2", "step": 1, "category": "memory", "tool": "memory", "args": "the claim", "ok": true, "text": cut }))?;
    post(&hand, "chat", "rp:t2:1", json!({ "text": answer, "turn": "t2" }))?;
    post(&hand, "work", "end-2", json!({ "kind": "turn.end", "turn": "t2", "outcome": "idle" }))?;
    post(owner, "chat", "photos", json!({ "text": "", "attachments": [missing("IMG_1.jpeg", "image/jpeg", 4_628_527), missing("IMG_2.jpeg", "image/jpeg", 3_153_194)] }))?;
    post(owner, "chat", "screenshot", json!({ "text": "My address has changed", "attachments": [missing("IMG_3.png", "image/png", 793_981)] }))?;
    post(owner, "chat", "steer", json!({ "kind": "command", "command": "steer", "args": "Or you could move them to your computer", "to": [agent] }))?;
    post(&hand, "chat", "last", json!({ "text": "The last word." }))?;

    // each page's uncaught errors, and what it says to the console
    let probe = "window.__errors = []; addEventListener('error', (e) => window.__errors.push(String(e.message))); window.__said = []; const said = console.error; console.error = (...a) => { window.__said.push(a.map(String).join(' ')); said(...a); };";
    let rest = "(() => { const md = [...document.querySelectorAll('.msg.agent .md')]; const reply = md.find((m) => m.querySelector('table th')?.textContent === 'Your new address'); return !!reply && reply.textContent.includes('It is on file now.') && md.some((m) => m.textContent === 'The last word.') && [...document.querySelectorAll('.prompt.closed .outcome.answered')].filter((o) => o.textContent.startsWith('Yes')).length === 2 && [...document.querySelectorAll('.message-attachments .attachment-name')].map((a) => a.textContent).join() === 'IMG_1.jpeg,IMG_2.jpeg,IMG_3.png' && [...document.querySelectorAll('.bubble')].some((b) => b.textContent === 'My address has changed') && document.querySelector('.bubble.command .command-name')?.textContent === '/steer'; })()";
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", session)?;
    let page = chrome.open_with_script(&api.site_url(&name, "__signin?return=/"), probe)?;
    super::fit(chrome, &page, 1280, 860, false)?;
    // the commentary as the agent's reply before its step's card; t2's answer once, as its reply
    let words = format!("(() => {{ const said = [...document.querySelectorAll('.msg.agent[data-step]')]; return said.length === 1 && said[0].dataset.turn === 't1' && said[0].querySelector('.md').textContent === {} && !!said[0].nextElementSibling?.matches('details.tools') && [...document.querySelectorAll('.msg.agent[data-turn=\"t2\"]')].length === 1; }})()", super::js(commentary));
    let drew = super::shows(chrome, &page, rest) && super::shows(chrome, &page, &words);
    let seen = chrome.eval(&page, "({ errors: window.__errors, unshown: document.querySelectorAll('.unshown').length, items: [...document.querySelectorAll('#messages > *')].map((n) => n.className) })")?;
    s.ok(
        "a chat of production's record shapes draws whole: legacy prompt answers, files with no blobs, a steer, long commentary as the agent's reply before its step, an answer that was a step's words too shown once, and a one-cell table over a rule",
        drew && seen["errors"] == json!([]) && seen["unshown"] == 0,
        &seen,
    );
    chrome.screenshot(&page, &s.dir("chat-polish").join("record-shapes.png"))?;
    chrome.close(page)?;
    let page = chrome.open_with_script(&api.site_url(&name, ""), &format!("delete Object.hasOwn; {probe}"))?;
    super::fit(chrome, &page, 1280, 860, false)?;
    let quiet = "(() => { const n = [...document.querySelectorAll('#messages > .msg.notice.unshown')]; return n.length === 2 && n.every((x) => x.textContent === 'This part of the chat could not be shown.') && !!n[0].previousElementSibling?.matches('.msg.agent[data-step=\"1\"]') && !document.querySelector('details.tools'); })()";
    let drew = super::shows(chrome, &page, rest) && super::shows(chrome, &page, quiet);
    let seen = chrome.eval(&page, "({ errors: window.__errors, said: window.__said, items: [...document.querySelectorAll('#messages > *')].map((n) => n.className) })")?;
    let said = seen["said"].as_array().is_some_and(|l| l.iter().any(|x| x.as_str().is_some_and(|x| x.contains("could not be shown"))));
    s.ok(
        "an item the page cannot draw (a card of steps, with no Object.hasOwn) is a quiet line in its place, said to the console, and everything after it draws",
        drew && said && seen["errors"] == json!([]),
        &seen,
    );
    chrome.screenshot(&page, &s.dir("chat-polish").join("record-shapes-unshown.png"))?;
    chrome.close(page)?;
    Ok(())
}

/// Alex's review of #580: a finished card said how long its turn worked up
/// to the next thing the chat showed (an idle end shows nothing of its
/// own), so a turn's "Worked for 3s" became "Worked for 1h 0m" once
/// someone spoke an hour later. Goal: a finished card's time runs from its
/// turn's start (a later card's, its first step) to its own turn's next
/// record, else its end, whatever comes after: a person's message, another
/// agent's turn, a reload. Both shapes: steps, then an idle end; an answer,
/// a housekeeping (memory) step, then an idle end. Method: a quiet chat's
/// agents post them, each end seconds after its last step, and the owner's
/// message and another agent's turn seconds after each end; the heads read
/// the times the records give, live and after a reload.
pub(super) fn finished_card_times(s: &mut Suite, api: &Api, owner: &Keys, chrome: &mut Browser) -> Result<()> {
    let name = s.named(api, owner, "card-times")?;
    let made = api.create_with(owner, json!({ "name": name, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the card-times chat: {made}");
    s.owned(&made.body, owner);
    let (hand, agent) = quiet_agent(api, owner, &name)?;
    let (other, other_id) = quiet_agent(api, owner, &name)?;
    let me = api.identity(owner)?;
    // a record posted, and the time the platform gave it
    let post = |by: &Keys, channel: &str, id: &str, body: Value| -> Result<i64> {
        let r = api.signed(by, "POST", &format!("/api/f/{name}/channels/{channel}"), Some(&json!({ "id": id, "body": body })))?;
        anyhow::ensure!(r.status == 200, "posting {id} on {channel}: {r}");
        r.body["record"]["at"].as_i64().ok_or_else(|| anyhow::anyhow!("{id}: no time in {r}"))
    };
    let pause = |ms: u64| std::thread::sleep(Duration::from_millis(ms));
    let step = |turn: &str, n: u32, category: &str, tool: &str, args: &str| json!({ "kind": "turn.step", "turn": turn, "step": n, "category": category, "tool": tool, "args": args, "ok": true });
    // the head's words for a time, as the page says it (`took`)
    let took = |ms: i64| {
        let s = ((ms as f64) / 1000.0).round().max(1.0) as i64;
        if s < 60 { format!("{s}s") } else { format!("{}m {:02}s", s / 60, s % 60) }
    };
    let page = chrome.open(&api.site_url(&name, "__signin?return=/"))?;
    super::fit(chrome, &page, 1000, 1100, false)?;
    // steps, then an idle end
    let start1 = post(&hand, "work", "a-start", json!({ "kind": "turn.start", "turn": "t1", "asker": me, "agent": agent }))?;
    post(&hand, "work", "a-1", step("t1", 1, "shell", "terminal", "ls"))?;
    pause(1_000);
    post(&hand, "work", "a-2", step("t1", 2, "shell", "terminal", "cargo check"))?;
    pause(1_500);
    let end1 = post(&hand, "work", "a-end", json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }))?;
    pause(3_000);
    post(owner, "chat", "later-1", json!({ "text": "Thanks!" }))?;
    // an answer, a housekeeping step, then an idle end
    let start2 = post(&hand, "work", "b-start", json!({ "kind": "turn.start", "turn": "t2", "asker": me, "agent": agent }))?;
    post(&hand, "chat", "rp:t2:1", json!({ "text": "Noted: you like tomatoes.", "turn": "t2" }))?;
    post(&hand, "work", "b-1", step("t2", 1, "memory", "memory", "tomatoes"))?;
    pause(1_500);
    let end2 = post(&hand, "work", "b-end", json!({ "kind": "turn.end", "turn": "t2", "outcome": "idle" }))?;
    pause(3_000);
    // later: the owner speaks, and another agent works
    post(owner, "chat", "later-2", json!({ "text": "And one more thing." }))?;
    post(&other, "work", "c-start", json!({ "kind": "turn.start", "turn": "t3", "asker": me, "agent": other_id }))?;
    post(&other, "work", "c-1", step("t3", 1, "web", "web_search", "tomato varieties"))?;
    post(&other, "work", "c-end", json!({ "kind": "turn.end", "turn": "t3", "outcome": "idle" }))?;
    let want = json!([format!("Worked for {}· 2 steps", took(end1 - start1)), format!("Worked for {}· 1 step", took(end2 - start2))]);
    let heads = "[...document.querySelectorAll('details.tools summary')].slice(0, 2).map((h) => h.textContent)";
    let three = "document.querySelectorAll('details.tools').length === 3 && [...document.querySelectorAll('.bubble')].some((b) => b.textContent === 'And one more thing.')";
    let live = super::shows(chrome, &page, three) && chrome.eval(&page, heads)? == want;
    let seen = chrome.eval(&page, heads)?;
    chrome.reload(&page)?;
    let reloaded = super::shows(chrome, &page, three) && chrome.eval(&page, heads)? == want;
    s.ok(
        "a finished card says how long its own turn worked, to its own end (steps then an idle end; an answer, a memory step, an idle end), not to what comes after: a later message, another agent's turn, a reload",
        live && reloaded,
        json!({ "want": want, "live": seen, "reloaded": chrome.eval(&page, heads)? }),
    );
    chrome.close(page)?;
    Ok(())
}
