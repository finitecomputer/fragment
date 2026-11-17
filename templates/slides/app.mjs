// A deck: notes on its pages are applib/notes.mjs (the doc template's,
// copied whole; a note's anchor is its page, `p1` for the first). Who
// presents live posts their page to the `present` channel, which
// fragment.json lets editors and up post to, and the page script
// (live/slides-live.js) follows; no code of the app's runs for it.
import { DurableObject } from "cloudflare:workers";
import { withNotes } from "./applib/notes.mjs";

export class App extends withNotes(DurableObject) {}
