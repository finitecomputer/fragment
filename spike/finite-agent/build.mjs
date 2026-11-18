// Bundles src/agent.js and this folder's pinned dependencies into the
// agent fragment's code: fragment.json and app.mjs as they are, and the
// bundle as applib/agent.mjs (an app's modules: at most 4 MiB). Run with
// the repo's pinned Node: `node build.mjs <out dir>`.
import { build } from "esbuild";
import { cpSync, mkdirSync, statSync } from "node:fs";

const out = process.argv[2] ?? "dist";
mkdirSync(`${out}/applib`, { recursive: true });
await build({
  entryPoints: ["src/agent.js"],
  outfile: `${out}/applib/agent.mjs`,
  bundle: true,
  format: "esm",
  platform: "neutral",
  target: "es2023",
  minify: true,
  mainFields: ["module", "main"],
  conditions: ["workerd", "worker", "import"],
  external: ["cloudflare:workers", "node:*"],
  logLevel: "warning",
  metafile: false,
});
cpSync("fragment.json", `${out}/fragment.json`);
cpSync("app.mjs", `${out}/app.mjs`);
console.log(`${out}/applib/agent.mjs: ${statSync(`${out}/applib/agent.mjs`).size} bytes`);
