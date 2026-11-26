// Settings → Secrets: manage the person's Vault secrets in the browser
// (`GET /api/vault`, `PUT /api/vault/{name}`, `DELETE /api/vault/{name}`).
// Never reads or displays a secret value; a masked input is cleared once
// saved, and when its sheet closes.
import { icon } from "./icons.js";
import { closeOnOutsidePress } from "./dialog.js";

// A Vault secret's placement in plain words.
export function placeLabel(place) {
  if (place?.sign) return place.sign === "nip98" ? "NIP-98 signing" : `Sign (${place.sign})`;
  if (place?.query) return `Query: ${place.query}`;
  if (place?.basic) return `Basic auth (${place.basic})`;
  const bearer = place?.header?.toLowerCase() === "authorization" && (!place.format || place.format === "Bearer {}");
  if (place?.header && !bearer) return place.format && place.format !== "{}" ? `Header: ${place.header} (${place.format})` : `Header: ${place.header}`;
  return "Authorization: Bearer";
}

// Comma- or whitespace-separated DNS hostnames from a text field.
export function parseHosts(raw) {
  return [...new Set(String(raw ?? "").split(/[\s,]+/).map((h) => h.trim().toLowerCase()).filter(Boolean))];
}

function formatDate(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return null;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

export function createSecrets({ api, el, section, named, computer, asks, isOpen, reopenSettings }) {
  // Keep the open detail sheet across reloads in the same settings visit
  // (leaving Settings closes it: connections.js closes every provider-sheet).
  let openSheet = null;
  // This page's writes go one at a time, so each names the revision the
  // one before it left (`written`, below).
  let writing = Promise.resolve();
  // What went wrong is said where the person is looking: in the sheet it
  // belongs to (`ADD`: the Add form) while that sheet is open, and
  // otherwise as a line in the section, kept through the page's redraws
  // until that sheet is opened again (it moves there) or Settings is left.
  // Only in the Settings visit its write began in (`visit`), never a later
  // one. A stale write (409) draws the page again with its sheet open,
  // which says why. Nothing else opens a sheet, so at most one is open.
  const ADD = "";
  const sheets = new Map(); // name -> the newest { sheet, said } drawn for it
  const unread = new Map(); // name -> { text, visit }, said in no sheet yet
  let lines = null; // the section's lines, as last drawn
  function say(name, text, visit) {
    if (!asks.current(visit)) return;
    const shown = sheets.get(name);
    if (shown?.sheet.isConnected && shown.sheet.open) {
      shown.said.textContent = text;
      shown.said.hidden = false;
      return;
    }
    unread.set(name, { text, visit });
    drawLines();
  }
  // what waits for `name` in this visit, taken: its sheet says it now
  function take(name) {
    const u = unread.get(name);
    unread.delete(name);
    drawLines();
    return u && asks.current(u.visit) ? u.text : null;
  }
  function drawLines() {
    if (!lines) return;
    lines.replaceChildren();
    for (const [name, u] of unread) {
      if (!asks.current(u.visit)) {
        unread.delete(name);
        continue;
      }
      const text = name === ADD ? `Add secret: ${u.text}` : u.text.startsWith(name) ? u.text : `${name}: ${u.text}`;
      const line = el("p", "form-error secret-said", text);
      line.dataset.secretSaid = name;
      lines.append(line);
    }
  }
  // a sheet as it opens: what waits for it, or nothing from before
  function opened(name, said) {
    const text = take(name);
    said.textContent = text ?? "";
    said.hidden = !text;
  }

  function secretsSection(vault) {
    const s = section("Secrets");
    s.id = "settings-secrets";
    if (!vault) {
      s.append(el("p", "muted", "Your secrets could not be read."));
      return s;
    }
    s.append(el("p", "muted", "Secrets your agents can send to specific hosts without ever seeing the values."));
    const list = vault.secrets ?? [];
    lines = el("div", "secret-lines");
    s.append(lines);
    const agents = computer()?.agents ?? [];
    if (list.length) {
      const grid = el("div", "provider-grid");
      grid.append(...list.map((sec) => secretCard(sec, agents)));
      s.append(grid);
    } else {
      s.append(el("p", "muted", "No secrets saved yet. Add one here, or let an agent ask when it needs one."));
    }
    const add = addSecretControl(agents, new Set(list.map((sec) => sec.name)));
    s.append(add.button, add.dialog);
    drawLines();
    return s;
  }

  function secretCard(sec, agents) {
    const card = el("article", "provider secret-card");
    card.dataset.secret = sec.name;
    const active = (sec.hosts?.length ?? 0) > 0;
    const place = placeLabel(sec.place);
    const tile = (big) => {
      const t = el("span", `provider-tile${big ? " big" : ""}`);
      t.append(icon("lock"));
      return t;
    };
    const light = () => {
      const st = el("span", "provider-state");
      st.append(el("span", `provider-light${active ? " on" : ""}`));
      return st;
    };
    const body = el("span", "provider-body");
    const sub = el("span", "provider-sub", active ? sec.hosts.join(", ") : "Parked — sent nowhere yet");
    body.append(el("span", "provider-name", sec.name), sub);
    const count = sec.agents?.length ?? 0;
    const whoText = `${count} ${count === 1 ? "agent" : "agents"}`;
    const footText = place === "Authorization: Bearer" ? whoText : `${whoText} · ${place}`;
    const foot = el("span", "provider-foot", footText);

    const open = el("button", "provider-open");
    open.type = "button";
    open.dataset.action = "open";
    open.setAttribute("aria-haspopup", "dialog");
    open.setAttribute("aria-label", `${sec.name}: ${active ? sec.hosts.join(", ") : "Parked"}`);

    // Detail sheet
    const sheet = el("dialog", "provider-sheet secret-sheet");
    sheet.dataset.secretSheet = sec.name;
    sheet.setAttribute("aria-label", sec.name);
    sheet.tabIndex = -1;
    const show = () => {
      sheet.showModal();
      sheet.focus();
    };
    const top = el("div", "provider-sheet-head");
    const titled = el("div");
    titled.append(el("h3", null, sec.name), el("p", "provider-kind", `Vault secret · ${place}`));
    const close = el("button", "icon-button provider-close");
    close.type = "button";
    close.setAttribute("aria-label", "Close");
    close.append(icon("x"));
    close.onclick = () => sheet.close();
    top.append(tile(true), titled, close);

    const inner = el("div", "provider-sheet-body");

    // The secret as this sheet shows it. Every write is onto that one
    // (`held`): a change since (an agent's Allow, another tab, the CLI) is
    // refused (409), never taken in or written over, and the sheet is drawn
    // again from what the Vault now holds. A write's answer is the secret as
    // it now is, which the next one builds on.
    let shown = sec;
    const said = el("p", "form-error secret-said");
    said.hidden = true;
    sheets.set(sec.name, { sheet, said });
    // drawn again after a stale write: this sheet opens, saying why
    const reopen = openSheet?.secret === sec.name && asks.current(openSheet.visit) && isOpen();
    if (reopen) opened(sec.name, said);
    const path = `/api/vault/${encodeURIComponent(sec.name)}`;
    function written(write) {
      const run = writing.then(async () => {
        const saved = await write(shown);
        if (saved?.revision) shown = saved;
      });
      writing = run.catch(() => {});
      return run;
    }
    async function failed(e, fallback, visit) {
      const text = e?.message || fallback;
      if (e?.status !== 409) return say(sec.name, text, visit);
      // stale: drawn again from what the Vault holds, which says why, in
      // the visit its write began in alone
      if (!asks.current(visit)) return;
      unread.set(sec.name, { text, visit });
      openSheet = { secret: sec.name, visit };
      await reopenSettings();
    }

    // Status + two-step remove
    const status = el("div", "provider-status");
    const words = el("span", "provider-status-text");
    words.append(el("span", "provider-state-name", active ? "Active" : "Parked"));
    const when = formatDate(sec.setAt);
    if (when) words.append(el("span", "provider-account", `Value set ${when}`));
    const actions = el("div", "settings-actions");
    function renderRemove(confirming) {
      actions.replaceChildren();
      if (!confirming) {
        const rm = el("button", "quiet", "Remove");
        rm.type = "button";
        rm.dataset.action = "remove-secret";
        rm.onclick = () => renderRemove(true);
        actions.append(rm);
        return;
      }
      const yes = el("button", "quiet", `Remove ${sec.name}?`);
      yes.type = "button";
      yes.dataset.action = "confirm-remove-secret";
      const cancel = el("button", "quiet", "Cancel");
      cancel.type = "button";
      cancel.onclick = () => {
        said.hidden = true;
        renderRemove(false);
      };
      yes.onclick = async () => {
        const visit = asks.now("settings");
        yes.disabled = true;
        cancel.disabled = true;
        said.hidden = true;
        try {
          await written((now) => api("DELETE", now.revision ? `${path}?held=${encodeURIComponent(now.revision)}` : path));
          openSheet = null;
          sheet.close();
          await reopenSettings();
        } catch (e) {
          yes.disabled = false;
          cancel.disabled = false;
          await failed(e, "Could not remove secret", visit);
        }
      };
      actions.append(yes, cancel);
    }
    renderRemove(false);
    status.append(light(), words, actions);
    inner.append(status, said);

    // Which agents may use it
    const knownFragments = new Set(agents.map((a) => a.fragment));
    const listedAgents = [
      ...agents.map((a) => a.fragment),
      ...(sec.agents ?? []).filter((f) => !knownFragments.has(f)),
    ];
    if (listedAgents.length) {
      const who = el("div", "provider-block provider-agents");
      who.append(el("h4", null, "Which agents may use it"));
      for (const fragment of listedAgents) {
        const allowed = (sec.agents ?? []).includes(fragment);
        const row = el("div", "provider-agent");
        const face = el("span", "settings-agent-name");
        face.dataset.agentFace = fragment;
        const flip = el("button", "provider-switch");
        flip.type = "button";
        flip.dataset.agent = fragment;
        flip.setAttribute("aria-pressed", String(allowed));
        named(flip, fragment, (t) => `${t} may use ${sec.name}`, "aria-label");
        flip.onclick = async () => {
          said.hidden = true;
          if (allowed && (sec.agents ?? []).length === 1) {
            said.textContent = "Keep at least one agent, or remove the secret instead.";
            said.hidden = false;
            return;
          }
          const visit = asks.now("settings");
          flip.disabled = true;
          openSheet = { secret: sec.name, visit };
          try {
            await written((now) => {
              const next = allowed ? now.agents.filter((x) => x !== fragment) : [...new Set([...now.agents, fragment])];
              if (!next.length) throw new Error("Keep at least one agent, or remove the secret instead.");
              return api("PUT", path, { hosts: now.hosts, place: now.place, agents: next, held: now.revision });
            });
            await reopenSettings();
          } catch (e) {
            flip.disabled = false;
            await failed(e, "That was not changed", visit);
          }
        };
        row.append(face, flip);
        who.append(row);
      }
      inner.append(who);
    }

    // Allowed hosts (keeps existing sealed value)
    const hostsBlock = el("div", "provider-block");
    hostsBlock.append(el("h4", null, "Allowed hosts"));
    const hostsForm = el("form", "provider-key secret-hosts-form");
    const hostsInput = el("input");
    hostsInput.type = "text";
    hostsInput.autocomplete = "off";
    hostsInput.spellcheck = false;
    hostsInput.value = (sec.hosts ?? []).join(", ");
    hostsInput.placeholder = "api.example.com (blank to park)";
    hostsInput.setAttribute("aria-label", `Allowed hosts for ${sec.name}`);
    const hostsSave = el("button", "quiet", "Save hosts");
    hostsSave.type = "submit";
    hostsForm.append(hostsInput, hostsSave);
    hostsForm.onsubmit = async (e) => {
      e.preventDefault();
      said.hidden = true;
      const visit = asks.now("settings");
      const hosts = parseHosts(hostsInput.value);
      hostsSave.disabled = true;
      openSheet = { secret: sec.name, visit };
      try {
        await written((now) => api("PUT", path, { hosts, place: now.place, agents: now.agents, held: now.revision }));
        await reopenSettings();
      } catch (err) {
        hostsSave.disabled = false;
        await failed(err, "Could not update hosts", visit);
      }
    };
    hostsBlock.append(hostsForm, el("p", "muted provider-detail", "Exact public DNS names, comma-separated (up to 8). Leave empty to park."));
    inner.append(hostsBlock);

    // Replace value (masked; never shows existing value)
    const replaceBlock = el("div", "provider-block");
    replaceBlock.append(el("h4", null, "Replace value"));
    const replaceForm = el("form", "provider-key secret-replace-form");
    const valueInput = el("input");
    valueInput.type = "password";
    valueInput.autocomplete = "off";
    valueInput.spellcheck = false;
    valueInput.dataset.lpignore = "true";
    valueInput.placeholder = "Paste a new value";
    valueInput.setAttribute("aria-label", `New value for ${sec.name}`);
    const replaceSave = el("button", "quiet", "Replace");
    replaceSave.type = "submit";
    replaceForm.append(valueInput, replaceSave);
    replaceForm.onsubmit = async (e) => {
      e.preventDefault();
      said.hidden = true;
      const val = valueInput.value.trim();
      if (!val) {
        said.textContent = "Paste a new value first.";
        said.hidden = false;
        valueInput.focus();
        return;
      }
      const visit = asks.now("settings");
      replaceSave.disabled = true;
      openSheet = { secret: sec.name, visit };
      try {
        await written((now) => api("PUT", path, { value: val, hosts: now.hosts, place: now.place, agents: now.agents, held: now.revision }));
        valueInput.value = "";
        await reopenSettings();
      } catch (err) {
        replaceSave.disabled = false;
        // refused as stale, it is pasted again onto what the sheet then shows
        if (err?.status === 409) valueInput.value = "";
        await failed(err, "Could not replace value", visit);
      }
    };
    replaceBlock.append(replaceForm);
    inner.append(replaceBlock);

    sheet.append(top, inner);
    closeOnOutsidePress(sheet);
    sheet.onclose = () => {
      valueInput.value = "";
      if (openSheet?.secret === sec.name) openSheet = null;
    };
    open.onclick = () => {
      openSheet = { secret: sec.name, visit: asks.now("settings") };
      opened(sec.name, said);
      show();
    };
    card.append(tile(false), body, light(), foot, open, sheet);

    if (reopen) {
      queueMicrotask(() => {
        if (sheet.isConnected && !sheet.open && isOpen()) show();
      });
    }
    return card;
  }

  // `kept`: the names already saved; a PUT sets a secret whole, so Add never
  // takes one of them (its own sheet changes it), nor one saved since
  // (`held: "none"`, which the Vault refuses for a name it keeps)
  function addSecretControl(agents, kept) {
    const button = el("button", "quiet agent-add-open");
    button.type = "button";
    button.dataset.action = "add-secret";
    button.append(icon("plus"), el("span", null, "Add secret"));

    const dialog = el("dialog", "provider-sheet secret-sheet");
    dialog.dataset.addSecret = "1";
    dialog.setAttribute("aria-label", "Add secret");
    dialog.tabIndex = -1;

    const top = el("div", "provider-sheet-head");
    const tile = el("span", "provider-tile big");
    tile.append(icon("lock"));
    const titled = el("div");
    titled.append(el("h3", null, "Add secret"), el("p", "provider-kind", "Stored sealed in your Vault — your agents never see the value."));
    const close = el("button", "icon-button provider-close");
    close.type = "button";
    close.setAttribute("aria-label", "Close");
    close.append(icon("x"));
    close.onclick = () => dialog.close();
    top.append(tile, titled, close);

    const form = el("form", "provider-sheet-body secret-add-form");
    const nameInput = el("input");
    nameInput.type = "text";
    nameInput.autocomplete = "off";
    nameInput.spellcheck = false;
    nameInput.placeholder = "github";
    nameInput.dataset.field = "name";
    const nameLabel = el("label", "secret-field-label");
    nameLabel.append(el("span", null, "Name (lowercase letters, digits, hyphens)"), nameInput);

    const valueInput = el("input");
    valueInput.type = "password";
    valueInput.autocomplete = "off";
    valueInput.spellcheck = false;
    valueInput.dataset.lpignore = "true";
    valueInput.placeholder = "Paste the token or key";
    valueInput.dataset.field = "value";
    const valueLabel = el("label", "secret-field-label");
    valueLabel.append(el("span", null, "Value"), valueInput);

    const hostsInput = el("input");
    hostsInput.type = "text";
    hostsInput.autocomplete = "off";
    hostsInput.spellcheck = false;
    hostsInput.placeholder = "api.github.com (optional; blank to park)";
    hostsInput.dataset.field = "hosts";
    const hostsLabel = el("label", "secret-field-label");
    hostsLabel.append(el("span", null, "Allowed hosts"), hostsInput);

    // Agent switches (default: first agent on when available)
    const selected = new Set(agents.length ? [agents[0].fragment] : []);
    const agentsBlock = el("div", "provider-block provider-agents");
    agentsBlock.append(el("h4", null, "Which agents may use it"));
    if (agents.length) {
      for (const a of agents) {
        const row = el("div", "provider-agent");
        const face = el("span", "settings-agent-name");
        face.dataset.agentFace = a.fragment;
        const flip = el("button", "provider-switch");
        flip.type = "button";
        flip.dataset.agent = a.fragment;
        flip.setAttribute("aria-pressed", String(selected.has(a.fragment)));
        named(flip, a.fragment, (t) => `Allow ${t}`, "aria-label");
        flip.onclick = () => {
          if (selected.has(a.fragment)) selected.delete(a.fragment);
          else selected.add(a.fragment);
          flip.setAttribute("aria-pressed", String(selected.has(a.fragment)));
        };
        row.append(face, flip);
        agentsBlock.append(row);
      }
    } else {
      agentsBlock.append(el("p", "muted provider-detail", "Create an agent first to grant it a secret."));
    }

    const said = el("p", "form-error secret-said");
    said.hidden = true;
    const footer = el("div", "settings-actions secret-form-actions");
    const save = el("button", "primary", "Save secret");
    save.type = "submit";
    save.disabled = !agents.length;
    const cancel = el("button", "quiet", "Cancel");
    cancel.type = "button";
    cancel.onclick = () => dialog.close();
    footer.append(save, cancel);

    form.append(nameLabel, valueLabel, hostsLabel, agentsBlock, said, footer);
    form.onsubmit = async (e) => {
      e.preventDefault();
      said.hidden = true;
      const name = nameInput.value.trim().toLowerCase();
      const value = valueInput.value.trim();
      const chosen = [...selected];
      if (!name) {
        said.textContent = "Enter a name for the secret.";
        said.hidden = false;
        nameInput.focus();
        return;
      }
      if (kept.has(name)) {
        said.textContent = `You already keep ${name}: open it to change it.`;
        said.hidden = false;
        nameInput.focus();
        return;
      }
      if (!value) {
        said.textContent = "Paste the secret value.";
        said.hidden = false;
        valueInput.focus();
        return;
      }
      if (!chosen.length) {
        said.textContent = "Choose at least one agent.";
        said.hidden = false;
        return;
      }
      const visit = asks.now("settings");
      save.disabled = true;
      try {
        await api("PUT", `/api/vault/${encodeURIComponent(name)}`, {
          value,
          hosts: parseHosts(hostsInput.value),
          agents: chosen,
          held: "none",
        });
        valueInput.value = "";
        nameInput.value = "";
        hostsInput.value = "";
        dialog.close();
        await reopenSettings();
      } catch (err) {
        save.disabled = false;
        say(ADD, err.message || "Could not save secret", visit);
      }
    };

    dialog.append(top, form);
    closeOnOutsidePress(dialog);
    dialog.onclose = () => {
      valueInput.value = "";
    };
    button.onclick = () => {
      opened(ADD, said);
      dialog.showModal();
      nameInput.focus();
    };
    sheets.set(ADD, { sheet: dialog, said });
    return { button, dialog };
  }

  return { secretsSection };
}
