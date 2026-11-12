package main

import (
	"fmt"
	"net/http"
	"strings"
)

func page(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	fmt.Fprint(w, strings.ReplaceAll(pageHTML, "{{P}}", platformOf(hostOf(r))))
}

func frame(w http.ResponseWriter, r *http.Request) {
	// The chat frame's own cookies, each SameSite flavour, host-only on the fragment.
	if len(r.Cookies()) == 0 {
		v := rnd()
		w.Header().Add("Set-Cookie", "ff_lax="+v+"; Path=/; Secure; HttpOnly; SameSite=Lax")
		w.Header().Add("Set-Cookie", "ff_none="+v+"; Path=/; Secure; HttpOnly; SameSite=None")
		w.Header().Add("Set-Cookie", "ff_part="+v+"; Path=/; Secure; HttpOnly; SameSite=None; Partitioned")
		w.Header().Add("Set-Cookie", "ff_strict="+v+"; Path=/; Secure; HttpOnly; SameSite=Strict")
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	fmt.Fprint(w, strings.ReplaceAll(frameHTML, "{{P}}", platformOf(hostOf(r))))
}

const pageHTML = `<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>tailnet spike</title>
<link rel="stylesheet" href="/static/a.css">
<script src="/static/a.js"></script>
<script src="https://cdn.{{P}}/static/a.js"></script>
<style>body{font:13px -apple-system,system-ui,sans-serif;margin:12px}td{padding:2px 6px;vertical-align:top}
.ok{color:#080}.bad{color:#b00}iframe{width:100%;height:60px;border:1px solid #ccc}code{word-break:break-all}</style>
</head><body>
<div id="css-probe"></div>
<h3>tailnet spike: {{P}}</h3>
<div id="state">running…</div>
<table id="t"></table>
<img id="img1" src="/static/i.svg"> <img id="img2" src="https://cdn.{{P}}/static/i.svg">
<iframe id="chat" src="https://chat.{{P}}/frame"></iframe>
<script>
const P = "{{P}}";
const q = new URLSearchParams(location.search);
const LONG = +(q.get("long") || 65), UPMB = +(q.get("up") || 25), RUN = q.get("run") || "";
if (!document.cookie.includes("sid_js=")) {
  sessionStorage.setItem("loginHop", String(Date.now()));
  location.replace("/login?next=" + encodeURIComponent(location.pathname + location.search));
}
const R = {run: RUN, ua: navigator.userAgent, href: location.href, t0: performance.timeOrigin, checks: {}};
const t = document.getElementById("t");
function rec(name, ok, detail) {
  R.checks[name] = {ok, detail, at: Math.round(performance.now())};
  const tr = document.createElement("tr");
  tr.innerHTML = "<td class=" + (ok ? "ok" : "bad") + ">" + (ok ? "✓" : "✗") + "</td><td>" + name + "</td><td><code></code></td>";
  tr.querySelector("code").textContent = typeof detail === "string" ? detail : JSON.stringify(detail);
  t.appendChild(tr);
  try { webkit.messageHandlers.spike.postMessage(JSON.stringify({check: name, ok, detail})); } catch (e) {}
}
const timeout = (ms, what) => new Promise((_, rej) => setTimeout(() => rej(new Error("timeout " + what)), ms));
async function j(url, opts) { const r = await fetch(url, opts); return r.json(); }

const frameMsg = new Promise(res => addEventListener("message", e => {
  if (e.origin === "https://chat." + P) res(e.data);
}));

async function wsTest(url, seconds) {
  const t0 = performance.now();
  const ws = new WebSocket(url);
  let ticks = 0, echoes = 0, openMs = 0;
  await Promise.race([new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error("ws error")); }), timeout(15000, "ws open")]);
  openMs = Math.round(performance.now() - t0);
  ws.onmessage = e => { if (e.data.startsWith("{\"tick")) ticks++; else echoes++; };
  for (let i = 0; i < 3; i++) ws.send("hello " + i);
  let closed = null;
  ws.onclose = e => { closed = {code: e.code, at: Math.round((performance.now() - t0) / 1000)}; };
  await new Promise(res => setTimeout(res, seconds * 1000));
  const state = ws.readyState;
  ws.close();
  return {openMs, echoes, ticks, seconds, stillOpen: state === 1, closed};
}

async function main() {
  const hop = sessionStorage.getItem("loginHop");
  rec("login redirect set cookies", document.cookie.includes("sid_js="), hop ? "via /login hop" : "already signed in");
  const nav = performance.getEntriesByType("navigation")[0];
  if (nav) R.nav = {dns: Math.round(nav.domainLookupEnd - nav.domainLookupStart), connect: Math.round(nav.connectEnd - nav.connectStart),
    tls: nav.secureConnectionStart ? Math.round(nav.connectEnd - nav.secureConnectionStart) : 0, ttfb: Math.round(nav.responseStart - nav.requestStart),
    total: Math.round(nav.responseEnd - nav.startTime), proto: nav.nextHopProtocol};
  await new Promise(res => document.readyState === "complete" ? res() : addEventListener("load", res));
  rec("subresource: script same-origin + cross-origin", window.__scriptLoaded === 2, "loaded " + window.__scriptLoaded);
  rec("subresource: stylesheet", getComputedStyle(document.getElementById("css-probe")).width === "7px", getComputedStyle(document.getElementById("css-probe")).width);
  rec("subresource: images same/cross-origin", img1.naturalWidth === 3 && img2.naturalWidth === 3, [img1.naturalWidth, img2.naturalWidth]);

  try { const w = await j("/whoami"); R.remote = w.remote; R.whoami = w;
    rec("fetch same-origin (platform cookies)", w.cookies.includes("sid") && w.cookies.includes("sid_js"), w);
  } catch (e) { rec("fetch same-origin (platform cookies)", false, String(e)); }
  try { const w = await j("https://api." + P + "/api/cors", {credentials: "include"});
    rec("fetch cross-origin CORS (fragment host)", true, w);
  } catch (e) { rec("fetch cross-origin CORS (fragment host)", false, String(e)); }

  try { const f = await Promise.race([frameMsg, timeout(20000, "frame")]); R.frame = f;
    rec("cross-origin chat frame loaded + reported", true, f);
    rec("frame: own cookies (lax/none/part/strict) on same-origin fetch", f.echo && ["ff_lax","ff_none","ff_part","ff_strict"].every(c => f.echo.cookies.includes(c)), f.echo && f.echo.cookies);
    rec("frame: WebSocket", !!(f.ws && f.ws.echo), f.ws);
    rec("frame: credentialed fetch to platform carries platform cookie (same-site)", !!(f.platform && f.platform.cookies && f.platform.cookies.includes("sid")), f.platform);
  } catch (e) { rec("cross-origin chat frame loaded + reported", false, String(e)); }
  try { const w = await j("https://chat." + P + "/echo", {credentials: "include"});
    rec("platform → chat host credentialed fetch sees frame cookies", w.cookies.length > 0, w.cookies);
  } catch (e) { rec("platform → chat host credentialed fetch sees frame cookies", false, String(e)); }

  // Upload
  try {
    const buf = new Uint8Array(UPMB * 1024 * 1024);
    for (let i = 0; i < buf.length; i += 65536) crypto.getRandomValues(buf.subarray(i, Math.min(i + 65536, buf.length)));
    const want = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", buf))).map(b => b.toString(16).padStart(2, "0")).join("");
    const t0 = performance.now();
    const up = await j("/upload", {method: "POST", body: new Blob([buf])});
    const ms = Math.round(performance.now() - t0);
    rec("upload " + UPMB + " MB (fetch POST)", up.sha256 === want && up.bytes === buf.length, {bytes: up.bytes, ms, mbps: +(buf.length * 8 / ms / 1000).toFixed(1), match: up.sha256 === want});
  } catch (e) { rec("upload " + UPMB + " MB (fetch POST)", false, String(e)); }

  // Long fetch (streamed), idle long fetch, and a WebSocket held open across them.
  const wsP = wsTest("wss://" + P + "/ws", LONG + 3).catch(e => ({error: String(e)}));
  const slowP = (async () => { const t0 = performance.now(); const r = await fetch("/slow?s=" + LONG); const rd = r.body.getReader(); let txt = "";
    for (;;) { const {done, value} = await rd.read(); if (done) break; txt += new TextDecoder().decode(value); }
    return {ticks: (txt.match(/tick/g) || []).length, done: txt.includes("done"), secs: Math.round((performance.now() - t0) / 1000)}; })().catch(e => ({error: String(e)}));
  const holdP = (async () => { const t0 = performance.now(); const r = await fetch("/hold?s=" + LONG); return {status: r.status, body: (await r.text()).trim(), secs: Math.round((performance.now() - t0) / 1000)}; })().catch(e => ({error: String(e)}));
  const [ws, sl, ho] = await Promise.all([wsP, slowP, holdP]);
  rec("WebSocket held " + LONG + "s (echo + server ticks)", !ws.error && ws.echoes === 3 && ws.stillOpen && ws.ticks >= LONG - 2, ws);
  rec("long fetch streamed " + LONG + "s", !!sl.done && sl.ticks === LONG, sl);
  rec("long fetch idle " + LONG + "s", ho.status === 200, ho);

  R.done = Date.now();
  const ok = Object.values(R.checks).every(c => c.ok);
  document.getElementById("state").textContent = (ok ? "ALL OK" : "FAILURES") + " — remote " + R.remote;
  try { await fetch("/log", {method: "POST", body: JSON.stringify(R)}); } catch (e) {}
  try { webkit.messageHandlers.spike.postMessage(JSON.stringify({final: R, ok})); } catch (e) {}
}
if (document.cookie.includes("sid_js=")) main();
</script></body></html>`

const frameHTML = `<!doctype html><html><head><meta charset="utf-8"><title>chat frame</title></head>
<body style="font:12px system-ui;margin:4px">chat frame on <b id="h"></b> <span id="s">…</span>
<script>
const P = "{{P}}";
document.getElementById("h").textContent = location.host;
(async () => {
  const out = {host: location.host, docCookie: document.cookie};
  try { out.echo = await (await fetch("/echo", {credentials: "include"})).json(); } catch (e) { out.echo = {error: String(e), cookies: []}; }
  try { out.platform = await (await fetch("https://" + P + "/whoami", {credentials: "include"})).json(); } catch (e) { out.platform = {error: String(e)}; }
  try {
    out.ws = await new Promise((res, rej) => {
      const ws = new WebSocket("wss://" + location.host + "/ws");
      const to = setTimeout(() => rej(new Error("timeout")), 10000);
      ws.onopen = () => ws.send("frame hello");
      ws.onmessage = e => { if (!e.data.startsWith("{\"tick")) { clearTimeout(to); ws.close(); res({echo: e.data}); } };
      ws.onerror = () => rej(new Error("ws error"));
    });
  } catch (e) { out.ws = {error: String(e)}; }
  document.getElementById("s").textContent = JSON.stringify(out.echo && out.echo.cookies);
  parent.postMessage(out, "https://" + P);
})();
</script></body></html>`
