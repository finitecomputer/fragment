// The launcher: one input for starting a chat or finding one again. The
// sidebar's +, the phone's top bar + and ⌘K open it (Skyler, 2026-10-12).
// A module of its own beside shell.js, so a launcher full of features
// leaves the shell small (Paul, 2026-10-24).
//
// Empty stays quiet. Words ask the person's search (`GET /api/search`:
// their chats and apps by title and label, then messages, newest first,
// archived ones included): a chat found opens in the middle column, an app
// in the desktop, a message its chat. Enter sends those exact words, and
// the files dropped or pasted beside them, as the first message of a new
// chat with their agent (`primaryAgent`), one record however often it is
// tried (its id, kept). A request for an app is a message like any other:
// their agent makes it. Before their agent runs, the words go to their
// first chat's composer instead (`firstChat`).
//
// Enter goes straight to the chat (`startFromSearch`): the launcher closes
// at once and the middle column is the new chat's while it is made; its
// page loads (`?sent=1`: no new chat's start, only the message once it is
// here) as its files are uploaded and its message posted. A message not
// sent opens the launcher again on its words and files, saying why. Once
// the person gives the middle column another view (a chat, settings,
// Back), the launch changes nothing on screen: its message is still sent,
// and one not sent is a notice, its words and files waiting in the
// launcher's next opening (docs/api.md, The launcher).

import { icon } from "./icons.js";

// At most 8 files, 25 MiB each, as the chat's page takes them.
const FILES_MAX = 8;
const FILE_MAX_BYTES = 25 * 1024 * 1024;
// A connection dropped on a file's upload is tried again, as `fragment.blob` does.
const UPLOAD_TRIES = 3;
// The input grows with a long prompt to this, then scrolls.
const INPUT_MAX_PX = 200;
// The search API's hint: eight words, 256 bytes at most.
const HINT_WORDS = 8;
const HINT_MAX_BYTES = 256;

/**
 * Makes the launcher, its dialog `#search-dialog` (index.html), and opens it
 * from `#sidebar-new-chat`, `#new-chat` and ⌘K. `shell`: the shell's
 * helpers it uses, as shell.js names them (`$`, `el`, `api`, `ApiError`,
 * `seg`, `state`, `byName`, `isChat`, `titleOf`, `chatTitle`, `mark`,
 * `iconOf`, `primaryAgent`, `agentsOwnChat`, `UNTITLED`, `openChat`,
 * `openApp`, `chatFrame`, `composeIn`, `newChat`, `leaveSidebar`,
 * `closeMenu`, `notice`, `prewake`, `originOf`), and three it reads late:
 * `views()` (how many views the middle column has been given: `openChat`
 * and `openSettings` count them), `busy(name)` (whether a chat's page holds
 * what no storage keeps: files chosen, a voice memo) and `setupChat()` (the
 * first run's chat, or null).
 *
 * Answers `{ open(trigger), agentsRead(), holds() }`: `agentsRead`, after
 * each read of the person's things (an agent read while the launcher is
 * open can start the chat now); `holds`, whether a message of its own is
 * not sent yet (a reload for a new release waits for it).
 */
