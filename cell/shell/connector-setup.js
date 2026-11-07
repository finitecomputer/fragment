// A connector's setup, in a dialog of the shell's own (Paul, 2026-11-04 and
// 2026-11-07: its setup happens in Settings, in the shell's look): its
// title, the agent it is for hugging it, then the connector's own setup
// page, framed and signed in as any of the person's fragments is (shell.js
// `framed`), or a frame its caller made (SimpleX's link page, a port of its
// agent's computer: connections.js), and Done. What the page does is the
// connector's (templates/telegram/site/setup.html, images/bridge's
// simplex/page.html); the dialog knows only where it is.
//
// The frame is one height, the tallest step of such a page on a phone (a
// longer page scrolls within it), so no message from the page sizes it;
// `tall`, on a wider screen, the height of a QR code's step. A
// frame that can't sign in (Safari's cookie rule) says so in its place,
// its link opening that same page in a tab (shell.js, BLOCKED). Done or
// Escape closes it, and `onClose` follows.
//
// A Set up's dialog opens once what it waits on answers (a port ticket, a
// connector made): each press takes its turn first (`pressSetup`), and only
// the latest press's opens, so an answer that comes late never closes the
// dialog of a Set up pressed after it.
export function createConnectorSetup({ el, named, frameOf }) {
  let open = null;
  let presses = 0;
  const pressSetup = () => ++presses;
  function openSetup({ pressed, title, agent, src, name, frame, tall = false, onClose }) {
    if (pressed !== presses) return null;
    frame ??= frameOf(src, `${title} setup`, name);
    open?.close();
    const dialog = el("dialog", "rename-agent connector-setup");
    dialog.setAttribute("aria-labelledby", "connector-setup-title");
    const head = el("h2", null, title);
    head.id = "connector-setup-title";
    const who = named(el("p", "connector-setup-agent"), agent);
    frame.className = tall ? "connector-setup-frame tall" : "connector-setup-frame";
    const done = el("button", null, "Done");
    done.type = "button";
    done.onclick = () => dialog.close();
    const actions = el("div", "rename-actions");
    actions.append(done);
    dialog.append(head, who, frame, actions);
    document.body.append(dialog);
    open = dialog;
    dialog.addEventListener("close", () => {
      dialog.remove();
      if (open === dialog) open = null;
      onClose?.();
    });
    dialog.showModal();
    return dialog;
  }
  return { pressSetup, openSetup };
}
