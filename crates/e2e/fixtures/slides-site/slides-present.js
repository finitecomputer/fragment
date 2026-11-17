// Who presents, from the `present` channel's records (in seq order, as
// every page reads them) and who is here (presence). No DOM and no imports,
// so a test drives it (crates/templates/tests/slides_present.test.mjs).
//
// A page presenting posts `{page, live, from}`, `from` its socket. The
// latest Present wins: the presenter is the page, still here and
// presenting, whose newest record is live and newest of all. A page
// presenting stops when another presenter's record is newer than its own
// start, so whoever pressed Present last presents. Two pressing it at the
// same instant may both stop; either presses it again.
//
// One page's posts reach the channel in the order of its actions (its
// Outbox, below), so a follower never goes back to a page the presenter
// has left.

export class Present {
  constructor() {
    this.sockets = new Map(); // from -> { page, live, seq, principal }
    this.newest = 0; // the newest seq seen
    this.presenting = false;
    this.since = 0; // the newest seq seen when this page started presenting
  }

  // A record of the channel, in seq order.
  record(rec) {
    const b = rec?.body;
    if (!b || typeof b.from !== "string" || !Number.isInteger(rec.seq)) return;
    this.newest = Math.max(this.newest, rec.seq);
    const s = this.sockets.get(b.from);
    if (s && rec.seq <= s.seq) return;
    this.sockets.set(b.from, { page: b.page, live: b.live === true, seq: rec.seq, principal: rec.principal });
  }

  start() {
    this.presenting = true;
    this.since = this.newest;
  }

  stop() {
    this.presenting = false;
  }

  // The presenter: {from, page, principal, seq}, or null. `here` is
  // presence's list ([{id, data}]); `myId` this page's socket, never its own
  // presenter.
  presenter(here, myId) {
    const presentingHere = new Set(here.filter((p) => p?.data?.presenting).map((p) => p.id));
    let best = null;
    for (const [from, s] of this.sockets) {
      if (from === myId || !s.live || !presentingHere.has(from)) continue;
      if (!best || s.seq > best.seq) best = { from, page: s.page, principal: s.principal, seq: s.seq };
    }
    return best;
  }

  // Whether this page, presenting, stops: another presenter's record newer
  // than its start and than its own newest.
  yields(here, myId) {
    if (!this.presenting) return false;
    const other = this.presenter(here, myId);
    return other !== null && other.seq > Math.max(this.since, this.sockets.get(myId)?.seq ?? 0);
  }
}

// A page's posts, one in flight at a time, in the order of its actions.
// `put(state)` with `{page, live}`; `send(state)` posts one and settles when
// the channel has it (or it failed: the next goes anyway). A page change
// waiting is replaced by a newer one; a start or a stop waits its turn and
// is never dropped.
export class Outbox {
  constructor(send) {
    this.send = send;
    this.queue = [];
    this.busy = false;
  }

  put(state) {
    const last = this.queue[this.queue.length - 1];
    if (last && last.live === state.live) this.queue[this.queue.length - 1] = state;
    else this.queue.push(state);
    if (!this.busy) this.drain();
  }

  async drain() {
    this.busy = true;
    while (this.queue.length) {
      const state = this.queue.shift();
      try {
        await this.send(state);
      } catch {
        // its sender says so; the next state still goes
      }
    }
    this.busy = false;
  }
}