export function createLauncher(shell) {
  const { $, el, api, ApiError, seg, state, byName, isChat, titleOf, chatTitle, mark, iconOf, primaryAgent, agentsOwnChat, UNTITLED } = shell;
  const { openChat, openApp, chatFrame, composeIn, newChat, leaveSidebar, closeMenu, notice, prewake, originOf } = shell;
  const input = $("workspace-search");
  const dialog = $("search-dialog");

  let asked = 0;
  let typing = null;
  let searchIdentity = null;
  let searchTrigger = null;
  let launching = false;
  let closingSearch = false;
  // Keep the same chat and record id after an uncertain send, so retry cannot
  // duplicate the first message. This is only the current dialog's draft.
  let pendingLaunch = null;
  // Why the last send failed, said until the words change.
  let launchError = "";
  // The launcher's files, dropped on it or pasted: sent with its words as
  // the new chat's first message's attachments (docs/chat-records.md,
  // Attachments), each uploaded to the chat once it is made.
  let launchFiles = []; // { file, url }: url, an image's preview

  function openFound(name) {
    closeSearch();
    if (isChat(name)) openChat(name); else openApp(name);
    leaveSidebar();
  }
  function foundRow(name, preview, extra) {
    const f = byName(name);
    const row = el("button", "row search-result");
    row.type = "button";
    row.dataset.fragment = name;
    const text = el("span", "agent-copy");
    text.append(el("span", "label", chatTitle(name)));
    if (preview) text.append(el("span", "agent-preview", preview));
    row.append(isChat(name) ? mark(name, "small") : iconOf(name), text);
    if (f?.archived) row.append(el("span", "meta", "Archived"));
    if (extra) row.append(extra);
    row.onclick = () => openFound(name);
    return row;
  }
  const when = (at) => {
    const d = new Date(at);
    return d.toDateString() === new Date().toDateString() ? d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) : d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  };
  function renderFound(fragments, messages) {
    const results = $("search-results");
    results.replaceChildren();
    const q = input.value.trim();
    if (!q) return;
    const waiting = !canLaunch() && firstChat();
    if (canLaunch() || waiting) {
      const action = el("button", "row search-result search-start");
      action.type = "button";
      const glyph = el("span", "search-action-icon");
      glyph.append(icon("newChat"));
      const copy = el("span", "agent-copy");
      copy.append(el("span", "label", waiting ? `Say it in ${titleOf(waiting)}` : `Start a new chat with ${titleOf(state.agents.get(searchIdentity).fragment)}`), el("span", "agent-preview", q));
      const enter = el("span", "meta search-enter");
      enter.title = "Enter";
      enter.replaceChildren(icon("cornerDownLeft"));
      action.append(glyph, copy, enter);
      action.onclick = startFromSearch;
      results.append(action);
    }
    const groups = [
      ["Chats", fragments.filter((f) => f.kind === "chat").map((f) => foundRow(f.name, f.preview ?? ""))],
      // the desktop's apps, as the dock lists them (`apps`)
      ["Apps", fragments.filter((f) => f.kind === "app" || f.kind === "brain").map((f) => foundRow(f.name, ""))],
      // a message in a fragment the list has not shown yet is left out until it has
      ["Messages", messages.filter((m) => byName(m.fragment)).map((m) => {
        const row = foundRow(m.fragment, m.snippet, el("span", "meta", when(m.at)));
        row.classList.add("search-message");
        row.dataset.seq = String(m.seq);
        return row;
      })],
    ];
    let count = 0;
    for (const [heading, rows] of groups) {
      if (!rows.length) continue;
      count += rows.length;
      results.append(el("h3", null, heading), ...rows);
    }
    $("search-status").textContent = [launchError, count || canLaunch() || waiting ? "" : "Nothing found."].filter(Boolean).join(" ");
  }

  // The person's agent, asked again each time (their computer's agents may
  // arrive after the launcher opened); kept while a send is on its way.
  function canLaunch() {
    if (!launching) searchIdentity = primaryAgent()?.identity ?? null;
    return !state.guest && !!searchIdentity && state.agents.has(searchIdentity);
  }
  // Before their agent runs (its first run, or its computer not read yet),
  // the words go to the person's first chat, in its composer, to send there:
  // the chat the first run makes, else their agent's own.
  function firstChat() {
    if (state.guest) return null;
    const setup = shell.setupChat();
    if (setup && byName(setup)) return setup;
    const primary = primaryAgent();
    return primary ? agentsOwnChat(primary.fragment)?.name ?? null : null;
  }
  const canSend = () => canLaunch() || !!firstChat();
  const sendable = () => canSend() && (!!input.value.trim() || (launchFiles.length > 0 && canLaunch()));

  function addFiles(files) {
    launchError = "";
    for (const file of files) {
      if (launchFiles.length >= FILES_MAX) {
        launchError = `A message carries at most ${FILES_MAX} files.`;
        break;
      }
      if (file.size > FILE_MAX_BYTES) {
        launchError = `${file.name || "A file"} is over 25 MB.`;
        continue;
      }
      launchFiles.push({ file, url: file.type.startsWith("image/") ? URL.createObjectURL(file) : null });
    }
    renderFiles();
    updateSearch();
  }
  function dropFile(f) {
    if (f.url) URL.revokeObjectURL(f.url);
    launchFiles = launchFiles.filter((x) => x !== f);
  }
  function clearFiles() {
    for (const f of launchFiles) dropFile(f);
    renderFiles();
  }
  function renderFiles() {
    const box = $("search-files");
    box.hidden = !launchFiles.length;
    box.replaceChildren(...launchFiles.map((f) => {
      const name = f.file.name || "a file";
      const chip = el("span", "search-file");
      chip.title = name;
      if (f.url) {
        const img = el("img");
        img.src = f.url;
        img.alt = "";
        chip.append(img);
      } else {
        const glyph = el("span", "search-file-icon");
        glyph.append(icon("file"));
        chip.append(glyph);
      }
      const remove = el("button", "search-file-remove");
      remove.type = "button";
      remove.append(icon("x"));
      remove.setAttribute("aria-label", `Remove ${name}`);
      remove.onclick = () => {
        if (launching) return;
        dropFile(f);
        renderFiles();
        updateSearch();
        input.focus();
      };
      chip.append(el("span", "search-file-name", name), remove);
      return chip;
    }));
  }
  // One of the launcher's files as one of `chat`'s blobs (`PUT
  // /api/f/{chat}/blobs/{sha256}`, its bytes hashed here): what a record
  // names it by. A dropped connection is tried again: the same bytes under
  // the same hash store nothing twice.
  async function upload(chat, file) {
    const bytes = await file.arrayBuffer();
    const sha256 = [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
    const type = file.type || "application/octet-stream";
    let r;
    for (let attempt = 1; ; attempt++) {
      try {
        r = await fetch(`/api/f/${seg(chat)}/blobs/${sha256}`, { method: "PUT", headers: { "x-fragment-shell": "1", "content-type": type }, body: bytes, credentials: "same-origin" });
        break;
      } catch (e) {
        if (attempt >= UPLOAD_TRIES) throw e;
        await new Promise((resolve) => setTimeout(resolve, 300 * attempt));
      }
    }
    if (!r.ok) {
      const out = await r.json().catch(() => ({}));
      throw new ApiError(r.status, out.error, out.message || r.statusText);
    }
    return { sha256, size: bytes.byteLength, type, name: file.name || "" };
  }

  async function search() {
    const q = input.value.trim();
    const mine = ++asked;
    if (!q) return renderFound([], []);
    try {
      // A prompt can be longer than the search API's words and bytes. Only
      // the search hint is bounded; the complete prompt is sent to the chat.
      const hint = [...q.split(/\s+/).slice(0, HINT_WORDS).join(" ")];
      while (new TextEncoder().encode(hint.join("")).length > HINT_MAX_BYTES) hint.pop();
      const found = await api("GET", `/api/search?q=${encodeURIComponent(hint.join(""))}`);
      // a slower answer to an older query never replaces a newer one's
      if (mine === asked) renderFound(found.fragments ?? [], found.messages ?? []);
    } catch (e) {
      if (mine === asked) $("search-status").textContent = e.message;
    }
  }
  // `text`: what a suggestion asks, in place of the last unsent one
  function openSearch(trigger, text = null) {
    if (dialog.open || launching) return;
    closeMenu(true);
    searchTrigger = trigger;
    input.value = text ?? pendingLaunch?.text ?? "";
    updateSearch();
    dialog.showModal();
    // measured once shown: a closed dialog's input has no height
    fitSearch();
    trigger?.setAttribute("aria-expanded", "true");
    input.focus();
    // the computer starts waking as the person types, not when they send
    prewake();
  }
  // `launched`: the panel lifts away as the chat it started shows beneath.
  async function closeSearch(force = false, launched = false) {
    if (!dialog.open || closingSearch || launching && !force) return;
    closingSearch = true;
    ++asked;
    clearTimeout(typing);
    const panel = dialog.querySelector(".search-panel");
    // Start at the current visual state, even if Escape interrupts the entrance.
    const animations = matchMedia("(prefers-reduced-motion: reduce)").matches ? [] : [
      dialog.animate([{ opacity: getComputedStyle(dialog).opacity }, { opacity: 0 }], { duration: 280, easing: "cubic-bezier(.4, 0, 1, 1)", fill: "forwards" }),
      panel.animate([{ opacity: getComputedStyle(panel).opacity, transform: getComputedStyle(panel).transform }, { opacity: 0, transform: launched ? "translateY(-10px) scale(1.02)" : "translateY(8px)" }], { duration: launched ? 320 : 240, easing: "cubic-bezier(.4, 0, 1, 1)", fill: "forwards" }),
    ];
    dialog.inert = true;
    await Promise.allSettled(animations.map(animation => animation.finished));
    dialog.close();
    for (const animation of animations) animation.cancel();
    dialog.inert = false;
    closingSearch = false;
  }

  async function startFromSearch() {
    const text = input.value.trim();
    const files = launchFiles.map((f) => f.file);
    if ((!text && !files.length) || launching || closingSearch) return;
    if (!canLaunch()) {
      const chat = firstChat();
      if (!chat || !text) return;
      await closeSearch(true);
      composeIn(chat, text);
      leaveSidebar();
      return;
    }
    launching = true;
    ++asked;
    clearTimeout(typing);
    const trigger = searchTrigger;
    const closed = closeSearch(true, true);
    if (!state.page) {
      for (const frame of state.frames.values()) frame.hidden = true;
      $("chat-title").textContent = UNTITLED;
    }
    // The middle column is the launch's until the person gives it another
    // view (a chat, settings, Back): from then on what it does late changes
    // nothing on screen, and its message is still sent.
    let view = shell.views();
    const ours = () => view === shell.views();
    // A chat's frame let go, to load again: never one whose page holds what
    // no storage keeps (`busy`).
    const unload = (name) => {
      if (shell.busy(name)) return;
      state.frames.get(name)?.remove();
      state.frames.delete(name);
    };
    let name = null;
    try {
      const same = pendingLaunch && pendingLaunch.text === text && pendingLaunch.identity === searchIdentity && pendingLaunch.files.length === files.length && pendingLaunch.files.every((f, i) => f === files[i]);
      if (!same) pendingLaunch = { text, files, identity: searchIdentity, id: crypto.randomUUID(), chat: null };
      pendingLaunch.chat ||= await newChat(searchIdentity, { quiet: true, open: false });
      if (!pendingLaunch.chat) throw new Error("Another new chat is being made; try again in a moment.");
      name = pendingLaunch.chat;
      if (ours()) {
        // a blank chat found again loads again so, its start never showing
        // (one holding a draft stays as it is)
        unload(name);
        chatFrame(name, "/?sent=1");
        openChat(name);
        leaveSidebar();
        view = shell.views();
      }
      // bounded: at most FILES_MAX, uploaded together
      const attachments = await Promise.all(files.map((f) => upload(name, f)));
      await api("POST", `/api/f/${seg(name)}/channels/chat`, { id: pendingLaunch.id, body: { text, ...(attachments.length ? { attachments } : {}) } });
      pendingLaunch = null;
      clearFiles();
    } catch (e) {
      // the chat stays, its start shown again (unless it holds a draft, or
      // the person opened it since), for the same message tried again
      const mine = ours();
      if (name && (mine || state.current !== name || state.page)) unload(name);
      if (mine && state.current && !state.page) openChat(state.current, false);
      await closed;
      launching = false;
      // a connection that dropped is fetch's TypeError, in each browser's words
      const unreached = e instanceof TypeError && /fetch|load failed|network/i.test(e.message);
      const why = unreached ? "Finite couldn't be reached: check your connection and try again." : e.message;
      launchError = `Your message wasn't sent. ${why}`;
      // the launcher again on its words where the person still is; moved
      // on, a notice, and its next opening has them
      if (mine) openSearch(trigger, text);
      else notice("Your message wasn't sent", why);
    } finally {
      launching = false;
    }
  }

  // The input grows with a long prompt.
  function fitSearch() {
    if (!input.offsetParent) return;
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, INPUT_MAX_PX)}px`;
  }
  function updateSearch() {
    fitSearch();
    ++asked; // Invalidate in-flight responses immediately, before the debounce.
    clearTimeout(typing);
    $("search-status").textContent = launchError;
    $("search-send").disabled = !sendable();
    renderFound([], []);
    if (input.value.trim()) typing = setTimeout(search, 150);
  }

  for (const id of ["sidebar-new-chat", "new-chat"]) {
    $(id).setAttribute("aria-keyshortcuts", "Meta+K Control+K");
    $(id).dataset.tooltip = "Search or start a new chat (⌘K)";
    $(id).setAttribute("aria-haspopup", "dialog");
    $(id).setAttribute("aria-controls", "search-dialog");
    $(id).setAttribute("aria-expanded", "false");
    $(id).onclick = () => openSearch($(id));
  }
  addEventListener("keydown", (e) => {
    if (e.isComposing || e.repeat || !(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey || e.code !== "KeyK" || $("layout").hidden) return;
    e.preventDefault();
    if (dialog.open) { input.focus(); return; }
    openSearch($("sidebar-new-chat"));
  });
  // Keyboard events inside the chat iframe cannot bubble to the desktop.
  addEventListener("message", async event => {
    if (event.data?.fragment !== "open-launcher" || $("layout").hidden) return;
    const frame = [...document.querySelectorAll("iframe[data-fragment]")].find(f => f.contentWindow === event.source);
    const name = frame?.dataset.fragment;
    if (!name || !byName(name) || document.activeElement !== frame) return;
    try { if (event.origin !== await originOf(name)) return; } catch { return; }
    if (frame.contentWindow === event.source && document.activeElement === frame) openSearch($("sidebar-new-chat"));
  });
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) closeSearch();
  });
  dialog.addEventListener("cancel", (e) => { e.preventDefault(); closeSearch(); });
  dialog.addEventListener("close", () => {
    // files are a draft of this opening's, as its words are (a send's, kept
    // until it is sent)
    if (!launching && !pendingLaunch) clearFiles();
    ++asked;
    clearTimeout(typing);
    searchTrigger?.setAttribute("aria-expanded", "false");
  });
  // Files dragged over the launcher drop into it, for a person whose agent
  // runs (a new chat takes them); dropped anywhere else in it, they never
  // open in the page.
  let dragDepth = 0;
  const hasFiles = (e) => [...(e.dataTransfer?.types ?? [])].includes("Files");
  const takesFiles = () => canLaunch() && !launching;
  dialog.addEventListener("dragenter", (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    if (dragDepth++ === 0 && takesFiles()) dialog.classList.add("dropping");
  });
  dialog.addEventListener("dragover", (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = takesFiles() ? "copy" : "none";
  });
  dialog.addEventListener("dragleave", (e) => {
    if (!hasFiles(e) || --dragDepth > 0) return;
    dragDepth = 0;
    dialog.classList.remove("dropping");
  });
  dialog.addEventListener("drop", (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth = 0;
    dialog.classList.remove("dropping");
    if (takesFiles()) addFiles([...e.dataTransfer.files]);
    input.focus();
  });
  input.addEventListener("paste", (e) => {
    const files = [...(e.clipboardData?.files ?? [])];
    if (!files.length || !takesFiles()) return;
    e.preventDefault();
    addFiles(files);
  });
  $("search-form").onsubmit = (e) => { e.preventDefault(); startFromSearch(); };
  input.addEventListener("input", () => { launchError = ""; updateSearch(); });
  input.addEventListener("keydown", (e) => {
    if (e.isComposing) return;
    const first = $("search-results").querySelector("button");
    // down from the input's last line goes to the results; Shift+Enter is a new line
    if (e.key === "ArrowDown" && first && input.selectionEnd === input.value.length) { e.preventDefault(); first.focus(); }
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); if (sendable()) startFromSearch(); else first?.click(); }
  });
  $("search-results").addEventListener("keydown", (e) => {
    if (!["ArrowDown", "ArrowUp"].includes(e.key)) return;
    const rows = [...$("search-results").querySelectorAll("button")];
    const index = rows.indexOf(document.activeElement);
    if (index < 0) return;
    e.preventDefault();
    if (e.key === "ArrowUp" && index === 0) input.focus();
    else rows[Math.min(rows.length - 1, index + (e.key === "ArrowDown" ? 1 : -1))]?.focus();
  });

  return {
    open: openSearch,
    agentsRead() {
      if (dialog.open && !launching) $("search-send").disabled = !sendable();
    },
    holds: () => !!pendingLaunch,
  };
}
