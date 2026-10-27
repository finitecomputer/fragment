/* The shell's service worker for a chat's alerts (docs/api.md, The shell,
   "A chat's alerts are the shell's"): push only, no fetch handler, ever.
   The shell registers it once for each chat whose alerts are on, its scope
   /__shell/alerts/<person>/<chat>/, and subscribes that registration with
   the chat's own VAPID key (shell.js, "a chat's alerts"), so only the
   chat's fragment can push to it. Its person and chat are its scope's,
   never the push's word. */
const [person, chat] = new URL(self.registration.scope).pathname.split("/").filter(Boolean).slice(2).map(decodeURIComponent);

// What may be shown of a push now, as the platform answers this browser
// (as the shell asks it): "yes" while its person is the one signed in and
// the chat still runs the release's chat; "no" once no one is (401),
// another is, the person may no longer see the chat (403, 404), or it
// runs other code (a fork's, its author's); "unknown" while the platform
// cannot say (offline, an error), when only that something came is shown.
async function shown() {
  const ask = (path) => fetch(path, { headers: { "x-fragment-shell": "1" }, credentials: "same-origin", cache: "no-store" });
  try {
    const [me, status] = await Promise.all([ask("/api/identities/me"), ask(`/api/f/${encodeURIComponent(chat)}/status`)]);
    if ([401, 403].includes(me.status) || [401, 403, 404].includes(status.status)) return "no";
    if (!me.ok || !status.ok) return "unknown";
    const [who, s] = await Promise.all([me.json(), status.json()]);
    if (who.id !== person) return "no";
    const code = s && s.code && s.code.id;
    if (typeof code !== "string") return "unknown";
    return code.startsWith("blessed:chat@") ? "yes" : "no";
  } catch (e) {
    return "unknown";
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
      const may = await shown();
      if (may === "no") {
        // no longer its person's alerts here: it lets itself go, and the
        // push service then tells the chat its subscription is gone (410)
        const sub = await self.registration.pushManager.getSubscription().catch(() => null);
        await sub?.unsubscribe().catch(() => {});
        await self.registration.unregister().catch(() => {});
        return;
      }
      // one notification a chat: its latest reply
      const opts =
        may === "yes"
          ? {
              body: typeof p.body === "string" ? p.body : "",
              tag: chat,
              // who it is from: an agent's picture, an address on the web
              icon: typeof p.icon === "string" && /^https?:\/\//.test(p.icon) ? p.icon : undefined,
            }
          : { body: "Something new in one of your chats", tag: chat };
      const title = may === "yes" && typeof p.title === "string" && p.title ? p.title : "Finite";
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
