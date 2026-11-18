// The Finite Agent spike: pi-durable (pi's durable agent harness) as an
// agent fragment's own code. Never merged; see ../README.md.
//
// One live operation, `drive`, does everything:
// - `{follow: <chat>}` (its owner's call): follow the chat's `chat` channel
//   as the agent, so each record there runs `drive` (a `followed` trigger);
// - `{fragment, channel, record}` (a followed record): a message for the
//   agent becomes a turn: pi's input in the chat's own pi conversation,
//   keyed by the turn's id, so a call made again (its call was cut) admits
//   nothing twice and waits on the same answer. While it waits, the
//   answer's text streams into the chat's draft; then the reply and the
//   turn's end, all as the bridge writes them (docs/chat-records.md).
//
// pi owns the transcript, the inbox and the run, in this fragment's own
// SQLite (`pi_` tables). The model is the platform's model route, through
// the MODELS capability; the tools run on the agent's computer, through
// COMPUTER, which wakes it only when a tool is called.

import { DurableObject } from "cloudflare:workers";
import { AssistantEntry, Harness, createRegistry, defineExtension, defineTool, section } from "@earendil-works/pi-durable";
import { SqliteStorage } from "@earendil-works/pi-durable/storage/sqlite";
import { DurableObjectSqliteDatabase } from "@earendil-works/chord/sqlite/cloudflare";
import { BACKGROUND_CONTEXT as BG } from "@earendil-works/chord/context";
import { createModels, createProvider } from "@earendil-works/pi-ai";
import { stream, streamSimple } from "@earendil-works/pi-ai/api/openai-completions";
import { obj, opt, str } from "@earendil-works/pi-schema";

// The model route's tier, as pi names its model.
const MODEL = { provider: "fragment", modelId: "cheap" };
// A turn's call holds at most this long, then asks to be called again,
// under the Workflow step's 15 minutes.
const TURN_BUDGET_MS = 12 * 60_000;
// A draft at most this often (the chat's own rate).
const DRAFT_EVERY_MS = 250;
// What a tool's output keeps for the model.
const OUTPUT_MAX = 16 * 1024;
const EXEC_MS = 60_000;
const WORK = "/data/work/finite-agent";

const PREAMBLE = [
  "You are an agent in a chat. Answer briefly and plainly.",
  "You have a computer: `run` runs a shell command there, `file` reads or writes a file there.",
  "Use them only when the question needs a command or a file; most questions need neither.",
].join("\n");

async function hex24(text) {
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
  return [...d].map((b) => b.toString(16).padStart(2, "0")).join("").slice(0, 24);
}

function textOf(message) {
  if (!message || message.role !== "assistant") return "";
  return message.content.map((p) => (p.type === "text" ? p.text : "")).join("");
}

function tail(text, max) {
  return text.length <= max ? text : `…${text.slice(-max)}`;
}

export class App extends DurableObject {
  #pi = null;
  // conversation id -> the turns admitted there, oldest first (the one pi runs is the first unsettled)
  #turns = new Map();
  #chats = new Map();

  // ---- the capabilities ----

