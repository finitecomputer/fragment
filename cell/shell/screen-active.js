// The person is at the shell: a key, a press, the pointer or a touch while
// the page is in sight. Each agent's screen it shows is told so (its frame
// sees only what happens inside it), at most once every ACTIVE_EVERY_MS,
// so a screen beside the chat its person types in keeps streaming, and one
// no one is at backs off to silence and lets its computer sleep
// (images/hermes/screen/attend.js; frame-messages.js, `screen`'s
// `active`). The shell wires it (shell.js): `frames` answers the screens'
// frames on view, `tell` posts to one.

// Well inside a screen's pause (attend.js, PAUSE_MS: 5 minutes), so a
// person at the shell never sees one pause.
export const ACTIVE_EVERY_MS = 30_000;

// Answers the shell's use handler: how many screens it told.
export function screenActive({ now, frames, tell, everyMs = ACTIVE_EVERY_MS }) {
  let told = -Infinity;
  return () => {
    const at = now();
    if (at - told < everyMs) return 0;
    const on = frames();
    // none on view: the next use tells the first one shown
    if (on.length === 0) return 0;
    told = at;
    let n = 0;
    for (const frame of on) {
      try {
        tell(frame);
        n += 1;
      } catch {}
    }
    return n;
  };
}
