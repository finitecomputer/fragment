// Who presents, as every page decides it alike: from the `present`
// channel's records (in seq order, as every page reads them) and who is
// here (presence). No DOM and no imports, so a test drives it
// (crates/templates/tests/slides_present.test.mjs).
//
// A page presenting posts `{page, live, from}`, `from` its socket. A
// socket's claim is the seq of its first live record since it last posted
// `live: false`. The presenter is the socket with the newest claim whose
// page is still here and presenting (presence) and whose newest record is
// live. Two editors who start at once both post; every page sees both
// claims in one order, so the newer wins everywhere, and the older yields
// once it sees it. A page that has started and not yet heard its own
// claim's seq yields to nothing: it cannot tell yet whether its claim is the
// older.

export class Present {
  constructor() {
    this.sockets = new Map(); // from -> { claim, page, live, seq, principal }
    this.presenting = false;
    this.mine = null; // my claim's seq, once known
    this.pending = false; // started, my claim's seq not yet known
  }

  // A record of the channel, in seq order.
  record(rec, myId) {
    const b = rec?.body;
    if (!b || typeof b.from !== "string" || !Number.isInteger(rec.seq)) return;
    const s = this.sockets.get(b.from);
    if (s && rec.seq <= s.seq) return;
    const claim = b.live ? (s?.live ? s.claim : rec.seq) : null;
    this.sockets.set(b.from, { claim, page: b.page, live: b.live === true, seq: rec.seq, principal: rec.principal });
    if (b.from === myId && b.live && this.presenting) this.claimed(claim);
  }

  start() {
    this.presenting = true;
    this.pending = true;
    this.mine = null;
  }

  // My post's answer: its seq. My claim is my first since starting.
  posted(seq) {
    if (this.presenting && this.pending && Number.isInteger(seq)) this.claimed(seq);
  }

  claimed(seq) {
    if (this.mine === null || seq < this.mine) this.mine = seq;
    this.pending = false;
  }

  stop() {
    this.presenting = false;
    this.pending = false;
    this.mine = null;
  }

  // The presenter: {from, page, principal, claim}, or null. `here` is
  // presence's list ([{id, data}]); `myId` this page's socket, never its own
  // presenter.
  presenter(here, myId) {
    const presentingHere = new Set(here.filter((p) => p?.data?.presenting).map((p) => p.id));
    let best = null;
    for (const [from, s] of this.sockets) {
      if (from === myId || !s.live || s.claim === null || !presentingHere.has(from)) continue;
      if (!best || s.claim > best.claim) best = { from, page: s.page, principal: s.principal, claim: s.claim };
    }
    return best;
  }

  // Whether this page, presenting, gives way: another's newer claim.
  yields(here, myId) {
    if (!this.presenting || this.pending || this.mine === null) return false;
    const other = this.presenter(here, myId);
    return other !== null && other.claim > this.mine;
  }
}
