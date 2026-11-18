// Whether a person is at an agent's screen (Paul, 2026-11-18: a view no one
// touches "should backoff to zero if there's no user interaction going on
// in the shell. just like computers go to sleep if you don't move your
// mouse!"). An open port tab holds its computer only while its page speaks
// (docs/computers.md, Ports), and an RFB client speaks for as long as its
// screen draws (a fence or an update request for each update), so the
// screen stops its stream once no one has touched it for `PAUSE_MS`: its
// tab goes quiet and its computer sleeps as under no tab. The person's next
// touch brings it back. Once its sockets are gone (its computer slept, or
// restarted, or the network went), only a press or a key opens them again,
// once each: a hand passing over it wakes no computer, and nothing here
// opens anything on a timer.
//
// Pure, its clock and timers handed in, so cell/shell-tests can run it: the
// page (index.html) tells it each use and each socket's fate, and does what
// it answers through `pause`, `resume` and `wake`.

// No touch for this long and the stream pauses: a screen saver's wait, so a
// person who watches an agent work without touching it keeps the stream a
// while; their computer sleeps IDLE_MS (20 minutes) after the stream goes.
export const PAUSE_MS = 5 * 60_000;

// A use: `press` (a pointer or a touch down, a key) or `move` (the pointer,
// the wheel, the page shown again, or the shell's word that its person is
// at it, frame-messages.js `screen`'s `active`).
export const USES = Object.freeze(["press", "move"]);

// live: streaming; paused: stopped for want of anyone, its computer up;
// gone: its sockets closed, opened again only by a press; waking: a press
// opened them, and the page waits to hear whether they came up.
export function attention({ now, setTimer, clearTimer, pause, resume, wake, pauseMs = PAUSE_MS }) {
  if (!(pauseMs > 0)) throw new Error("a pause is a span of time");
  let state = "live";
  let last = now();
  let timer = null;
  const arm = () => {
    clearTimer(timer);
    timer = setTimer(due, Math.max(0, last + pauseMs - now()));
  };
  function due() {
    timer = null;
    if (state !== "live") return;
    if (now() - last < pauseMs) return arm();
    state = "paused";
    pause();
  }
  arm();
  return {
    state: () => state,
    // the person used it (`press` or `move`, `USES`)
    used(kind) {
      if (!USES.includes(kind)) throw new Error(`no use ${kind}`);
      last = now();
      if (state === "paused") {
        state = "live";
        arm();
        resume();
      } else if (state === "gone" && kind === "press") {
        state = "waking";
        wake();
      }
    },
    // its sockets came up after a press woke them: live, its pause counted
    // from that press (a stream that comes back by itself is no use, so a
    // desktop that keeps restarting keeps nothing awake)
    up() {
      if (state !== "waking") return;
      state = "live";
      arm();
    },
    // its sockets closed: nothing opens them but the person's next press
    gone() {
      clearTimer(timer);
      timer = null;
      state = "gone";
    },
    // whether a stream that closed by itself (its desktop restarted on a
    // computer still up, or came up slowly after a press) may be opened
    // again: only within PAUSE_MS of its person's last use
    attended: () => (state === "live" || state === "waking") && now() - last < pauseMs,
  };
}
