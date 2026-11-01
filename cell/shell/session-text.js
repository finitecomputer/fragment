// An imported conversation's words as History and the launcher's Knowledge
// show them: a search snippet of the Session Archive's index, whose section
// text is a turn's lines, "Person: …", "Agent: …" (with " [files]" and
// " (used tools)" at their end) and "Summary: …" (templates/sessions/README.md,
// "Section text"; `fragment_core::session_search`, its PERSON, AGENT and
// SUMMARY), said for a person: "You: … · Juniper: …", the files and tools
// left out. The index keeps its neutral words (an agent reading a section
// reads them so); this is their face alone.
//
// A snippet is a cut of a section (`…` where it was cut), so its first line
// may have lost its speaker: a line with none continues the one before, or
// nobody's at the start. A person may type words that look like a speaker,
// a file or a tool: the marks are taken off only at the end of a speaker's
// words, a display's guess, never the index's.

const SPEAKERS = [["Person: ", "person"], ["Agent: ", "agent"], ["Summary: ", "summary"]];
// at the end of a speaker's words: its tools, then its files, whole or cut short (`…`)
const USED = /\s*\(used [^)]*(?:\)|…)$/;
const FILES = /\s*\[[^\]]*(?:\]|…)$/;

export function speakers(snippet, agent = null) {
  const said = [];
  for (const line of String(snippet ?? "").split("\n")) {
    const [prefix, who] = SPEAKERS.find(([p]) => line.startsWith(p)) ?? [];
    if (who) said.push({ who, words: [line.slice(prefix.length)] });
    else if (said.length) said.at(-1).words.push(line);
    else said.push({ who: null, words: [line] });
  }
  const name = { person: "You", agent: agent?.trim() || "Agent", summary: "Summary" };
  const parts = [];
  for (const { who, words } of said) {
    let text = words.map((w) => w.trim()).filter(Boolean).join(" ");
    if (who === "agent") text = text.replace(USED, "");
    // files end any speaker's words, a snippet's first, speakerless ones too
    text = text.replace(FILES, "");
    text = text.trim();
    if (!text) continue;
    // one speaker's lines in a row are one
    const last = parts.at(-1);
    if (last && who && last.who === who) last.text += ` ${text}`;
    else parts.push({ who, text });
  }
  return parts.map(({ who, text }) => (who ? `${name[who]}: ${text}` : text)).join(" · ");
}
