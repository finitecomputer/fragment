// What one message carries, at most: the platform's own word, `limits`
// (docs/api.md: a page's hello, read with `fragment.limits()`; the
// person's list, the shell's `state.limits`). Read where it is checked,
// never kept: a deploy may change it while a page stays open, and the
// socket's reconnect after it brings the new word. The chat's page and
// the shell's launcher check alike (cell/shell/limits.js is this file).

/// The bound in the platform's own number: "over 16 MB, the most a chat takes".
export const over = (limits) => `over ${Math.round(limits.uploadMaxBytes / 1e6)} MB, the most a chat takes`;

/// Why `files` ({name, size} each) cannot go in one message that holds
/// `held` more besides, under `limits`; null when they can, or when the
/// platform names none (an older one: its upload route decides).
export function refusal(limits, files, held = 0) {
  if (!limits) return null;
  if (held + files.length > limits.filesPerMessage) return `A message carries at most ${limits.filesPerMessage} files.`;
  const big = files.find((f) => f.size > limits.uploadMaxBytes);
  return big ? `${big.name || "A file"} is ${over(limits)}.` : null;
}
