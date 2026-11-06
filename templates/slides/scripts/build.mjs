// Builds the deck into site/: `node scripts/build.mjs` (or `npm run build`)
// from the deck's folder, then `fragment deploy <name> --dir .`.
// `node scripts/build.mjs --check` builds it again aside and says whether
// site/ is that build, byte for byte, changing nothing.
//
// open-slide (package.json pins it, package-lock.json locks all it needs)
// builds the deck with its own Vite build. Its toolchain (about 250 MB) is
// installed once per lockfile (`npm ci`) into a folder outside the deck and
// linked as node_modules; a node_modules the deck already holds (`npm ci`)
// is used instead. That folder is OPEN_SLIDE_HOME, else /tmp/open-slide:
// named, never the temporary folder TMPDIR names, which on a computer is
// the agent's work and saved with it (images/hermes/boot/src/hermes.rs
// `tmp_dir`), while the container's /tmp is in no save (docs/computers.md).
// A folder under /data, the computer's saved disk, is refused. npm's cache,
// the install's staging and the build's own output are made there too, and
// removed on every way out, a failure's included. The build's files then get
// what a fragment's site needs: the deck's page at /, /s/<id>/ and
// /s/<id>/presenter/ (a site has no fallback page), the live layer (live/:
// following a presenter, who is watching, notes), and site/build.json,
// which names the hash of every input the build read and of what it made.
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DECK_ID = /^[A-Za-z0-9_-]{1,64}$/;
const LIVE = ["slides-live.css", "slides-live.js", "slides-present.js", "slides-route.js"];
// Every input of a build, files and folders (whole). The template's own
// test (crates/templates) hashes the same, as `inputsHash` does.
const INPUTS = ["live", "open-slide.config.ts", "package-lock.json", "package.json", "scripts", "slides"];

// A refusal: thrown, so every folder this made is removed on the way out.
class Refused extends Error {}
function fail(message) {
  throw new Refused(message);
}

function run(cmd, args, opts) {
  const r = spawnSync(cmd, args, { stdio: "inherit", shell: process.platform === "win32", ...opts });
  if (r.error) fail(`${cmd} did not run: ${r.error.message}`);
  if (r.status !== 0) fail(`${cmd} ${args.join(" ")} failed (exit ${r.status})`);
}

// Where the toolchain, its staging and the build's output go: never under
// /data, however the path gets there (a link is followed).
function installRoot() {
  const root = path.resolve(process.env.OPEN_SLIDE_HOME || (process.platform === "win32" ? path.join(os.tmpdir(), "open-slide") : "/tmp/open-slide"));
  // where it really is, before anything is made there: its nearest folder
  // that exists, links followed, and the rest of its path
  let at = root;
  while (!fs.existsSync(at)) at = path.dirname(at);
  const real = path.join(fs.realpathSync(at), path.relative(at, root));
  if (real === "/data" || real.startsWith("/data/")) fail(`${root} is on /data, the computer's saved disk: name another with OPEN_SLIDE_HOME`);
  fs.mkdirSync(real, { recursive: true });
  return real;
}

// Folders to remove on the way out, whatever the way.
const scratch = new Set();

// Each file under `dir` at the paths `names` (a file, or a folder whole),
// by path, `/`-separated, sorted.
function filesOf(dir, names) {
  const files = [];
  const walk = (rel) => {
    const stat = fs.statSync(path.join(dir, rel), { throwIfNoEntry: false });
    if (stat?.isDirectory()) for (const entry of fs.readdirSync(path.join(dir, rel))) walk(`${rel}/${entry}`);
    else if (stat?.isFile()) files.push(rel);
  };
  for (const name of names) walk(name);
  return files.sort();
}

// A hash of files by path and bytes: `path \0 bytes \0`, each in path order.
function hashOf(dir, files) {
  const h = createHash("sha256");
  for (const rel of files) {
    h.update(rel);
    h.update("\0");
    h.update(fs.readFileSync(path.join(dir, rel)));
    h.update("\0");
  }
  return h.digest("hex");
}

const inputsHash = (dir) => hashOf(dir, filesOf(dir, INPUTS));
// What a build made: every file of it but build.json, which names this.
const outputHash = (dir) => hashOf(dir, filesOf(dir, fs.readdirSync(dir)).filter((f) => f !== "build.json"));

