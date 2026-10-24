/* The shell's service worker for a chat's alerts (docs/api.md, The shell,
   "A chat's alerts are the shell's"): push only, no fetch handler, ever.
   The shell registers it once for each chat whose alerts are on, its scope
   /__shell/alerts/<person>/<chat>/, and subscribes that registration with
   the chat's own VAPID key (shell.js, "a chat's alerts"), so only the
   chat's fragment can push to it. Its person and chat are its scope's,
   never the push's word. */
const [person, chat] = new URL(self.registration.scope).pathname.split("/").filter(Boolean).slice(2).map(decodeURIComponent);

// Whether its person is the one signed in to this browser now: signing
// out (any way: the shell's form, the sign-out page, a session that ended)
// or another person signing in ends what it shows. A platform that cannot
// say (offline, an error) leaves it be: the push came.
async function theirs() {
  try {
    const r = await fetch("/api/identities/me", { headers: { "x-fragment-shell": "1" }, credentials: "same-origin", cache: "no-store" });
    if (r.status === 401 || r.status === 403) return false;
    if (!r.ok) return true;
    return (await r.json()).id === person;
  } catch (e) {
    return true;
  }
}

self.addEventListener("push", (event) => {
  let p = null;
  try {
    p = event.data && event.data.json();
  } catch (e) {}
  if (!p || typeof p !== "object") p = {};
  event.waitUntil(
    (async () => {
      if (!(await theirs())) {
        // no longer its person's browser: it lets itself go, and the push
        // service then tells the chat its subscription is gone (410)
        const sub = await self.registration.pushManager.getSubscription().catch(() => null);
        await sub?.unsubscribe().catch(() => {});
        await self.registration.unregister().catch(() => {});
        return;
      }
      const title = (typeof p.title === "string" && p.title) || "Finite";
      const opts = {
        body: typeof p.body === "string" ? p.body : "",
        // one notification a chat: its latest reply
        tag: chat,
        // who it is from: an agent's picture, an address on the web
        icon: typeof p.icon === "string" && /^https?:\/\//.test(p.icon) ? p.icon : undefined,
      };
      await self.registration.showNotification(title, opts).catch(() => {});
    })(),
  );
});

// A click opens the chat in the shell: one already open is shown it (the
// shell hears `{fragment: "open-chat"}`), else a new one opens at it
// (`/?chat=<chat>`).
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(
    (async () => {
      try {
        const list = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
        const shell = list.find((c) => ["/", "/settings"].includes(new URL(c.url).pathname));
        if (shell) {
          await shell.focus();
          return shell.postMessage({ fragment: "open-chat", name: chat });
        }
        await self.clients.openWindow(`/?chat=${encodeURIComponent(chat)}`);
      } catch (e) {}
    })(),
  );
});
