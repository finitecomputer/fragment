// Writes a brain's map.json (README.md, "The map"), from its synced folder
// and the fragment CLI, as whoever runs it (its owner, or their agent):
//
//   fragment sync <brain> --dir ~/brains/<brain>
//   node scripts/snapshot.mjs <brain> --dir ~/brains/<brain> [--agent <agent>]... [--out map.json]
//   fragment write <this map> map.json --from map.json
//
// `--agent` names an agent whose routines (`fragment schedule list`) may
// feed the brain, and whose name labels it among the brain's readers; one
// you cannot read is said on stderr and left out. The CLI is `fragment` on
// PATH, or FRAGMENT_BIN. Node 22 or newer; no packages.
//
// It writes no token, email or person's identity: people are one count.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { assemble, readFolder, readersOf, routinesFor } from "./brain-folder.mjs";

// What the page reads (app.mjs MAP_MAX_BYTES).
const MAP_MAX_BYTES = 512 * 1024;
const USAGE = "usage: node scripts/snapshot.mjs <brain> --dir <its synced folder> [--agent <agent>]... [--out <file>]";
const FRAGMENT = process.env.FRAGMENT_BIN || "fragment";

class Refused extends Error {}

function args(argv) {
  const out = { brain: null, dir: null, agents: [], out: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Refused(`${a} needs a value\n${USAGE}`);
      return argv[++i];
    };
    if (a === "--dir") out.dir = value();
    else if (a === "--agent") out.agents.push(value());
    else if (a === "--out") out.out = value();
    else if (a.startsWith("-") || out.brain) throw new Refused(`unexpected ${a}\n${USAGE}`);
    else out.brain = a;
  }
  if (!out.brain || !out.dir) throw new Refused(USAGE);
  return out;
}

/** One CLI call's `data`, with `--json`: a refusal throws, naming the call and why. */
function cli(argv) {
  const r = spawnSync(FRAGMENT, ["--json", ...argv], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  if (r.error) throw new Refused(`${FRAGMENT} did not run: ${r.error.message}`);
  const line = r.stdout.trim().split("\n").pop() ?? "";
  let answer;
  try {
    answer = JSON.parse(line);
  } catch {
    throw new Refused(`fragment ${argv.join(" ")}: ${r.stderr.trim() || "no answer"}`);
  }
  if (answer.ok !== true) throw new Refused(`fragment ${argv.join(" ")}: ${answer.error?.message ?? "refused"}`);
  return answer.data;
}

/** An agent's name and npub, and its routines naming the brain; null when this signer cannot read them. */
function agentOf(agent) {
  try {
    const status = cli(["status", agent]);
    const schedules = cli(["schedule", "list", agent]).schedules ?? [];
    return { npub: status.npub, title: status.title ?? agent, schedules };
  } catch (e) {
    process.stderr.write(`snapshot: left out ${agent}'s routines: ${e.message}\n`);
    return null;
  }
}

function main() {
  const a = args(process.argv.slice(2));
  const status = cli(["status", a.brain]);
  // a call's answer is its envelope's `result` ({id, replayed, result})
  const index = cli(["call", a.brain, "index_status", "--input", "{}"]).result;
  const members = cli(["members", "list", a.brain]).members ?? [];
  const folder = readFolder(a.dir);
  const agents = a.agents.map((name) => [name, agentOf(name)]).filter(([, v]) => v);
  const ids = folder.wikis.map((w) => w.id);
  const workflows = agents.flatMap(([agent, v]) => routinesFor(v.schedules, { brain: status.name, title: status.title, wikis: ids, agent, agentTitle: v.title }));
  const readers = readersOf(status, members, new Map(agents.map(([, v]) => [v.npub, v.title])));
  const map = assemble({ status, index, folder, workflows, readers, takenAt: new Date().toISOString() });
  const json = `${JSON.stringify(map, null, 2)}\n`;
  if (Buffer.byteLength(json) > MAP_MAX_BYTES) throw new Refused(`the map is ${Buffer.byteLength(json)} bytes; the page reads at most ${MAP_MAX_BYTES}`);
  if (a.out) fs.writeFileSync(a.out, json);
  else process.stdout.write(json);
  const n = (k) => map[k].length;
  process.stderr.write(`snapshot: ${map.brain.name}: ${n("wikis")} wikis, ${n("sources")} sources, ${n("workflows")} workflows, ${n("readers")} readers\n`);
}

try {
  main();
} catch (e) {
  process.stderr.write(`snapshot: ${e instanceof Refused ? e.message : e.stack}\n`);
  process.exit(1);
}