function theDeck() {
  const dir = path.join(root, "slides");
  const decks = fs.existsSync(dir) ? fs.readdirSync(dir).filter((d) => fs.existsSync(path.join(dir, d, "index.tsx"))) : [];
  if (decks.length !== 1) fail(`a deck fragment holds one deck, slides/<id>/index.tsx; found ${decks.length ? decks.join(", ") : "none"}`);
  if (!DECK_ID.test(decks[0])) fail(`slides/${decks[0]}: a deck's folder is 1 to 64 of A-Z a-z 0-9 _ -`);
  return decks[0];
}

// The toolchain's node_modules: the deck's own, or one installed outside it.
function toolchain(pins, base) {
  const own = path.join(root, "node_modules");
  const core = (nm) => path.join(nm, "@open-slide", "core", "package.json");
  const version = (nm) => (fs.existsSync(core(nm)) ? JSON.parse(fs.readFileSync(core(nm), "utf8")).version : null);
  const stat = fs.lstatSync(own, { throwIfNoEntry: false });
  if (stat && !stat.isSymbolicLink()) {
    const v = version(own);
    if (v !== pins["@open-slide/core"]) fail(`node_modules holds open-slide ${v ?? "nothing"}, and this deck pins ${pins["@open-slide/core"]}: run npm ci, or delete node_modules`);
    return own;
  }
  const lock = fs.readFileSync(path.join(root, "package-lock.json"));
  const key = createHash("sha256").update(fs.readFileSync(path.join(root, "package.json"))).update(lock).digest("hex").slice(0, 12);
  const home = path.join(base, key);
  const installed = path.join(home, "node_modules");
  if (version(installed) !== pins["@open-slide/core"]) {
    console.log(`slides: installing open-slide ${pins["@open-slide/core"]} into ${home} (once)`);
    const staging = `${home}.${process.pid}`;
    scratch.add(staging);
    fs.rmSync(staging, { recursive: true, force: true });
    fs.mkdirSync(staging, { recursive: true });
    for (const f of ["package.json", "package-lock.json"]) fs.copyFileSync(path.join(root, f), path.join(staging, f));
    // npm's cache too stays out of the deck's home (a computer's home is on
    // its saved disk): beside the install, and gone once it is done
    const cache = path.join(staging, ".npm-cache");
    run("npm", ["ci", "--no-fund", "--no-audit", "--ignore-scripts", "--loglevel=error", "--update-notifier=false", "--cache", cache], { cwd: staging });
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

// The whole site, built into `out`.
function build(out, pkg, id, base) {
  const nm = toolchain(pkg.dependencies, base);
  run(process.execPath, [path.join(nm, "@open-slide", "core", "bin.js"), "build", "--out-dir", out], { cwd: root, shell: false });
  const html = page(fs.readFileSync(path.join(out, "index.html"), "utf8"), id);
  fs.writeFileSync(path.join(out, "index.html"), html);
  for (const at of [`s/${id}`, `s/${id}/presenter`]) {
    fs.mkdirSync(path.join(out, at), { recursive: true });
    fs.writeFileSync(path.join(out, at, "index.html"), html);
  }
  for (const f of LIVE) fs.copyFileSync(path.join(root, "live", f), path.join(out, f));
  const stamp = { openSlide: pkg.dependencies["@open-slide/core"], deck: id, inputs: inputsHash(root), output: outputHash(out) };
  fs.writeFileSync(path.join(out, "build.json"), `${JSON.stringify(stamp, null, 2)}\n`);
}

const check = process.argv.includes("--check");
try {
  const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
  const id = theDeck();
  const base = installRoot();
  const out = fs.mkdtempSync(path.join(base, "build-"));
  scratch.add(out);
  build(out, pkg, id, base);
  const site = path.join(root, "site");
  if (check) {
    const made = filesOf(out, fs.readdirSync(out));
    const held = fs.existsSync(site) ? filesOf(site, fs.readdirSync(site)) : [];
    const differ = [...new Set([...made, ...held])].sort().filter((f) => !made.includes(f) || !held.includes(f) || !fs.readFileSync(path.join(out, f)).equals(fs.readFileSync(path.join(site, f))));
    if (differ.length) fail(`site/ is not this deck's build; these differ: ${differ.join(", ")}`);
    console.log(`slides: site/ is this deck's build, byte for byte (${made.length} files)`);
  } else {
    fs.rmSync(site, { recursive: true, force: true });
    fs.cpSync(out, site, { recursive: true });
    console.log(`slides: built ${id} into site/. Deploy it: fragment deploy <name> --dir ${path.relative(process.cwd(), root) || "."}`);
  }
} catch (e) {
  if (!(e instanceof Refused)) throw e;
  console.error(`slides: ${e.message}`);
  process.exitCode = 1;
} finally {
  for (const dir of scratch) fs.rmSync(dir, { recursive: true, force: true });
}
