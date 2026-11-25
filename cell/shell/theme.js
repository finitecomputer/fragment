// The shell's light or dark (FIN-652): the person's choice on this device
// (Settings, Theme), or the system's while they keep System, the default.
// The root's data-theme says which, as each app's frame says its own
// (chat.css): the shell's styles follow it (shell.css, platform-pages.css),
// and shell.js tells the frames. Stored like the layout, so a browser
// without storage keeps System.
import { store } from "./layout.js";

const KEY = "finite.theme.v1";
export const THEMES = Object.freeze(["system", "light", "dark"]);
const system = matchMedia("(prefers-color-scheme: dark)");
const listeners = new Set();
// the bars around the page (index.html's theme-color metas): each keeps its
// system colour under System, and both take the chosen one otherwise
const bars = [...document.querySelectorAll('meta[name="theme-color"]')].map((meta) => ({
  meta,
  own: meta.content,
  mode: meta.media.includes("dark") ? "dark" : "light",
}));

export function themeChoice() {
  const chosen = store.get(KEY, "system");
  return THEMES.includes(chosen) ? chosen : "system";
}

export function themeMode() {
  const chosen = themeChoice();
  if (chosen !== "system") return chosen;
  return system.matches ? "dark" : "light";
}

function apply() {
  const chosen = themeChoice();
  const mode = themeMode();
  document.documentElement.dataset.theme = mode;
  const colour = bars.find((b) => b.mode === mode)?.own;
  for (const b of bars) b.meta.content = chosen === "system" || !colour ? b.own : colour;
  for (const f of listeners) f(mode);
}

export function chooseTheme(chosen) {
  if (!THEMES.includes(chosen)) throw new Error(`No such theme: ${chosen}`);
  store.set(KEY, chosen);
  apply();
}

// `f(mode)` each time the shell's light or dark may have changed
export function onThemeChange(f) {
  listeners.add(f);
  return () => listeners.delete(f);
}

system.addEventListener("change", () => { if (themeChoice() === "system") apply(); });
// another tab's choice
addEventListener("storage", (e) => { if (e.key === KEY) apply(); });
apply();
