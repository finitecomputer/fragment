// Whose words a message is (docs/chat-records.md, "A person's message"):
// a message an agent relays from its owner through a door is its owner's,
// once the members say the relaying agent is that owner's; anyone else's
// is its poster's. `members` is the fragment's list as last read (empty
// before its first read: then a relay is its poster's).
export function relayedOwner(m, members) {
  if (!m.relayed || !members.length) return null;
  const by = members.find((x) => x.principal === m.relayed.by);
  return !by || (by.kind === "agent" && by.owner === m.relayed.for) ? m.relayed.for : null;
}

export const speakerOf = (m, members) => relayedOwner(m, members) ?? m.principal;

// When each poster (and whom a relay was for) last said something not of
// a turn: kept by who posted it, so whose words they are is read as the
// members say when it is asked, not as they said when it came (a relay
// come before the members were read is its owner's once they are).
export function noteSaid(spoke, m) {
  if (m.turn) return;
  const key = `${m.principal} ${m.relayed?.by ?? ""} ${m.relayed?.for ?? ""}`;
  if (!(spoke.get(key)?.at >= m.at)) spoke.set(key, { m: { principal: m.principal, relayed: m.relayed ?? null }, at: m.at });
}

// Whether `who` said something not of a turn at `since` or after.
export const saidSince = (spoke, members, who, since) => [...spoke.values()].some((x) => x.at >= since && speakerOf(x.m, members) === who);
