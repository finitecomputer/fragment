// A chat's stand-in (Paul, 2026-11-01: "every nav to a chat potentially
// wakes a worker, i see a blank page flash"): while a frame of the middle
// column comes (its fragment woken, its page and files fetched), the shell
// shows the chat's empty room in the theme's colours and its composer's
// outline over it, nothing more: no message, no name, nothing it would
// have to guess. It goes once the page has loaded (its `load`, a page of
// its own: never the empty one a frame holds before its `src`) or spoken
// to the shell (`settled`, any message of its: shell.js), whichever comes
// first, and comes back if the frame is sent elsewhere (`again`). A quiet
// spinner shows in it only past 1.5 s (shell.css: an animation's delay,
// no timer here).

export function standIn(frame, el) {
  const box = el("div", "frame-standin");
  box.setAttribute("aria-hidden", "true");
  const composer = el("div", "standin-composer");
  box.append(el("span", "standin-wait"), composer);
  const loaded = () => {
    if (frame.getAttribute("src")) settled();
  };
  function settled() {
    box.remove();
    frame.removeEventListener("load", loaded);
  }
  // shown (again): beside its frame, until it loads or speaks
  function again() {
    if (!box.isConnected) frame.after(box);
    box.hidden = frame.hidden;
    frame.removeEventListener("load", loaded);
    frame.addEventListener("load", loaded);
  }
  frame.addEventListener("load", loaded);
  return { box, settled, again, shown: () => box.isConnected };
}
