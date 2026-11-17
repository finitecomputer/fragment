// The slides template's build script (templates/slides/scripts/build.mjs)
// with two decks sharing one toolchain home (#1057 review, round 3), run as
// processes against a fake npm (it "installs" a fake open-slide) and that
// fake's builder (it can be held mid-build). Valid: a deck building while
// another deck installs a new lockfile keeps its toolchain until it is
// done (builds take turns on the home's lock), and only then is the old
// version removed. Invalid: a toolchain removed under a build using it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const template = fileURLToPath(new URL("../../../templates/slides/", import.meta.url));

// npm's stand-in: `npm ci` writes a fake @open-slide/core of the locked
// version, taking NPM_MS to do it, and says so in EVENTS.
const NPM = `
import fs from "node:fs";
import path from "node:path";
const log = (line) => fs.appendFileSync(process.env.EVENTS, line + "\\n");
const lock = JSON.parse(fs.readFileSync("package-lock.json", "utf8"));
log("install " + (lock.name ?? "") + " start");
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, Number(process.env.NPM_MS || 0));
const core = path.join("node_modules", "@open-slide", "core");
fs.mkdirSync(core, { recursive: true });
fs.writeFileSync(path.join(core, "package.json"), JSON.stringify({ version: lock.packages[""].dependencies["@open-slide/core"] }));
fs.copyFileSync(process.env.BUILDER, path.join(core, "bin.js"));
log("install " + (lock.name ?? "") + " end");
`;

// open-slide's builder's stand-in: `build --out-dir <dir>` writes a page;
// while HOLD exists it waits first. It says whether its own toolchain was
// still there once it was let go.
const BUILDER = `
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const log = (line) => fs.appendFileSync(process.env.EVENTS, line + "\\n");
const deck = path.basename(process.cwd());
log("build " + deck + " start");
while (process.env.HOLD && fs.existsSync(process.env.HOLD)) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 20);
const mine = path.dirname(fileURLToPath(import.meta.url));
if (!fs.existsSync(path.join(mine, "package.json"))) { log("build " + deck + " lost its toolchain"); process.exit(1); }
const out = process.argv[process.argv.indexOf("--out-dir") + 1];
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, "index.html"), "<html><head><script></script></head><body></body></html>");
log("build " + deck + " end");
`;

function world() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "slides-build-test-"));
  const bin = path.join(dir, "bin");
  fs.mkdirSync(bin);
  fs.writeFileSync(path.join(dir, "npm.mjs"), NPM);
  fs.writeFileSync(path.join(dir, "builder.mjs"), BUILDER);
  fs.writeFileSync(path.join(bin, "npm"), `#!/bin/sh\nexec "${process.execPath}" "${path.join(dir, "npm.mjs")}" "$@"\n`, { mode: 0o755 });
  const env = { ...process.env, PATH: `${bin}${path.delimiter}${process.env.PATH}`, OPEN_SLIDE_HOME: path.join(dir, "home"), EVENTS: path.join(dir, "events"), BUILDER: path.join(dir, "builder.mjs"), HOLD: path.join(dir, "hold") };
  fs.writeFileSync(env.EVENTS, "");
  // a deck from the template, its lockfile named `lockName` (so another
  // name is another lockfile, with its own hash)
  const deck = (name, lockName = "one") => {
    const at = path.join(dir, name);
    fs.cpSync(template, at, { recursive: true, filter: (src) => !src.startsWith(path.join(template, "site")) && !src.startsWith(path.join(template, "node_modules")) });
    const lock = JSON.parse(fs.readFileSync(path.join(at, "package-lock.json"), "utf8"));
    lock.name = lockName;
    fs.writeFileSync(path.join(at, "package-lock.json"), JSON.stringify(lock));
    return at;
  };
  // a build: `done` its exit code and what it said; `said()` so far
  const build = (at, extra = {}) => {
    const child = spawn(process.execPath, [path.join(at, "scripts", "build.mjs")], { cwd: at, env: { ...env, ...extra }, stdio: ["ignore", "pipe", "pipe"] });
    let said = "";
    child.stdout.on("data", (d) => (said += d));
    child.stderr.on("data", (d) => (said += d));
    const done = new Promise((resolve) => child.on("exit", (code) => resolve({ code, said })));
    return { done, said: () => said };
  };
  const events = () => fs.readFileSync(env.EVENTS, "utf8").split("\n").filter(Boolean);
  const kept = () => fs.readdirSync(env.OPEN_SLIDE_HOME).filter((n) => !n.startsWith("."));
  return { dir, env, deck, build, events, kept };
}

const until = async (what, ms = 20000) => {
  const since = Date.now();
  while (!what()) {
    assert.ok(Date.now() - since < ms, "waited too long");
    await new Promise((r) => setTimeout(r, 20));
  }
};

test("a build keeps its toolchain while another deck installs a new lockfile; the old goes after", { timeout: 15000 }, async (t) => {
  const w = world();
  t.after(() => fs.rmSync(w.dir, { recursive: true, force: true })); // the hold with it: no build waits on
  const [a, b] = [w.deck("a", "one"), w.deck("b", "two")];
  assert.equal((await w.build(a).done).code, 0, "a's toolchain installed");
  const before = w.kept();
  fs.writeFileSync(w.env.HOLD, "");
  const ra = w.build(a);
  // its second build, held inside the builder, with the lock
  await until(() => w.events().filter((e) => e === "build a start").length === 2);
  const rb = w.build(b, { NPM_MS: "200" });
  await until(() => rb.said().includes("waiting for another build"));
  assert.ok(!w.events().includes("install two start"), `b waits for a's build: ${w.events()}`);
  assert.deepEqual(w.kept(), before, "a's toolchain is untouched while a builds");
  fs.rmSync(w.env.HOLD);
  const [da, db] = await Promise.all([ra.done, rb.done]);
  assert.equal(da.code, 0, da.said);
  assert.equal(db.code, 0, db.said);
  const seen = w.events();
  assert.ok(seen.indexOf("build a end") < seen.indexOf("install two start"), `b installed after a's build: ${seen}`);
  assert.ok(!seen.some((e) => e.includes("lost")), `no build lost its toolchain: ${seen}`);
  assert.equal(w.kept().length, 1, "only b's version is kept now");
  assert.notDeepEqual(w.kept(), before);
});
