// Runs before open-slide's app: the fragment's own page (/) opens its one
// deck, so the app starts there (scripts/build.mjs puts this first).
const deck = document.querySelector('meta[name="slides-deck"]')?.content;
if (deck && location.pathname === "/") history.replaceState(history.state, "", `/s/${deck}${location.search}${location.hash}`);
