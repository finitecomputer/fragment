// A restart the person asked for in this page, followed until the computer
// says it is done (docs/api.md: a restart answers `starting` once under
// way, never awake). One explicit machine (FIN-395): `step` takes a state
// and an event to the next state and the effects it asks for, and nothing
// else decides; the runner below (`createRestartFollow`) only performs
// those effects and turns their outcomes into events.
//
// A state is none (no restart followed), `failed`, or one of the reading
// states, each of which also says whether the restart's own ask failed
// and is not yet decided (`refused`: its words). Every pairing is one the
// page can be in; `failed` carries neither, so a refused ask always waits
// for a read that decides it, whatever happens between:
//
//   following    its view is read every FOLLOW_MS
//     hidden           -> paused       (no timer; a read under way dropped)
//     a read fails     -> read_failed  (no timer; said, with Check again)
//     its ask fails    -> following, refused, read at once
//   paused       nothing is read while the page is hidden
//     shown            -> following    (read at once)
//     its ask fails    -> paused, refused (decided once shown)
//   read_failed  nothing is read: the read's failure is said, never the
//                restart's (a failed read is no news of it), until Check again
//     Check again      -> following, or paused if hidden (read at once)
//     its ask answers  -> following, or paused (an answer is a read that worked)
//     its ask fails    -> read_failed, refused (decided after Check again)
//   any read that works, refused: under way (its answer was what was lost)
//                    -> refused forgotten; not -> failed, with its words
//   failed       the ask failed and a read showed no restart under way
//     Restart          -> a new restart
//   any state:   a view awake on a newer start, or a newer start that won't
//                start (its own notice says so) -> none; disposed -> none
//
// An active restart (`active`) is one being followed, never a failure kept
// on screen: only it makes Settings' State say Saving (computer.js).

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
// a restart being followed (not a failure kept on screen)
export const active = (op) => !!op && op.state !== FAILED;

const reading = (s, name, more = {}) => ({ name, asked: s.asked, at: s.at, refused: s.refused, error: null, ...more });

// The machine: `s` (null, or a state as above) and an event to `{ s, fx }`.
// Events: press {asked, at, hidden} · tick · read_ok {v} · read_err
// {message} · ask_ok {v, hidden} · ask_err {message} · hidden · shown ·
// check_again {hidden} · view {v} (one the page took elsewhere) · dispose.
// Effects: ask · read · schedule · cancel (the timer, and any read under
// way). The same `s` back when nothing changed.
export function step(s, e) {
  const none = { s: null, fx: [{ do: "cancel" }] };
  if (e.type === "press") {
    if (s && s.name !== FAILED) return { s, fx: [] };
    const name = e.hidden ? PAUSED : FOLLOWING;
    return { s: { name, asked: e.asked, at: e.at, refused: null, error: null }, fx: [{ do: "cancel" }, { do: "ask" }, ...(name === FOLLOWING ? [{ do: "schedule" }] : [])] };
  }
  if (!s) return { s, fx: [] };
  switch (e.type) {
    case "dispose":
      return none;
    case "view":
      return finished(e.v, s.asked) ? none : { s, fx: [] };
    case "tick":
      return { s, fx: s.name === FOLLOWING ? [{ do: "read" }] : [] };
    case "read_ok": {
      // a read is only ever under way while following
      if (s.name !== FOLLOWING) return { s, fx: [] };
      if (finished(e.v, s.asked)) return none;
      if (s.refused && !underway(e.v, s.asked)) return { s: { name: FAILED, asked: s.asked, at: s.at, error: s.refused }, fx: [{ do: "cancel" }] };
      return { s: s.refused ? reading(s, FOLLOWING, { refused: null }) : s, fx: [{ do: "schedule" }] };
    }
    case "read_err":
      if (s.name !== FOLLOWING) return { s, fx: [] };
      return { s: reading(s, READ_FAILED, { error: e.message }), fx: [{ do: "cancel" }] };
    case "ask_ok": {
      if (s.name === FAILED) return { s, fx: [] };
      if (finished(e.v, s.asked)) return none;
      // newer than any read under way
      if (s.name === READ_FAILED) {
        const name = e.hidden ? PAUSED : FOLLOWING;
        return { s: reading(s, name), fx: name === FOLLOWING ? [{ do: "schedule" }] : [] };
      }
      return { s, fx: s.name === FOLLOWING ? [{ do: "cancel" }, { do: "schedule" }] : [] };
    }
    case "ask_err": {
      if (s.name === FAILED) return { s, fx: [] };
      const refused = reading(s, s.name, { refused: e.message, error: s.error });
      // decided by the next read, which waits while hidden or after a read
      // that failed, for shown or Check again
      return { s: refused, fx: s.name === FOLLOWING ? [{ do: "cancel" }, { do: "read" }] : [] };
    }
    case "hidden":
      return s.name === FOLLOWING ? { s: reading(s, PAUSED), fx: [{ do: "cancel" }] } : { s, fx: [] };
    case "shown":
      return s.name === PAUSED ? { s: reading(s, FOLLOWING), fx: [{ do: "read" }] } : { s, fx: [] };
    case "check_again": {
      if (s.name !== READ_FAILED) return { s, fx: [] };
      const name = e.hidden ? PAUSED : FOLLOWING;
      return { s: reading(s, name), fx: name === FOLLOWING ? [{ do: "read" }] : [] };
    }
  }
  throw new Error(`no restart event ${e.type}`);
}

