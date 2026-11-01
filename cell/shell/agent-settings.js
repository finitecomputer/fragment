import { avatar } from "./agent-identity.js";
import { providerName } from "./connections.js";

export const DEFAULT_AGENT_TIER = "cheap";
// An agent's role, at most (proto's `ROLE_MAX_CHARS`), and the agents one
// computer runs (the cell's `AGENTS_MAX`): copies, each held to its source
// by a host test (xtask/src/guards.rs, `the_shell_copies_its_limits`).
const ROLE_MAX_CHARS = 48;
const AGENTS_MAX = 16;

export function createAgentSettings({ api, el, seg, byName, agentFace, titleOf, agents, named, rename, load, nameScreen, renderNames, computer, homePicker, addAgent, primaryAgent, chats, own, me }) {
  // ---- an agent's name and picture (docs/api.md, "An agent's name and
  // picture"): its agent fragment's title and its avatar.png, written and
  // put live as any of its files are (POST …/files, then …/deploy). Every
  // page and the platform show it by them; the rows here follow the list. ----
  const AGENT_PICTURE = "avatar.png";
  // What the platform takes (fragment_core::face): a PNG, JPEG, WebP or GIF
  // of at most 256 KiB. A larger one, or any other image the browser reads,
  // is drawn again at most PICTURE_PX square: a PNG, else a JPEG, that fits.
  const PICTURE_MAX_BYTES = 256 * 1024;
  const PICTURE_PX = 512;
  async function pictureOf(file) {
    if (file.size <= PICTURE_MAX_BYTES && /^image\/(png|jpeg|webp|gif)$/.test(file.type)) return file;
    let bitmap;
    try {
      bitmap = await createImageBitmap(file);
    } catch {
      throw new Error("That file is not a picture: choose a PNG, JPEG, WebP or GIF.");
    }
    const scale = Math.min(1, PICTURE_PX / Math.max(bitmap.width, bitmap.height));
    const canvas = el("canvas");
    canvas.width = Math.max(1, Math.round(bitmap.width * scale));
    canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    for (const [type, quality] of [["image/png"], ["image/jpeg", 0.9], ["image/jpeg", 0.7]]) {
      const blob = await new Promise((done) => canvas.toBlob(done, type, quality));
      if (blob && blob.size <= PICTURE_MAX_BYTES) return blob;
    }
    throw new Error("That picture stays over 256 KiB however it is drawn: choose a smaller one.");
  }
  async function base64Of(blob) {
    const bytes = new Uint8Array(await blob.arrayBuffer());
    let text = "";
    for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return btoa(text);
  }
  // The agent's picture from `file`, or none (null), put live.
  async function setPicture(agent, file) {
    const write = file ? { path: AGENT_PICTURE, base64: await base64Of(await pictureOf(file)) } : { path: AGENT_PICTURE, delete: true };
    await api("POST", `/api/f/${seg(agent)}/files`, { message: file ? "its picture" : "no picture", files: [write] });
    await api("POST", `/api/f/${seg(agent)}/deploy`, {});
  }
  // An agent removed (FIN-250): the chats it is the last live agent of are
  // archived first, so they are in History, read and continued with another
  // agent (its membership kept, so its words stay its), and then its
  // fragment is deleted: it leaves its computer, and its routines (its own
  // schedules) stop with it (docs/api.md, DELETE /api/f/{name}). Archived
  // first: a delete that fails leaves it as it was but for those chats,
  // which Remove again finds archived, and the person can unarchive.
  async function removeAgent(a) {
    const live = liveOwn();
    const last = [];
    for (const f of chats().filter((f) => own(f) && !f.archived && (f.agents ?? []).includes(a.identity))) {
      if (!(await keepsAnAgent(f, a, live))) last.push(f);
    }
    for (const f of last) await api("PUT", `/api/fragments/${seg(f.name)}/archived`, { archived: true });
    await api("DELETE", `/api/f/${seg(a.fragment)}`);
    await load();
  }
  // The person's own agents still theirs: run by their computer and in
  // their list (one removed leaves the list at once, its computer a moment
  // after: Continue's boundary too).
  const liveOwn = () => new Set([...agents().values()].filter((x) => x.fragment && byName(x.fragment)).map((x) => x.identity));
  // Whether `chat` keeps a live agent once `a` is gone: one of theirs still
  // theirs, or another person's. A chat's agents are its agent members,
  // and a removed agent keeps its membership (for whose words were its), so
  // one not live of theirs is told from another person's by the chat's
  // members' owners, asked only then.
  async function keepsAnAgent(chat, a, live) {
    const others = (chat.agents ?? []).filter((id) => id !== a.identity);
    if (others.some((id) => live.has(id))) return true;
    if (!others.length) return false;
    const { members = [] } = await api("GET", `/api/f/${seg(chat.name)}/members`);
    const mine = me()?.id;
    return members.some((m) => m.kind === "agent" && others.includes(m.principal) && !!m.owner && m.owner !== mine);
  }
  // Whether an agent may be removed here: one of several, never the main
  // agent (the first of theirs, whom new chats go to).
  const removable = (a) => {
    const live = liveOwn();
    return live.has(a.identity) && live.size > 1 && primaryAgent()?.fragment !== a.fragment;
  };
  // An agent's row's Rename, its picture (Add, Change, Remove), Remove agent
  // (asked once more, saying what it does), and a line that says what they
  // did or why not.
  function faceControls(a) {
    const said = el("p", "muted agent-face-said");
    said.hidden = true;
    const tell = (text) => {
      said.textContent = text;
      said.hidden = !text;
    };
    const renameIt = el("button", "quiet", "Rename");
    renameIt.type = "button";
    renameIt.dataset.renameAgent = a.fragment;
    renameIt.onclick = () => rename(a.fragment);
    const picture = el("label", "quiet picture");
    const label = el("span");
    label.dataset.pictureLabel = a.fragment;
    const file = el("input");
    file.type = "file";
    file.accept = "image/png,image/jpeg,image/webp,image/gif";
    file.dataset.pictureFor = a.fragment;
    file.hidden = true;
    picture.append(label, file);
    const remove = el("button", "quiet", "Remove picture");
    remove.type = "button";
    remove.dataset.pictureRemove = a.fragment;
    const change = async (chosen) => {
      for (const b of [renameIt, remove, file]) b.disabled = true;
      tell(chosen ? "Saving its picture…" : "Taking its picture away…");
      try {
        await setPicture(a.fragment, chosen);
        tell("");
        await load();
      } catch (e) {
        tell(e.message);
      } finally {
        for (const b of [renameIt, remove, file]) b.disabled = false;
        file.value = "";
      }
    };
    file.onchange = () => file.files?.[0] && change(file.files[0]);
    remove.onclick = () => change(null);
    const actions = el("span", "agent-face-actions");
    actions.append(renameIt, picture, remove);
    if (removable(a)) {
      const ask = el("button", "quiet", "Remove agent");
      ask.type = "button";
      ask.dataset.removeAgent = a.fragment;
      ask.onclick = () => {
        const what = el("span", "muted agent-remove-what", "Its chats go to History. Its routines stop.");
        const sure = el("button", "quiet", `Remove ${titleOf(a.fragment)}`);
        sure.type = "button";
        sure.dataset.removeAgentSure = a.fragment;
        const cancel = el("button", "quiet", "Cancel");
        cancel.type = "button";
        cancel.onclick = () => actions.replaceChildren(renameIt, picture, remove, ask);
        sure.onclick = async () => {
          for (const b of [sure, cancel]) b.disabled = true;
          tell(`Removing ${titleOf(a.fragment)}…`);
          try {
            await removeAgent(a);
            // its row goes with it
            const row = document.querySelector(`.agent-setting[data-agent="${CSS.escape(a.fragment)}"]`);
            row?.nextElementSibling === said && said.remove();
            row?.remove();
          } catch (e) {
            tell(e.message);
            actions.replaceChildren(renameIt, picture, remove, ask);
          }
        };
        actions.replaceChildren(what, sure, cancel);
      };
      actions.append(ask);
    }
    return [actions, said];
  }
  // Settings' agent rows as the list says now: each one's face and name, and
  // whether it has a picture to change or remove; and its open screens. The
  // names its other controls carry are `named`'s.
  function renderAgentFaces() {
    for (const f of document.querySelectorAll("iframe[data-screen]")) nameScreen(f);
    for (const name of document.querySelectorAll("[data-agent-face]")) {
      const fragment = name.dataset.agentFace;
      const id = [...agents().values()].find((a) => a.fragment === fragment)?.identity ?? null;
      const face = agentFace(fragment, id);
      const shown = JSON.stringify([face.picture, titleOf(fragment)]);
      if (name.dataset.shown === shown) continue;
      name.dataset.shown = shown;
      name.replaceChildren(avatar(face, "small"), titleOf(fragment));
    }
    for (const label of document.querySelectorAll("[data-picture-label]")) label.textContent = byName(label.dataset.pictureLabel)?.picture ? "Change picture" : "Add a picture";
    for (const remove of document.querySelectorAll("[data-picture-remove]")) remove.hidden = !byName(remove.dataset.pictureRemove)?.picture;
    renderNames();
  }

  // ---- an agent's settings in Settings' Agents: its row, then its
  // agent.json's picks, read once for them all. Each pick is written as the
  // others are, one write at a time (each reads what the one before wrote),
  // and put live, as its name and picture are: its computer reads main, its
  // page live (`__file`), and both then say the same. Its computer reads
  // agent.json again within seconds and runs its next turn so. A pick is
  // written only onto agent.json as it is now, read just before (none there,
  // 404, is nothing to keep): a read that fails otherwise writes nothing and
  // says so, so no other setting of the agent (its tier, its model) is lost.
  const agentJson = (agent) => api("GET", `/api/f/${seg(agent)}/file?path=agent.json`);
  const UNREAD_SETTINGS = "Couldn't read this agent's settings, so nothing was changed. Try again.";
  // One write of an agent's agent.json at a time, each on what the one before
  // wrote: `change` sets its pick on what agent.json holds now (its tier
  // kept), and says what changed.
  const agentJsonWrites = new Map();
  function writeAgentJson(agent, change) {
    const written = (agentJsonWrites.get(agent) ?? Promise.resolve()).catch(() => {}).then(async () => {
      const now = await agentJson(agent).catch((e) => {
        if (e.status === 404) return {};
        throw new Error(UNREAD_SETTINGS);
      });
      const next = { ...now, tier: now.tier ?? DEFAULT_AGENT_TIER };
      const message = change(next);
      // (none: as it is already, so nothing written)
      if (message === null) return;
      await api("POST", `/api/f/${seg(agent)}/files`, { message, files: [{ path: "agent.json", text: JSON.stringify(next, null, 2) + "\n" }] });
      await api("POST", `/api/f/${seg(agent)}/deploy`, {});
    });
    agentJsonWrites.set(agent, written);
    return written;
  }
  // ---- an agent's role (its fragment.json's `meta.role`; Paul, 2026-10-30):
  // the tag its chats' rows show beside its name, one source, written and
  // put live as its name is; a role taken away is none. The rows follow the
  // list, never this field. ----
  async function setRole(agent, role) {
    const manifest = await api("GET", `/api/f/${seg(agent)}/manifest`);
    const meta = { ...(manifest.meta ?? {}) };
    if (role) meta.role = role;
    else delete meta.role;
    manifest.meta = meta;
    await api("POST", `/api/f/${seg(agent)}/files`, { message: role ? "its role" : "no role", files: [{ path: "fragment.json", text: JSON.stringify(manifest, null, 2) + "\n" }] });
    await api("POST", `/api/f/${seg(agent)}/deploy`, {});
  }
  function roleField(a, said) {
    const field = el("input", "agent-role");
    field.type = "text";
    field.maxLength = ROLE_MAX_CHARS;
    field.placeholder = "e.g. chief of staff";
    field.dataset.roleFor = a.fragment;
    field.value = byName(a.fragment)?.agentRole ?? "";
    named(field, a.fragment, (t) => `${t}'s role`, "aria-label");
    field.onchange = async () => {
      const role = field.value.trim();
      field.disabled = true;
      said.textContent = "Saving…";
      try {
        await setRole(a.fragment, role);
        said.textContent = "Saved.";
        await load();
      } catch (e) {
        said.textContent = e.message;
        field.value = byName(a.fragment)?.agentRole ?? "";
      } finally {
        field.disabled = false;
      }
    };
    const label = settingLabel("Role", field);
    label.classList.add("agent-role-label");
    return label;
  }
  // ---- another agent (Paul, 2026-10-30): its name and role, then it is
  // made as the first was, in its own chat (onboarding.js `addAgent`),
  // which opens. None past what one computer runs. ----
  function addAgentForm() {
    if (agents().size >= AGENTS_MAX) return [];
    const form = el("form", "agent-add");
    const name = el("input", "agent-add-name");
    name.type = "text";
    name.maxLength = 48;
    name.placeholder = "Name";
    name.setAttribute("aria-label", "New agent's name");
    const role = el("input", "agent-add-role");
    role.type = "text";
    role.maxLength = ROLE_MAX_CHARS;
    role.placeholder = "Role, e.g. dev";
    role.setAttribute("aria-label", "New agent's role");
    const add = el("button", "quiet", "Add agent");
    add.type = "submit";
    const said = el("p", "muted agent-face-said");
    said.hidden = true;
    form.onsubmit = async (event) => {
      event.preventDefault();
      if (!name.value.trim()) return name.focus();
      add.disabled = true;
      try {
        await addAgent({ name: name.value.trim(), role: role.value.trim() });
      } catch (e) {
        said.textContent = e.message;
        said.hidden = false;
      } finally {
        add.disabled = false;
      }
    };
    form.append(name, role, add);
    return [form, said];
  }
  function agentSettings(a, row, modeled) {
    const box = el("div", "agent-setting");
    box.dataset.agent = a.fragment;
    const said = el("span", "muted agent-model-said");
    // what the picks show: agent.json, none (404: the defaults), or unread (null)
    const read = agentJson(a.fragment).catch((e) => (e.status === 404 ? {} : null));
    read.then((j) => { if (j === null) said.textContent = "Couldn't read this agent's settings: reload to see them."; });
    const write = (change) => writeAgentJson(a.fragment, change);
    // the model pick: when there may be a choice, a route the deployment
    // names (Private, Standard) or a model of a provider the person
    // connected; whether there is one, with a single route, is the agent's
    // own tier's to say (once agent.json is read)
    const picks = [roleField(a, said), ...modelPickers(a, modeled, read, write, said), ...homePicker(a, said)];
    box.append(row, ...picks, said);
    return box;
  }
  // a pick (`value`) being written (`written`), said under its agent's
  // settings; one not written shows again what agent.json holds (`was`)
  async function savePick(pick, value, said, written, saved = "Saved: its next message uses it.") {
    pick.disabled = true;
    delete pick.dataset.saved;
    said.textContent = "Saving…";
    try {
      await written;
      said.textContent = saved;
      pick.dataset.saved = value;
      pick.dataset.was = value;
      return true;
    } catch (e) {
      said.textContent = e.message;
      pick.value = pick.dataset.was ?? "";
      return false;
    } finally {
      pick.disabled = false;
    }
  }
  function settingLabel(key, pick) {
    const label = el("label", "agent-model-label");
    label.append(el("span", "settings-key", key), pick);
    return label;
  }
  function modelOption(value, text) {
    const o = el("option", null, text);
    o.value = value;
    return o;
  }

  // ---- an agent's model: its agent.json's `model` ({provider, id}), a model
  // of a provider the person connected, or none: its `tier`'s (`medium`, else
  // the default: `high` is off, decision 23), as their computer's view names
  // it (the deployment's `models`). A tier the deployment labels is a route
  // a person picks (Paul, 2026-10-24: "Private (recommended)", "Standard"):
  // each labelled tier is offered by its label and its model's id, and
  // picked, it is the agent's `tier`, its own model gone. None labelled, the
  // agent's own tier is shown by its name and its model's id, as before.
  // Vision and voice notes stay on the platform's own models.
  const TIER = { cheap: "Cheap", medium: "Medium" };
  const tierOf = (tier) => (tier === "medium" ? "medium" : DEFAULT_AGENT_TIER);
  const routesOffered = () => {
    const routes = computer()?.models?.routes;
    return routes ? ["cheap", "medium"].filter((t) => routes[t]?.label) : [];
  };
  const tierName = (tier) => {
    const t = tierOf(tier);
    const id = computer()?.models?.[t];
    const label = computer()?.models?.routes?.[t]?.label;
    if (label) return id ? `${label} · ${id}` : label;
    return id ? `${TIER[t]} (${id})` : TIER[t];
  };
  // a route's value: no provider's name has a colon (a model's is `provider id`)
  const routeValue = (tier) => `route:${tierOf(tier)}`;
  const modelValue = (provider, id) => `${provider} ${id}`;
  // BYO keeps the legacy model + reasoning as Fast. Deep is an optional
  // nested choice, ignored by old images; without one it inherits Fast.
  const REASONING = [["low", "Low"], ["high", "High"], ["max", "Max"]];
  const effortOf = (value, fallback = "high") => REASONING.some(([v]) => v === value) ? value : fallback;
  function ownChoice(value) {
    if (typeof value?.provider !== "string" || typeof value?.id !== "string") return null;
    const { provider, id } = value;
    if (!provider.length || provider.length > 32 || /[^a-z0-9-]/.test(provider)) return null;
    if (!id.length || id.length > 128 || /[^A-Za-z0-9._:/@-]/.test(id)) return null;
    return { provider, id };
  }
  function ownModes(j) {
    const fast = ownChoice(j?.model);
    if (!fast) return null;
    const reasoning = effortOf(j.reasoning);
    const saved = j.model.deep;
    const deep = ownChoice({ ...saved, provider: saved?.provider === undefined ? fast.provider : saved.provider });
    return { fast: { ...fast, reasoning }, deep: deep ? { ...deep, reasoning: effortOf(saved.reasoning, reasoning) } : { ...fast, reasoning } };
  }
  function modelPickers(a, modeled, read, write, said) {
    const offered = routesOffered();
    const fast = el("select", "agent-model"), deep = el("select", "agent-model");
    fast.dataset.modelFor = deep.dataset.deepModelFor = a.fragment;
    const effort = (mode) => {
      const pick = el("select", "agent-model agent-effort");
      pick.dataset[mode === "Fast" ? "reasoningFor" : "deepReasoningFor"] = a.fragment;
      named(pick, a.fragment, (t) => `${t}'s ${mode} reasoning`, "aria-label");
      for (const [value, text] of REASONING) pick.append(modelOption(value, text));
      return pick;
    };
    const fastEffort = effort("Fast"), deepEffort = effort("Deep");
    const row = (key, model, level) => {
      const line = el("div", "agent-model-label");
      const picks = el("span", "agent-mode-picks");
      picks.append(model, level);
      line.append(el("span", "settings-key", key), picks);
      return line;
    };
    const fastRow = row("Model", fast, fastEffort), deepRow = row("Deep", deep, deepEffort);
    deepRow.classList.add("has-modes");
    deepRow.hidden = fastEffort.hidden = true;
    const controls = [fast, deep, fastEffort, deepEffort];
    for (const pick of controls) pick.disabled = true;
    named(deep, a.fragment, (t) => `${t}'s Deep model`, "aria-label");
    const fill = (pick, choice, tier, routes) => {
      pick.replaceChildren();
      if (routes) pick.append(...(offered.length ? offered.map((t) => modelOption(routeValue(t), tierName(t))) : [modelOption("", tierName(tier))]));
      for (const p of modeled) {
        const group = el("optgroup");
        group.label = `${providerName(p.provider)} (your account)`;
        for (const m of p.models) group.append(modelOption(modelValue(p.provider, m.id), m.name));
        pick.append(group);
      }
      const value = choice ? modelValue(choice.provider, choice.id) : routes && offered.length ? routeValue(tier) : "";
      if (value && ![...pick.options].some((o) => o.value === value)) pick.append(modelOption(value, choice ? choice.id : tierName(tier)));
      pick.value = pick.dataset.was = value;
    };
    const show = (j) => {
      const modes = ownModes(j);
      fill(fast, modes?.fast, j?.tier, true);
      fill(deep, modes?.deep, j?.tier, false);
      fastEffort.value = fastEffort.dataset.was = modes?.fast.reasoning ?? "high";
      deepEffort.value = deepEffort.dataset.was = modes?.deep.reasoning ?? "high";
      fastEffort.hidden = deepRow.hidden = !modes;
      fastRow.classList.toggle("has-modes", !!modes);
      fastRow.querySelector(".settings-key").textContent = modes ? "Fast" : "Model";
      fastRow.hidden = !modes && modeled.length === 0 && fast.options.length < 2;
      named(fast, a.fragment, (t) => `${t}'s ${modes ? "Fast model" : "model"}`, "aria-label");
      for (const pick of controls) { pick.disabled = j === null; pick.dataset.read = "1"; }
    };
    const choose = async (pick, change) => {
      let applied;
      const value = pick.value;
      const written = write((next) => { const message = change(next, value); applied = next; return message; });
      if (await savePick(pick, value, said, written)) show(applied);
    };
    const requireOwn = (next) => {
      const modes = ownModes(next);
      if (!modes) throw new Error("The model changed. Reload these settings and try again.");
      return modes;
    };
    fast.onchange = () => choose(fast, (next, value) => {
      if (!value || value.startsWith("route:")) {
        if (value) next.tier = tierOf(value.slice(6));
        delete next.model;
        return `its route: ${computer()?.models?.routes?.[next.tier]?.label ?? next.tier}`;
      }
      const [provider, id] = value.split(" ");
      const choice = ownChoice({ provider, id });
      if (!choice) return null;
      const modes = ownModes(next);
      const prior = modes ? next.model : {};
      next.model = { ...prior, ...choice, deep: { ...prior.deep, ...(modes?.deep ?? { ...choice, reasoning: effortOf(next.reasoning) }) } };
      return `its Fast model: ${id}`;
    });
    deep.onchange = () => choose(deep, (next, value) => {
      const modes = requireOwn(next);
      const [provider, id] = value.split(" ");
      const choice = ownChoice({ provider, id });
      if (!choice) return null;
      next.model.deep = { ...next.model.deep, ...modes.deep, ...choice };
      return `its Deep model: ${id}`;
    });
    for (const [mode, pick] of [["Fast", fastEffort], ["Deep", deepEffort]]) pick.onchange = () => choose(pick, (next, value) => {
      const modes = requireOwn(next);
      next.model.deep = { ...next.model.deep, ...modes.deep };
      if (mode === "Fast") next.reasoning = effortOf(value);
      else next.model.deep.reasoning = effortOf(value);
      return `its ${mode} reasoning: ${value}`;
    });
    read.then(show);
    return [fastRow, deepRow];
  }
  return { faceControls, renderAgentFaces, agentSettings, addAgentForm, agentJson, writeAgentJson, savePick, settingLabel, modelOption, removeAgent, removable };
}
