/* The shell's service worker for a chat's alerts (docs/chat-records.md,
   Push): push only, no fetch handler, ever. The shell registers it once
   for each chat whose alerts are on, its scope /__shell/alerts/<chat>/,
   and subscribes that registration with the chat's own VAPID key (shell.js,
   "a chat's alerts"), so only the chat's fragment can push to it; the
   chat is its scope's, never the push's word. */
const chat = decodeURIComponent(new URL(self.registration.scope).pathname.split("/").filter(Boolean).pop() || "");

self.addEventListener("push", (event) => {
  let p = null;
  try {
    p = event.data && event.data.json();
  } catch (e) {}
  if (!p || typeof p !== "object") p = {};
  const title = (typeof p.title === "string" && p.title) || "Finite";
  const opts = {
    body: typeof p.body === "string" ? p.body : "",
    // one notification a chat: its latest reply
    tag: chat,
    // who it is from: an agent's picture, an address on the web
    icon: typeof p.icon === "string" && /^https?:\/\//.test(p.icon) ? p.icon : undefined,
  };
  event.waitUntil(self.registration.showNotification(title, opts).catch(() => {}));
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
