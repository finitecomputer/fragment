import { KIND } from "./frame-messages.js";

export function createSessionRefresh({ held, getFrames, personId, viewerKeys, validFragment, openApp, originOf, endAlerts, dropAlerts }) {
  // ---- a new release: the page reloads itself at a safe moment ----
  // A page outlives a deploy: a home screen's app or a tab left open runs the
  // shell, and its chats' pages, of the release it loaded with while the
  // platform under it moves on (Paul's phone, 2026-10-20: after the day's
  // deploys one chat would not load in the home screen's app, and did in a
  // tab of its own, until the app was quit). So the page names the release
  // that answered it (index.html's meta, `RELEASE`), every answer of the
  // platform names its own (`x-fragment-deploy`), and once one names another
  // the page reloads whole, its chats' and apps' frames with it, where it
  // was (its path, its chat, its apps' windows): while it is hidden, or at
  // the person's next move (a chat or an app opened). Words typed in a chat
  // are its page's to keep (in its own origin's storage, as its person's),
  // so they survive this reload as any other; so the shell reloads only
  // when no chat's page says it is busy with what no storage keeps (files
  // chosen, a voice memo, a message on its way or one that failed, a quote
  // chosen: `{fragment: KIND.busy, busy}`,
  // told as it changes), and none of this page's own dialogs, menu, fields
  // typed in, prompts on their way or writes still going hold it. Nothing
  // is asked at the moment of the reload, so nothing can change between an
  // answer and the reload; it is decided a moment after the page hid or the
  // person moved, so what a page posted just before is heard, and a page
  // busy as the reload is under way stops it (`reloadWhenSafe`, its one
  // window left). Each release is reloaded for at most
  // RELEASE_TRIES times, RELEASE_AGAIN_MS apart (a reload can still meet the
  // old release), and the release a reload left is no reason to reload for
  // RELEASE_LEFT_MS (a deploy's old side answers for a while). The page asks
  // for itself (`HEAD /healthz`) as it is hidden or shown again, comes back
  // from the back-forward cache or comes online, at most once a
  // RELEASE_LOOK_MS; it hears every answer it reads.
  const RELEASE_ID = /^[A-Za-z0-9._-]{1,64}$/;
  const RELEASE_HEADER = "x-fragment-deploy";
  const RELEASE_LOOK_MS = 60_000;
  const RELEASE_LEFT_MS = 10 * 60_000;
  const RELEASE_TRIES = 3;
  const RELEASE_AGAIN_MS = 60_000;
  const WINDOWS_KEPT_MAX = 16;
  // a click that opens a chat from the launcher closes its dialog first
  const NAVIGATED_SETTLE_MS = 400;
  // and the page hidden, a moment for what a chat's page posted just before to be heard
  const RELOAD_SETTLE_MS = 100;
  // signing out waits at most this long for the chats' pages to let their drafts go
  const FORGET_WAIT_MS = 1_000;
  const RELOADED_KEY = "shell.release.v1"; // {from, to, at, tries}: this tab's last reload for a release
  const WINDOWS_KEY = "shell.windows.v2"; // {who, windows: [{name, path}]}: the app windows to open again
  // this tab's own storage, which a reload keeps
  const tab = {
    get(key, fallback) { try { return JSON.parse(sessionStorage.getItem(key)) ?? fallback; } catch { return fallback; } },
    set(key, value) { try { sessionStorage.setItem(key, JSON.stringify(value)); return true; } catch { return false; } },
    drop(key) { try { sessionStorage.removeItem(key); } catch {} },
  };
  // #403's, which kept chats' words here and named no person: let go
  tab.drop("shell.unsent.v1");
  tab.drop("shell.windows.v1");
  const RELEASE = document.querySelector('meta[name="fragment-release"]')?.content ?? "";
  // when it was deployed, seconds (its other meta; none: empty, or unfilled)
  const DEPLOYED_META = document.querySelector('meta[name="fragment-release-deployed"]')?.content ?? "";
  const DEPLOYED = /^\d{1,12}$/.test(DEPLOYED_META) ? Number(DEPLOYED_META) : null;
  // mine: the release this page is (none, an unfilled page: it never reloads);
  // next: the release it waits to reload for
  const release = { mine: RELEASE_ID.test(RELEASE) ? RELEASE : null, next: null, lookedAt: 0, reloading: false };

  // An answer of the platform's, heard: one naming a release to reload for
  // is waited on, and reloaded for at once while the page is hidden.
  function heard(r) {
    const id = r.headers.get(RELEASE_HEADER);
    if (!release.mine || !id || id === release.mine || id === release.next || !RELEASE_ID.test(id) || !reloadsFor(id)) return;
    release.next = id;
    // what it waits for, for whoever looks (the e2e, a person's devtools)
    document.documentElement.dataset.releaseNext = id;
    if (document.hidden) reloadWhenSafe("hidden");
  }
  function lastReload() {
    const r = tab.get(RELOADED_KEY, null);
    return r && typeof r.from === "string" && typeof r.to === "string" && Number.isFinite(r.at) && Number.isInteger(r.tries) ? r : null;
  }
  // Whether another release is one to reload for: not the one this tab's
  // last reload left while that deploy may still be going out, nor one it
  // reloaded for and still is not, RELEASE_TRIES times or within
  // RELEASE_AGAIN_MS of the last.
  function reloadsFor(id) {
    const last = lastReload();
    if (!last) return true;
    const since = Date.now() - last.at;
    if (id === last.from && since < RELEASE_LEFT_MS) return false;
    return !(id === last.to && (last.tries >= RELEASE_TRIES || since < RELEASE_AGAIN_MS));
  }

  // the fields of this page the person typed in
  const TYPED = new Set(["text", "search", "email", "url", "tel", "password", "number"]);
  const typedIn = new Set();
  document.addEventListener("input", (e) => {
    const f = e.target;
    if (f instanceof HTMLTextAreaElement || (f instanceof HTMLInputElement && TYPED.has(f.type))) typedIn.add(f);
  }, true);
  // What this page itself holds that a reload would lose.
  function shellHolds() {
    for (const f of typedIn) if (!f.isConnected) typedIn.delete(f);
    const typed = [...typedIn].some((f) => f.value.trim() && f.getClientRects().length > 0);
    return typed || !!document.querySelector("dialog[open]") || held();
  }

  // The chats' frames whose page said hello (`agents?`, which the chat's
  // page sends as it starts), and those of them that say they are busy with
  // what no storage keeps. A hello is a new page, busy with nothing yet; a
  // frame sent elsewhere (a sign-in it could not make, tried again) is
  // neither. A page in a frame that says so wrongly only holds a reload back.
  const greeted = new WeakSet();
  const busyFrames = new WeakSet();
  const chatsBusy = () => [...getFrames()].some((f) => f.isConnected && busyFrames.has(f));

  // Reloads for `release.next` if nothing holds it: as the page is hidden
  // (`moment` "hidden"), and as the person opens a chat or an app
  // ("navigated"). Decided in a task of its own a moment later
  // (RELOAD_SETTLE_MS, NAVIGATED_SETTLE_MS), so what a chat's page posted
  // just before is heard first; nothing is asked or awaited. A chat's page
  // that says it is busy as the reload is under way stops it (`stopReload`).
  // What is left: a page that becomes busy once the new page has begun to
  // come in (its answer committed) is gone with it. That window opens only
  // while the page is hidden, or as the person themselves moves in it.
  function reloadWhenSafe(moment) {
    if (!release.next || release.reloading) return;
    setTimeout(() => {
      if ((moment === "hidden" && !document.hidden) || !release.next || release.reloading || chatsBusy() || shellHolds()) return;
      const last = lastReload();
      const tries = last?.to === release.next ? last.tries + 1 : 1;
      if (!tab.set(WINDOWS_KEY, { who: personId() ?? null, windows: openWindows() }) || !tab.set(RELOADED_KEY, { from: release.mine, to: release.next, at: Date.now(), tries })) return;
      release.reloading = { last };
      location.reload();
    }, moment === "navigated" ? NAVIGATED_SETTLE_MS : RELOAD_SETTLE_MS);
  }
  // The reload under way, stopped (`window.stop`), as if it had not begun:
  // the tab's last reload as it was, no windows kept; `release.next` waits
  // on for the next safe moment.
  function stopReload() {
    const { last } = release.reloading;
    window.stop();
    release.reloading = false;
    if (last) tab.set(RELOADED_KEY, last);
    else tab.drop(RELOADED_KEY);
    tab.drop(WINDOWS_KEY);
  }
  // The app windows open, each at the page it opened (`openApp`).
  function openWindows() {
    return viewerKeys().filter((k) => k.startsWith("app:")).slice(0, WINDOWS_KEPT_MAX).map((key) => {
      const src = document.querySelector(`#stack .pane[data-key="${CSS.escape(key)}"] iframe`)?.dataset.src ?? "";
      return { name: key.slice("app:".length), path: new URL(src, location.origin).searchParams.get("return") || "/" };
    });
  }
  // The windows the reload kept, taken as the page loads (a later reload by
  // hand opens none), and open again once its chat is, for the person who
  // had them open (on settings, which fill the work area, they are let go).
  const keptWindows = tab.get(WINDOWS_KEY, null);
  tab.drop(WINDOWS_KEY);
  function reopenWindows(settings) {
    if (settings || keptWindows?.who !== personId() || !Array.isArray(keptWindows?.windows)) return;
    for (const w of keptWindows.windows.slice(0, WINDOWS_KEPT_MAX)) {
      if (typeof w?.name === "string" && validFragment(w.name) && typeof w.path === "string" && w.path.startsWith("/")) openApp(w.name, w.path);
    }
  }

  // Signing out, or in with another account, ends the tab's person: what was
  // kept for them goes first, its windows here, each chat page's draft
  // (`{fragment: KIND.forgetDraft}`, answered `draft-forgotten`, waited for at
  // most FORGET_WAIT_MS) and their chats' alerts (`dropAlerts`, as long),
  // then the form goes. A draft of theirs a page not open here keeps is
  // never shown to anyone else (its page gives a draft back only to its own
  // person) and goes when someone else opens it.
  document.addEventListener("submit", (e) => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || !/^\/auth\/(logout|switch)(\?|$)/.test(form.getAttribute("action") ?? "")) return;
    tab.drop(WINDOWS_KEY);
    const frames = [...getFrames()].filter((f) => f.isConnected && greeted.has(f));
    e.preventDefault();
    endAlerts();
    const dropped = Promise.race([dropAlerts(), new Promise((done) => setTimeout(done, FORGET_WAIT_MS))]);
    Promise.all([frames.length ? forgetDrafts(frames) : null, dropped]).then(() => HTMLFormElement.prototype.submit.call(form));
  }, true);
  function forgetDrafts(frames) {
    return new Promise((done) => {
      const left = new Set(frames.map((f) => f.contentWindow));
      const timer = setTimeout(() => finish(), FORGET_WAIT_MS);
      const forgotten = (event) => {
        if (event.data?.fragment === KIND.draftForgotten && left.delete(event.source) && !left.size) finish();
      };
      const finish = () => {
        clearTimeout(timer);
        removeEventListener("message", forgotten);
        done();
      };
      addEventListener("message", forgotten);
      for (const f of frames) originOf(f.dataset.fragment).then((origin) => f.contentWindow?.postMessage({ fragment: KIND.forgetDraft }, origin), () => {});
    });
  }

  // the person opened a chat or an app: once the click has settled
  function navigated() {
    reloadWhenSafe("navigated");
  }
  // Asks the platform which release it is now (`HEAD /healthz`), at most
  // once a RELEASE_LOOK_MS; then, hidden, reloads if it may.
  async function lookForRelease() {
    if (!release.next && release.mine && Date.now() - release.lookedAt >= RELEASE_LOOK_MS) {
      release.lookedAt = Date.now();
      try {
        heard(await fetch("/healthz", { method: "HEAD", cache: "no-store" }));
      } catch {
        // offline (a phone just woken): its coming online asks again
        release.lookedAt = 0;
        return;
      }
    }
    if (document.hidden) reloadWhenSafe("hidden");
  }
  document.addEventListener("visibilitychange", () => lookForRelease());
  addEventListener("pagehide", () => reloadWhenSafe("hidden"));
  addEventListener("pageshow", (e) => { if (e.persisted) lookForRelease(); });
  addEventListener("online", () => lookForRelease());
  return { heard, navigated, reopenWindows, release, DEPLOYED,
    isBusy: (frame) => busyFrames.has(frame),
    resetFrame: (frame) => { greeted.delete(frame); busyFrames.delete(frame); },
    greet: (frame) => { greeted.add(frame); busyFrames.delete(frame); },
    busy: (frame, on) => { if (on) { busyFrames.add(frame); if (release.reloading) stopReload(); } else busyFrames.delete(frame); },
  };
}
