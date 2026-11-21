// The passkey door's page (passkeys.rs): sign in, enroll from an invite,
// add a passkey, recover with a code. Served by the door itself from its
// own origin; it loads nothing else. The browser's WebAuthn does the
// ceremony; the door checks it.
"use strict";

const main = document.querySelector("main");
const base = main.dataset.base;
const say = (text, bad) => {
  const p = document.getElementById("said");
  p.textContent = text;
  p.className = bad ? "bad" : "";
};

// base64url <-> bytes, for browsers without the JSON helpers
const b64 = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const unb64 = (s) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((s.length + 3) % 4)), (c) => c.charCodeAt(0));

function creationOptions(o) {
  if (window.PublicKeyCredential?.parseCreationOptionsFromJSON) return PublicKeyCredential.parseCreationOptionsFromJSON(o);
  return { ...o, challenge: unb64(o.challenge), user: { ...o.user, id: unb64(o.user.id) }, excludeCredentials: (o.excludeCredentials || []).map((c) => ({ ...c, id: unb64(c.id) })) };
}
function requestOptions(o) {
  if (window.PublicKeyCredential?.parseRequestOptionsFromJSON) return PublicKeyCredential.parseRequestOptionsFromJSON(o);
  return { ...o, challenge: unb64(o.challenge), allowCredentials: (o.allowCredentials || []).map((c) => ({ ...c, id: unb64(c.id) })) };
}
function asJSON(cred) {
  if (cred.toJSON) return cred.toJSON();
  const r = cred.response;
  const response = { clientDataJSON: b64(r.clientDataJSON) };
  if (r.attestationObject) response.attestationObject = b64(r.attestationObject);
  if (r.authenticatorData) response.authenticatorData = b64(r.authenticatorData);
  if (r.signature) response.signature = b64(r.signature);
  if (r.userHandle) response.userHandle = b64(r.userHandle);
  return { id: cred.id, rawId: b64(cred.rawId), type: cred.type, response, clientExtensionResults: cred.getClientExtensionResults() };
}

async function post(path, body) {
  const r = await fetch(base + path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const j = await r.json().catch(() => ({ error: "the door did not answer" }));
  if (!r.ok) throw new Error(j.error || "refused");
  return j;
}
const create = async (options) => asJSON(await navigator.credentials.create({ publicKey: creationOptions(options.publicKey) }));
const get = async (options) => asJSON(await navigator.credentials.get({ publicKey: requestOptions(options.publicKey) }));

function busy(form, on) {
  for (const b of form.querySelectorAll("button")) b.disabled = on;
}

async function run(form, steps) {
  busy(form, true);
  say("");
  try {
    await steps();
  } catch (e) {
    say(e.name === "NotAllowedError" ? "The passkey was not used: try again." : e.message, true);
  } finally {
    busy(form, false);
  }
}

function showCodes(codes, next) {
  const box = document.getElementById("codes");
  box.hidden = false;
  box.querySelector("pre").textContent = codes.join("\n");
  const a = box.querySelector("a");
  a.href = next;
}

const signin = document.getElementById("signin");
signin?.addEventListener("submit", (ev) => {
  ev.preventDefault();
  run(signin, async () => {
    const begun = await post("/passkeys/signin/begin", { state: main.dataset.state, email: signin.email.value });
    const done = await post("/passkeys/signin/finish", { flow: begun.flow, credential: await get(begun.options) });
    say("Signed in.");
    location.assign(done.redirect);
  });
});

const enroll = document.getElementById("enroll");
enroll?.addEventListener("submit", (ev) => {
  ev.preventDefault();
  run(enroll, async () => {
    const begun = await post("/passkeys/enroll/begin", { invite: main.dataset.invite });
    const done = await post("/passkeys/enroll/finish", { flow: begun.flow, credential: await create(begun.options) });
    enroll.hidden = true;
    say("Your passkey is made.");
    showCodes(done.recovery_codes, done.continue);
  });
});

const add = document.getElementById("add");
add?.addEventListener("submit", (ev) => {
  ev.preventDefault();
  run(add, async () => {
    const begun = await post("/passkeys/add/begin", { email: add.email.value });
    const proved = await post("/passkeys/add/prove", { flow: begun.flow, credential: await get(begun.options) });
    await post("/passkeys/add/finish", { flow: proved.flow, credential: await create(proved.options) });
    say("The new passkey is added.");
  });
});

const recover = document.getElementById("recover");
recover?.addEventListener("submit", (ev) => {
  ev.preventDefault();
  run(recover, async () => {
    const begun = await post("/passkeys/recover/begin", { email: recover.email.value, code: recover.code.value });
    await post("/passkeys/recover/finish", { flow: begun.flow, credential: await create(begun.options) });
    say("A new passkey is made, and that code is spent.");
  });
});
