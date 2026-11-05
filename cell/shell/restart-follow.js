// A restart the person asked for in this page, followed until the computer
// says it is done (docs/api.md: a restart answers `starting`, never awake).
// One small explicit state (FIN-395), every way out of each written below,
// so the page never says "Starting…" of a restart it can no longer see:
//
//   following    its view is read every FOLLOW_MS
//     hidden          -> paused       (no timer; a read under way is dropped)
//     a read fails    -> read_failed  (no timer; said, with Check again)
//     its ask fails   -> one read decides: under way, it goes on; not, failed
//   paused       nothing is read while the page is hidden
//     shown           -> following    (read at once)
//   read_failed  nothing is read: the read's failure is said, never the
//                restart's (a failed read is no news of the restart)
//     Check again     -> following    (read at once)
//     its ask answers -> following    (an answer is a read that worked)
//   failed       its ask failed and the computer shows no restart under way
//     Restart         -> following    (a new restart)
//   any state:   the computer awake on a newer start, or a newer start that
//                won't start, from any read -> done (none); disposed -> none
//
// Its effects come in (`read`, `ask`, `timers`, `hidden`), so its tests
// drive this module itself with a fake clock (cell/shell-tests/).

export const FOLLOW_MS = 2000;
export const FOLLOWING = "following";
export const PAUSED = "paused";
export const READ_FAILED = "read_failed";
export const FAILED = "failed";

// done: awake on a newer start than the one it restarted, or a newer start
// that won't start (its own notice then says so)
export const finished = (c, asked) => !!c && c.generation > asked && (c.phase === "awake" || c.phase === "wont_wake");
// the computer shows this restart going on: a newer start, or its save or
// start under way
export const underway = (c, asked) => !!c && (c.generation > asked || c.phase === "sleeping" || c.phase === "starting");

// `read`: its view, read again; `ask`: the restart's own POST, of the start
// it restarts; `view`: the view the page shows now; `take`: a view read or
// answered, shown; `changed`: the state changed (draw it); `hidden`: the
// page is hidden.
export function createRestartFollow({ read, ask, view, take, changed, hidden, timers = globalThis, now = () => Date.now() }) {
  // { asked, at, state, error, timer, seq }: `seq` drops a read asked
  // before a newer answer, a pause or a disposal
  let op = null;
  function clear(o) {
    if (o.timer !== null) timers.clearTimeout(o.timer);
    o.timer = null;
  }
  function to(o, state, error = null) {
    o.state = state;
    o.error = error;
    changed();
  }
  function schedule(o) {
    clear(o);
    o.timer = timers.setTimeout(() => {
      o.timer = null;
      if (op === o && o.state === FOLLOWING) readNow(o);
    }, FOLLOW_MS);
  }
  // done, by whatever view the page has now; `tell`: draw it (not when the
  // page is drawing already: `check`)
  function done(tell = true) {
    if (!op || !finished(view(), op.asked)) return false;
    clear(op);
    op.seq++;
    op = null;
    if (tell) changed();
    return true;
  }
  // `deciding`: the read after its ask failed, which says whether the
  // restart is under way all the same
  function readNow(o, deciding = false) {
    const seq = ++o.seq;
    read().then(
      (v) => {
        if (op !== o || o.seq !== seq) return;
        take(v);
        // drawing it may have found it done (`check`)
        if (op !== o || done()) return;
        if (deciding && !underway(view(), o.asked)) return to(o, FAILED, o.askFailed);
        if (o.state === FOLLOWING) schedule(o);
      },
      (e) => {
        if (op !== o || o.seq !== seq) return;
        clear(o);
        to(o, READ_FAILED, e?.message ?? String(e));
      },
    );
  }
  return {
    // what is shown: null, or { asked, at, state, error }
    op: () => op && { asked: op.asked, at: op.at, state: op.state, error: op.error },
    // Restart pressed: none runs (or the last one failed), so one begins
    press() {
      const c = view();
      if (!c || (op && op.state !== FAILED)) return false;
      if (op) clear(op);
      const o = { asked: c.generation || 0, at: now(), state: hidden() ? PAUSED : FOLLOWING, error: null, timer: null, seq: 0, askFailed: null };
      op = o;
      changed();
      if (o.state === FOLLOWING) schedule(o);
      ask(o.asked).then(
        (v) => {
          if (op !== o) return;
          // newer than any read under way
          o.seq++;
          take(v);
          if (op !== o || done()) return;
          // an answer is a read that worked
          if (o.state === READ_FAILED) to(o, hidden() ? PAUSED : FOLLOWING);
          if (o.state === FOLLOWING) schedule(o);
        },
        (e) => {
          if (op !== o) return;
          // a failed ask is no proof it did not restart (its answer may be
          // what was lost): one read says
          o.askFailed = e?.message ?? String(e);
          clear(o);
          readNow(o, true);
        },
      );
      return true;
    },
    // a view the page took from elsewhere (its list's read): done by it?
    check: () => done(false),
    // the page hidden or shown again
    visibility() {
      if (!op) return;
      if (hidden() && op.state === FOLLOWING) {
        clear(op);
        op.seq++;
        to(op, PAUSED);
      } else if (!hidden() && op.state === PAUSED) {
        to(op, FOLLOWING);
        readNow(op);
      }
    },
    // Check again, after a read that failed
    readAgain() {
      if (op?.state !== READ_FAILED) return;
      to(op, hidden() ? PAUSED : FOLLOWING);
      if (op.state === FOLLOWING) readNow(op);
    },
    dispose() {
      if (!op) return;
      clear(op);
      op.seq++;
      op = null;
    },
  };
}
