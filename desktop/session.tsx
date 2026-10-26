import { signal, WebView, type YNode } from "nsh";
import type { Computer, Invite, Person, SearchResults, Workspace } from "./types";

export class ApiError extends Error {
  status: number;
  code?: string;
  reason?: string;

  constructor(status: number, code?: string, message?: string, reason?: string) {
    super(message || `Request failed (${status})`);
    this.status = status;
    this.code = code;
    this.reason = reason;
  }
}

type Pending = { resolve: (value: any) => void; reject: (error: Error) => void };

const BRIDGE = String.raw`
(() => {
  if (window.__finiteDesktop) return;
  window.__finiteDesktop = true;
  let watch;
  const send = (message) => window.nsh.postMessage(JSON.stringify(message));
  window.addEventListener("message", async (event) => {
    const request = typeof event.data === "string" ? JSON.parse(event.data) : event.data;
    if (!request || request.channel !== "finite-desktop") return;
    if (request.action === "watch") {
      if (watch) watch.close();
      const scheme = location.protocol === "https:" ? "wss" : "ws";
      watch = new WebSocket(scheme + "://" + location.host + "/api/fragments/watch");
      watch.onopen = () => send({ type: "watch", state: "open" });
      watch.onmessage = (message) => send({ type: "watch", state: "message", data: JSON.parse(message.data) });
      watch.onclose = () => send({ type: "watch", state: "closed" });
      return;
    }
    if (request.action !== "api") return;
    try {
      const headers = { "x-fragment-shell": "1", ...(request.body === undefined ? {} : { "content-type": "application/json" }) };
      const response = await fetch(request.path, {
        method: request.method,
        headers,
        body: request.body === undefined ? undefined : JSON.stringify(request.body),
        credentials: "same-origin",
      });
      const text = await response.text();
      let body = {};
      try { body = text ? JSON.parse(text) : {}; } catch { body = { message: text }; }
      send({ type: "api", id: request.id, status: response.status, ok: response.ok, body });
    } catch (error) {
      send({ type: "api", id: request.id, status: 0, ok: false, body: { message: String(error) } });
    }
  });
})();
`;

export class Session {
  host: string;
  node?: YNode;
  ready = signal(false);
  watching = signal(false);
  private next = 1;
  private pending = new Map<number, Pending>();
  private watchTimer?: ReturnType<typeof setTimeout>;
  onChange?: () => void;
  onInvites?: () => void;
  onFragment?: (name: string, data: any, origin: string) => void;

  constructor(host: string) {
    this.host = host.replace(/\/$/, "");
  }

  Bridge = () => (
    <WebView
      ref={(node: YNode) => { this.node = node; }}
      src={this.host}
      injectedJavaScriptBeforeContentLoaded={BRIDGE}
      style={{ width: 2, height: 2, opacity: 0.01, position: "absolute", left: 0, top: 0 }}
      onLoad={() => {
        this.ready.set(true);
        this.watch();
      }}
      onMessage={(event) => this.receive(JSON.parse(event.data))}
      onError={(event) => {
        console.error(`Session bridge: ${event.message}`);
        this.ready.set(false);
      }}
    />
  );

  url(path: string): string {
    return `${this.host}${path}`;
  }

  loginUrl(): string {
    return this.url(`/auth/login?return=${encodeURIComponent("/")}`);
  }

  fragmentUrl(name: string, path = "/", frame = false): string {
    const route = frame ? "frame" : "fragment";
    return this.url(`/auth/${route}?name=${encodeURIComponent(name)}&return=${encodeURIComponent(path)}`);
  }

  api<T = any>(method: string, path: string, body?: unknown): Promise<T> {
    if (!this.node) return Promise.reject(new Error("The session is still connecting."));
    const id = this.next++;
    const promise = new Promise<T>((resolve, reject) => this.pending.set(id, { resolve, reject }));
    this.node.postMessage(JSON.stringify({ channel: "finite-desktop", action: "api", id, method, path, body }));
    return promise;
  }

  async workspace(): Promise<Workspace> {
    const [me, listed, computers, seat, invited] = await Promise.all([
      this.api<Person>("GET", "/api/identities/me"),
      this.api<{ fragments: any[] }>("GET", "/api/fragments"),
      this.api<{ computers: Computer[] }>("GET", "/api/computers").catch(() => ({ computers: [] })),
      this.api<{ guest: boolean }>("GET", "/api/seat").catch(() => ({ guest: false })),
      this.api<{ invites: Invite[] }>("GET", "/api/invites").catch(() => ({ invites: [] })),
    ]);
    return {
      me,
      guest: seat.guest,
      fragments: listed.fragments ?? [],
      computer: computers.computers?.[0] ?? null,
      invites: invited.invites ?? [],
    };
  }

  search(prompt: string, hint: string): Promise<SearchResults> {
    return this.api("GET", `/api/search?q=${encodeURIComponent(hint)}`);
  }

  watch() {
    if (!this.ready() || !this.node) return;
    this.node.postMessage(JSON.stringify({ channel: "finite-desktop", action: "watch" }));
  }

  private receive(message: any) {
    if (message.type === "api") {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.ok) pending.resolve(message.body);
      else pending.reject(new ApiError(message.status, message.body.error, message.body.message, message.body.reason));
      return;
    }
    if (message.type === "fragment") {
      this.onFragment?.(message.name, message.data, message.origin);
      return;
    }
    if (message.type === "watch") {
      this.watching.set(message.state === "open");
      if (message.state === "message") {
        if (message.data?.type === "changed") this.onChange?.();
        if (message.data?.type === "invited") this.onInvites?.();
      }
      if (message.state === "closed") {
        clearTimeout(this.watchTimer);
        this.watchTimer = setTimeout(() => this.watch(), 2000);
      }
    }
  }

  dispose() {
    clearTimeout(this.watchTimer);
    for (const pending of this.pending.values()) pending.reject(new Error("Session closed"));
    this.pending.clear();
  }
}