// The runner. `read`: its view, read again; `ask`: the restart's own POST,
// of the start it restarts; `view`: the view the page shows now; `take`: a
// view read or answered, shown; `changed`: the state changed (draw it);
// `hidden`: the page is hidden. Its effects come in, so its tests drive it
// with a fake clock and held promises (cell/shell-tests/).
export function createRestartFollow({ read, ask, view, take, changed, hidden, timers = globalThis, now = () => Date.now() }) {
  let s = null;
  let timer = null;
  // a read's or an ask's outcome counts only while its `seq`/`run` stands
  let seq = 0;
  let run = 0;
  function cancel() {
    if (timer !== null) timers.clearTimeout(timer);
    timer = null;
    seq++;
  }
  function dispatch(e, draw = true) {
    const before = s;
    const next = step(s, e);
    s = next.s;
    if (s === null) run++;
    for (const f of next.fx) {
      if (f.do === "cancel") cancel();
      else if (f.do === "schedule") {
        if (timer !== null) timers.clearTimeout(timer);
        timer = timers.setTimeout(() => {
          timer = null;
          dispatch({ type: "tick" });
        }, FOLLOW_MS);
      } else if (f.do === "read") {
        const mine = ++seq;
        read().then(
          (v) => {
            if (mine !== seq) return;
            take(v);
            // drawing it may have found it done (`check`)
            if (mine === seq) dispatch({ type: "read_ok", v });
          },
          (err) => mine === seq && dispatch({ type: "read_err", message: err?.message ?? String(err) }),
        );
      } else if (f.do === "ask") {
        const mine = ++run;
        ask(s.asked).then(
          (v) => {
            if (mine !== run) return;
            take(v);
            if (mine === run) dispatch({ type: "ask_ok", v, hidden: hidden() });
          },
          (err) => mine === run && dispatch({ type: "ask_err", message: err?.message ?? String(err) }),
        );
      }
    }
    if (draw && s !== before) changed();
    return s !== before;
  }
  return {
    // what is shown: null, or { asked, at, state, error }
    op: () => s && { asked: s.asked, at: s.at, state: s.name, error: s.error },
    // Restart pressed: none runs (or the last one failed), so one begins
    press() {
      const c = view();
      if (!c) return false;
      return dispatch({ type: "press", asked: c.generation || 0, at: now(), hidden: hidden() });
    },
    // a view the page took from elsewhere (its list's read), while it is
    // drawing: done by it?
    check: () => dispatch({ type: "view", v: view() }, false) && s === null,
    // the page hidden or shown again
    visibility: () => dispatch({ type: hidden() ? "hidden" : "shown" }),
    // Check again, after a read that failed
    readAgain: () => dispatch({ type: "check_again", hidden: hidden() }),
    dispose: () => dispatch({ type: "dispose" }, false),
  };
}