  async #reach(name, method, path, body) {
    const r = await this.env.FRAGMENTS.request(name, method, path, body);
    const text = await r.text();
    if (!r.ok) throw new Error(`${method} ${name}${path}: ${r.status} ${text.slice(0, 300)}`);
    return text ? JSON.parse(text) : null;
  }

  // a command on the agent's computer: its output and exit, and when its first byte came
  async #exec(argv, stdin, t0) {
    const r = await this.env.COMPUTER.exec({ argv, stdin: stdin ?? null, ms: EXEC_MS });
    if (!r.ok) throw new Error(`exec: ${r.status} ${(await r.text()).slice(0, 300)}`);
    let out = "";
    let exit = null;
    let firstByteMs = null;
    let rest = "";
    const decoder = new TextDecoder();
    for await (const chunk of r.body) {
      if (firstByteMs === null) firstByteMs = Date.now() - t0;
      rest += decoder.decode(chunk, { stream: true });
      let nl;
      while ((nl = rest.indexOf("\n")) >= 0) {
        const line = JSON.parse(rest.slice(0, nl));
        rest = rest.slice(nl + 1);
        if (line.out !== undefined) out = tail(out + line.out, OUTPUT_MAX);
        if (line.exit !== undefined) exit = line.exit;
        if (line.error !== undefined) throw new Error(line.error);
      }
    }
    return { out, exit, firstByteMs };
  }

  // ---- pi ----

  async #open() {
    if (this.#pi) return this.#pi;
    this.#pi = (async () => {
      const t0 = Date.now();
      const database = new DurableObjectSqliteDatabase(this.ctx.storage);
      const storage = await SqliteStorage.open(database, { prefix: "pi_" });
      await database.exec("CREATE TABLE IF NOT EXISTS fa_chats (chat TEXT PRIMARY KEY, conversation INTEGER NOT NULL)");
      // the turns this agent ended: a record run again finds its turn here
      await database.exec("CREATE TABLE IF NOT EXISTS fa_ended (turn TEXT PRIMARY KEY, at INTEGER NOT NULL)");
      const modelFetch = async (_url, init) => this.env.MODELS.chat(typeof init.body === "string" ? init.body : await new Response(init.body).text());
      const provider = createProvider({
        id: "fragment",
        auth: { apiKey: { name: "Fragment", resolve: async () => ({ auth: { apiKey: "fragment" } }) } },
        models: [
          {
            id: "cheap",
            name: "Fragment cheap",
            api: "openai-completions",
            provider: "fragment",
            baseUrl: "https://models.fragment.internal/v1",
            reasoning: false,
            input: ["text"],
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
            contextWindow: 128_000,
            maxTokens: 4096,
            compat: { supportsStore: false, supportsDeveloperRole: false, supportsReasoningEffort: false, maxTokensField: "max_tokens", supportsUsageInStreaming: true },
          },
        ],
        api: {
          stream: (model, context, options) => stream(model, context, { ...options, fetch: modelFetch }),
          streamSimple: (model, context, options) => streamSimple(model, context, { ...options, fetch: modelFetch }),
        },
      });
      const models = createModels();
      models.setProvider(provider);
      const registry = createRegistry();
      registry.install(defineExtension({ name: "finite", sections: [section("preamble", () => PREAMBLE, { tag: false })], tools: this.#tools() }));
      const pi = await Harness.open(storage, { models, registry, settings: { retry: { maxRetries: 1 } } }, BG);
      pi.resume();
      return { pi, database, openedMs: Date.now() - t0 };
    })();
    this.#pi.catch(() => (this.#pi = null));
    return this.#pi;
  }

  async #conversation(chat) {
    const { pi, database } = await this.#open();
    if (!this.#chats.has(chat)) {
      this.#chats.set(chat, (async () => {
        const row = await database.get("SELECT conversation FROM fa_chats WHERE chat = ?", chat);
        if (row) return (await pi.conversation(row.conversation, BG)) ?? Promise.reject(new Error(`conversation ${row.conversation} is gone`));
        const conversation = await pi.createConversation({ ownership: { kind: "ownerless" }, agent: { model: MODEL } }, BG);
        await database.run("INSERT OR IGNORE INTO fa_chats (chat, conversation) VALUES (?, ?)", chat, conversation.id);
        return conversation;
      })());
    }
    return this.#chats.get(chat);
  }

  // the turn pi runs in a conversation now: its first unsettled
  #current(conversationId) {
    return (this.#turns.get(conversationId) ?? []).find((t) => !t.settled);
  }

  #tools() {
    const recorded = (name, category, preview, run) => async (args, api) => {
      const turn = this.#current(api.conversationId);
      const t0 = Date.now();
      const step = turn ? ++turn.steps : 0;
      const shown = preview(args).slice(0, 140);
      const response = turn ? `rs:${turn.id}:${turn.steps}` : "";
      if (turn) {
        const call = await hex24(`${response}\0${0}`);
        await this.#reach(turn.chat, "POST", "/api/channels/work", { id: `wk:${turn.id}:call:${call}`, body: { kind: "turn.call", turn: turn.id, response, tool_call_id: api.callId, tool: name, category, args: shown } }).catch(() => {});
      }
      let result;
      let ok = true;
      let excerpt = "";
      try {
        result = await run(args, t0, turn);
        excerpt = result.excerpt ?? "";
        ok = result.ok;
      } catch (e) {
        ok = false;
        result = { text: String(e?.message ?? e) };
      }
      if (turn) {
        turn.tools.push({ tool: name, ms: Date.now() - t0, firstByteMs: result.firstByteMs ?? null });
        await this.#reach(turn.chat, "POST", "/api/channels/work", { id: `wk:${turn.id}:${step}`, body: { kind: "turn.step", turn: turn.id, step, tool: name, category, args: shown, ok, excerpt: excerpt.slice(0, 300), tool_call_id: api.callId, status: ok ? "completed" : "failed", duration_ms: Date.now() - t0, response } }).catch(() => {});
      }
      return { output: [{ type: "text", text: result.text || "(no output)" }], isError: !ok };
    };
    const run = defineTool({
      name: "run",
      description: "Run a shell command on your computer (sh -c), in your work folder. Answers its output and exit code.",
      parameters: obj({ command: str({ minLength: 1 }) }),
      execute: recorded("run", "shell", (a) => a.command, async ({ command }, t0) => {
        const { out, exit, firstByteMs } = await this.#exec(["sh", "-c", `mkdir -p ${WORK} && cd ${WORK} && ${command}`], null, t0);
        return { ok: exit === 0, text: `${out}\n(exit ${exit})`, excerpt: `Exit code: ${exit}`, firstByteMs };
      }),
    });
    const file = defineTool({
      name: "file",
      description: "Read a file on your computer, or write one whole (give `text`). Paths are relative to your work folder.",
      parameters: obj({ path: str({ minLength: 1 }), text: opt(str()) }),
      replay: "safe",
      execute: recorded("file", "read", (a) => `${a.text === undefined ? "read" : "write"} ${a.path}`, async ({ path, text }, t0) => {
        const script = text === undefined ? 'cd "$0" && cat -- "$1"' : 'cd "$0" && mkdir -p -- "$(dirname -- "$1")" && cat > "$1"';
        const { out, exit, firstByteMs } = await this.#exec(["sh", "-c", `mkdir -p ${WORK} && ${script}`, WORK, path], text ?? null, t0);
        return { ok: exit === 0, text: text === undefined ? out : exit === 0 ? `wrote ${path}` : out, excerpt: "", firstByteMs };
      }),
    });
    return [run, file];
  }

  // ---- the operation ----

  async drive(input, live) {
    if (input.follow) {
      const r = await this.#reach(input.follow, "POST", "/api/subscriptions", { channel: "chat", follow: live.fragment });
      return { following: r };
    }
    if (input.record) return await this.#turn(input, live);
    return { ignored: true };
  }

  async #turn({ fragment: chat, channel, record }, live) {
    const received = Date.now();
    const me = live.principal;
    const body = typeof record.body === "string" ? { text: record.body } : record.body ?? {};
    const forMe = !Array.isArray(body.to) || body.to.includes(me);
    if (channel !== "chat" || record.principal === me || typeof body.text !== "string" || !forMe || (body.kind && body.kind !== "message") || record.principal?.startsWith("anon:")) {
      return { ignored: record.seq };
    }
    const id = await hex24(`${live.fragment}|${chat}/${channel}/${record.seq}`);
    const t = { opened: 0, submitted: 0, firstDraft: null, answered: 0 };
    const { openedMs, database } = await this.#open();
    if (await database.get("SELECT turn FROM fa_ended WHERE turn = ?", id)) return { turn: id, replayed: true };
    const conversation = await this.#conversation(chat);
    t.opened = Date.now() - received;
    const turn = { id, chat, steps: 0, tools: [], settled: false };
    const list = this.#turns.get(conversation.id) ?? [];
    if (!list.some((x) => x.id === id)) list.push(turn);
    this.#turns.set(conversation.id, list);
    const work = (part, b) => this.#reach(chat, "POST", "/api/channels/work", { id: `wk:${id}:${part}`, body: b });
    await work("start", { kind: "turn.start", turn: id, asker: record.principal, agent: me, cause: { fragment: chat, channel, seq: record.seq }, life: `${await hex24(live.fragment)}00000000` });
    const submission = await conversation.submit({ type: "input", content: body.text, requestId: id }, BG);
    t.submitted = Date.now() - received;
    // the answer's text as it streams, into the chat's draft
    let lastDraft = 0;
    let drafting = Promise.resolve();
    const watch = await conversation.watch(BG);
    watch.start(async (call) => {
      if (call.kind !== "value" || this.#current(conversation.id)?.id !== id) return;
      const text = textOf(call.value.docs?.["pi.live"]?.generation?.message);
      if (!text || Date.now() - lastDraft < DRAFT_EVERY_MS) return;
      lastDraft = Date.now();
      if (t.firstDraft === null) t.firstDraft = Date.now() - received;
      drafting = drafting.then(() => this.#reach(chat, "PUT", "/api/channels/chat/draft", { turn: id, text }).catch(() => {}));
    });
    let settled;
    try {
      settled = await Promise.race([submission.wait(BG), new Promise((r) => setTimeout(() => r(null), TURN_BUDGET_MS))]);
    } finally {
      await watch.stop().catch(() => {});
    }
    if (settled === null) {
      live.again();
      return { turn: id, again: true };
    }
    turn.settled = true;
    await drafting;
    t.answered = Date.now() - received;
    let text = "";
    let outcome = "idle";
    let error;
    if (settled.status === "done" && settled.answer !== undefined) {
      const entry = await conversation.commit((tx) => tx.entry(AssistantEntry, settled.answer), BG);
      text = textOf(entry?.model?.[0]);
    } else {
      outcome = "error";
      error = `the runtime gave no answer (${settled.reason ?? "unanswered"})`;
    }
    if (text) await this.#reach(chat, "POST", "/api/channels/chat", { id: `rp:${id}:1`, body: { text, turn: id } });
    const timing = { received_at: received, opened_ms: openedMs, open_ms: t.opened, submitted_ms: t.submitted, first_draft_ms: t.firstDraft, answered_ms: t.answered, tools: turn.tools };
    await work("end", { kind: "turn.end", turn: id, outcome, ...(error ? { error } : {}), timing });
    await database.run("INSERT OR IGNORE INTO fa_ended (turn, at) VALUES (?, ?)", id, Date.now());
    return { turn: id, timing };
  }
}
