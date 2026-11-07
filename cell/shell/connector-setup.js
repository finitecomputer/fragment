// A connector's setup, in a dialog of the shell's own (Paul, 2026-11-04 and
// 2026-11-07: its setup happens in Settings, in the shell's look): its
// title, the agent it is for hugging it, then the connector's own setup
// page, framed and signed in as any of the person's fragments is (shell.js
// `framed`), and Done. What the page does is the connector template's
// (templates/telegram/site/setup.html); the dialog knows only where it is.
//
// The frame is one height, the tallest step of such a page on a phone (a
// longer page scrolls within it), so no message from the page sizes it. A
// frame that can't sign in (Safari's cookie rule) says so in its place,
// its link opening that same page in a tab (shell.js, BLOCKED). Done or
// Escape closes it, and `onClose` follows.
export function createConnectorSetup({ el, named, frameOf }) {
  let open = null;
  function openSetup({ src, name, agent, title, onClose }) {
    open?.close();
    const dialog = el("dialog", "rename-agent connector-setup");
    dialog.setAttribute("aria-labelledby", "connector-setup-title");
    const head = el("h2", null, title);
    head.id = "connector-setup-title";
    const who = named(el("p", "connector-setup-agent"), agent);
    const frame = frameOf(src, `${title} setup`, name);
    frame.className = "connector-setup-frame";
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
  return { openSetup };
}
