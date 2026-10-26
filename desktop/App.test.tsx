import { expect, open, test } from "nsh/test";
import type { Workspace } from "./types";

const workspace: Workspace = {
  me: { id: "person:desktop", email: "desktop@e2e.test" },
  guest: false,
  fragments: [
    { name: "maple--k3x9", kind: "agent", title: "Maple", role: "owner" },
    { name: "maple-chat--p2m4", kind: "chat", title: "Your first chat", role: "owner", agents: ["agent:maple"] },
    { name: "garden-chat--q8n2", kind: "chat", title: "Garden plans", role: "owner", agents: ["agent:maple"], unread: 2, previewAt: 2 },
    { name: "groceries--m4z7", kind: "app", title: "Groceries", role: "owner" },
  ],
  computer: {
    computer: "computer:desktop",
    phase: "awake",
    version: "test-image",
    agents: [{ identity: "agent:maple", fragment: "maple--k3x9" }],
  },
  invites: [],
};

async function shell() {
  const app = await open("./App.tsx", {
    platform: "linux",
    width: 1440,
    height: 900,
    colorScheme: "dark",
    storage: { "desktop.fixture": JSON.stringify(workspace) },
  });
  await app.waitFor("text:Maple");
  return app;
}

test("shows the native workspace and opens settings", async () => {
  const app = await shell();
  expect(app.texts()).toContain("Finite.Computer");
  expect(app.texts()).toContain("Garden plans");
  await app.press("id:settings");
  await app.waitFor("text:Manage your account, your computer, and the tools your agents use.");
  expect(app.texts()).toContain("desktop@e2e.test");
  expect(app.texts()).toContain("Awake");
  await app.screenshot("../target/desktop-settings.png").catch(() => {});
});

test("opens the app dock and app-creation prompts", async () => {
  const app = await shell();
  await app.press("id:toggle-desktop");
  await app.screenshot("../target/desktop-dock.png").catch(() => {});
  await app.waitFor("text:Groceries");
  await app.press("id:add-app");
  await app.waitFor("text:Ask for an app");
  expect(app.texts()).toContain("Make a shared grocery list my family can add to from their phones");
  await app.screenshot("../target/desktop-app-prompts.png").catch(() => {});
});

test("switches chats and opens the launcher", async () => {
  const app = await shell();
  await app.press("id:chat-garden-chat--q8n2");
  await app.waitFor("text:Garden plans");
  await app.press("id:open-launcher");
  await app.waitFor("id:workspace-search");
  await app.screenshot("../target/desktop-launcher.png").catch(() => {});
  expect(app.value("id:workspace-search")).toBe("");
  await app.press("label:Close search");
  await app.waitFor("id:workspace-search", { state: "gone" });
});
