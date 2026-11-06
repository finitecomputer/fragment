// Builds the deck into site/: `node scripts/build.mjs` (or `npm run build`)
// from the deck's folder, then `fragment deploy <name> --dir .`.
//
// open-slide (package.json pins it) builds the deck with its own Vite
// build. Its toolchain (about 250 MB) is installed once per pinned version
// into a folder outside the deck, OPEN_SLIDE_HOME or the system's temporary
// folder (npm's cache with it, deleted after), and linked as node_modules; a node_modules the deck already holds
// (`npm install`) is used instead. The build's files then get what a
// fragment's site needs: the deck's page at /, /s/<id>/ and
// /s/<id>/presenter/ (a site has no fallback page), the live layer
// (live/: following a presenter, who is watching, notes), and
// site/build.json, which names the sources it was built from.
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DECK_ID = /^[A-Za-z0-9_-]{1,64}$/;
const LIVE = ["slides-live.js", "slides-live.css", "slides-route.js"];

function fail(message) {
  console.error(`slides: ${message}`);
  process.exit(1);
}

function run(cmd, args, opts) {
  const r = spawnSync(cmd, args, { stdio: "inherit", shell: process.platform === "win32", ...opts });
  if (r.error) fail(`${cmd} did not run: ${r.error.message}`);
  if (r.status !== 0) fail(`${cmd} ${args.join(" ")} failed (exit ${r.status})`);
}

// The sources a build is made from, by path: the deck, its config and its
// pins. The template's own test (crates/templates) hashes the same, so a
// committed site never drifts from its deck.
function sourcesHash(dir) {
  const files = ["package.json", "open-slide.config.ts"];
  const walk = (rel) => {
    for (const entry of fs.readdirSync(path.join(dir, rel), { withFileTypes: true })) {
      const child = `${rel}/${entry.name}`;
      if (entry.isDirectory()) walk(child);
      else if (entry.isFile()) files.push(child);
    }
  };
  walk("slides");
  files.sort();
  const h = createHash("sha256");
  for (const rel of files) {
    h.update(rel);
    h.update("\0");
    h.update(fs.readFileSync(path.join(dir, rel)));
    h.update("\0");
  }
  return h.digest("hex");
}

function theDeck() {
  const dir = path.join(root, "slides");
  const decks = fs.existsSync(dir) ? fs.readdirSync(dir).filter((d) => fs.existsSync(path.join(dir, d, "index.tsx"))) : [];
  if (decks.length !== 1) fail(`a deck fragment holds one deck, slides/<id>/index.tsx; found ${decks.length ? decks.join(", ") : "none"}`);
  if (!DECK_ID.test(decks[0])) fail(`slides/${decks[0]}: a deck's folder is 1 to 64 of A-Z a-z 0-9 _ -`);
  return decks[0];
}

// The toolchain's node_modules: the deck's own, or one installed outside it.
function toolchain(pins) {
  const own = path.join(root, "node_modules");
  const core = (nm) => path.join(nm, "@open-slide", "core", "package.json");
  const version = (nm) => (fs.existsSync(core(nm)) ? JSON.parse(fs.readFileSync(core(nm), "utf8")).version : null);
  const stat = fs.lstatSync(own, { throwIfNoEntry: false });
  if (stat && !stat.isSymbolicLink()) {
    const v = version(own);
    if (v !== pins["@open-slide/core"]) fail(`node_modules holds open-slide ${v ?? "nothing"}, and this deck pins ${pins["@open-slide/core"]}: run npm install, or delete node_modules`);
    return own;
  }
  const key = createHash("sha256").update(JSON.stringify(pins)).digest("hex").slice(0, 12);
  const home = path.join(process.env.OPEN_SLIDE_HOME || path.join(os.tmpdir(), "open-slide"), key);
  const installed = path.join(home, "node_modules");
  if (version(installed) !== pins["@open-slide/core"]) {
    console.log(`slides: installing open-slide ${pins["@open-slide/core"]} into ${home} (once)`);
    const staging = `${home}.${process.pid}`;
    fs.rmSync(staging, { recursive: true, force: true });
    fs.mkdirSync(staging, { recursive: true });
    fs.writeFileSync(path.join(staging, "package.json"), JSON.stringify({ private: true, dependencies: pins }));
    // npm's cache too stays out of the deck's home (a computer's home is on
    // its saved disk): beside the install, and gone once it is done
    const cache = path.join(staging, ".npm-cache");
    run("npm", ["install", "--no-fund", "--no-audit", "--no-package-lock", "--loglevel=error", "--cache", cache], { cwd: staging });
    fs.rmSync(cache, { recursive: true, force: true });
    fs.rmSync(home, { recursive: true, force: true });
    try {
      fs.renameSync(staging, home);
    } catch (e) {
      // another build installed it first
      fs.rmSync(staging, { recursive: true, force: true });
      if (version(installed) !== pins["@open-slide/core"]) fail(`installing into ${home}: ${e.message}`);
    }
  }
  if (stat) fs.unlinkSync(own);
  fs.symlinkSync(installed, own, "junction");
  return installed;
}

function page(html, id) {
  const head = [`<meta name="slides-deck" content="${id}">`, `<script type="module" src="/slides-route.js"></script>`, `<link rel="stylesheet" href="/slides-live.css">`];
  const at = html.indexOf("<script");
  if (at < 0 || !html.includes("</body>")) fail("open-slide's page has no script or no </body>: this build script needs updating");
  // the route goes before the app's own script, so the app starts at the deck
  html = `${html.slice(0, at)}${head.join("\n    ")}\n    ${html.slice(at)}`;
  return html.replace("</body>", `  <script type="module" src="/slides-live.js"></script>\n  </body>`);
}

const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const id = theDeck();
const nm = toolchain(pkg.dependencies);
const out = fs.mkdtempSync(path.join(os.tmpdir(), "slides-build-"));
try {
  run(process.execPath, [path.join(nm, "@open-slide", "core", "bin.js"), "build", "--out-dir", out], { cwd: root, shell: false });
  const html = page(fs.readFileSync(path.join(out, "index.html"), "utf8"), id);
  fs.writeFileSync(path.join(out, "index.html"), html);
  for (const at of [`s/${id}`, `s/${id}/presenter`]) {
    fs.mkdirSync(path.join(out, at), { recursive: true });
    fs.writeFileSync(path.join(out, at, "index.html"), html);
  }
  for (const f of LIVE) fs.copyFileSync(path.join(root, "live", f), path.join(out, f));
  const stamp = { openSlide: pkg.dependencies["@open-slide/core"], deck: id, sources: sourcesHash(root) };
  fs.writeFileSync(path.join(out, "build.json"), `${JSON.stringify(stamp, null, 2)}\n`);
  fs.rmSync(path.join(root, "site"), { recursive: true, force: true });
  fs.cpSync(out, path.join(root, "site"), { recursive: true });
  console.log(`slides: built ${id} into site/. Deploy it: fragment deploy <name> --dir ${path.relative(process.cwd(), root) || "."}`);
} finally {
  fs.rmSync(out, { recursive: true, force: true });
}
