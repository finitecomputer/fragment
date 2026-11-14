// A brain map: where one brain's knowledge comes from and who reads it,
// drawn with React Flow (site/). Its data is one file, map.json at main
// (README.md, "The map"), a snapshot an agent or scripts/snapshot.mjs
// writes: an app reads only its own files, never a brain's, so the map is
// made by someone who can read the brain, as a parent brain is.
//
//   map      (query)    map.json as main holds it, checked, and its rev
//   touched  (mutation) map.json moved (its file trigger): every open
//                       page's live `map` answers again
//
// A mutation cannot read files, so `touched` only counts: its commit is
// what makes open pages ask `map` again. A snapshot is written now and
// then, well inside the platform's triggered runs an hour.
import { DurableObject } from "cloudflare:workers";

export const MAP_PATH = "map.json";
// The map every viewer's browser reads whole: a brain's sources, workflows
// and readers at their lists' bounds (site/graph.js LIST_MAX) fit with room.
export const MAP_MAX_BYTES = 512 * 1024;
export const VERSION = 1;

/** Why `map` is not a map this page draws, or null when it is. */
export function problemOf(map) {
  if (map === null || typeof map !== "object" || Array.isArray(map)) return "map.json is not an object";
  if (map.version !== VERSION) return `map.json is version ${JSON.stringify(map.version ?? null)}; this page draws version ${VERSION}`;
  if (map.brain === null || typeof map.brain !== "object") return "map.json names no brain";
  for (const key of ["wikis", "sources", "workflows", "readers"]) {
    if (map[key] !== undefined && !Array.isArray(map[key])) return `map.json's ${key} is not a list`;
  }
  return null;
}

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS seen (id INTEGER PRIMARY KEY CHECK (id = 1), rev INTEGER NOT NULL, commit_sha TEXT)");
  }

  #rev() {
    return this.ctx.storage.sql.exec("SELECT rev FROM seen WHERE id = 1").toArray()[0]?.rev ?? 0;
  }

  async map() {
    const rev = this.#rev();
    const stat = await this.files.stat(MAP_PATH);
    if (!stat) return { rev, map: null, problem: "This fragment holds no map.json yet." };
    if (stat.size > MAP_MAX_BYTES) return { rev, map: null, problem: `map.json is ${stat.size} bytes; the most this page reads is ${MAP_MAX_BYTES}.` };
    const text = await this.files.read(MAP_PATH);
    if (text === null) return { rev, map: null, problem: "map.json went away while it was read." };
    let map;
    try {
      map = JSON.parse(text);
    } catch (e) {
      return { rev, map: null, problem: `map.json is not JSON: ${e.message}` };
    }
    const problem = problemOf(map);
    return problem ? { rev, map: null, problem } : { rev, map };
  }

  touched(input) {
    const commit = typeof input?.commit === "string" ? input.commit.slice(0, 64) : null;
    this.ctx.storage.sql.exec(
      "INSERT INTO seen (id, rev, commit_sha) VALUES (1, 1, ?) ON CONFLICT (id) DO UPDATE SET rev = rev + 1, commit_sha = excluded.commit_sha",
      commit,
    );
    return { rev: this.#rev() };
  }
}
