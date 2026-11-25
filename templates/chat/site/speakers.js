// Whose words a message is, and where it came from (docs/chat-records.md,
// "A person's message" & "A message heard in a room"):
// - A message an agent or hosted door relays from its owner is its owner's,
//   once the members say the poster is that owner's agent or none of the
//   chat's members (and its platform author stamp, when present, is "door").
//   Its room and context only when that stamp is there.
// - A message heard from an outside speaker in a room is that speaker's,
//   never a member's and never the owner's, only when posted by a non-member
//   carrier key (`npub1…`) the platform stamped author "door", with a valid
//   room. The bridge runs it by the same rule (images/bridge, `whose`).
// - From any other member, from app code, or before the members are read,
//   `relayed`, `heard`, `room`, and `context` are ignored.
//
// Kept free of platform imports so Node's tests import it directly
// (cell/shell-tests/chat-speakers.test.mjs).

const DOOR_ID = /^[a-z](?:[a-z0-9]|-(?=[a-z0-9])){0,31}$/;
const ROOM_ID = /^[A-Za-z0-9._-]{1,128}$/;
const HEARD_FROM_MAX_BYTES = 128;
const HEARD_NAME_MAX_CHARS = 80;
const ROOM_NAME_MAX_CHARS = 80;
const ROOM_THREAD_MAX_BYTES = 128;
const CONTEXT_MAX_MESSAGES = 20;
const CONTEXT_MAX_BYTES = 16 * 1024;
const CONTEXT_NAME_MAX_CHARS = 80;
const CONTEXT_TEXT_MAX_CHARS = 4000;
const UTF8 = new TextEncoder();

export const DOOR_NAMES = { simplex: "SimpleX", telegram: "Telegram", slack: "Slack" };
export const doorId = (v) => typeof v === "string" && DOOR_ID.test(v);
export const doorName = (v) => DOOR_NAMES[v] ?? v;

const byteLen = (s) => UTF8.encode(s).length;
const oneLine = (s) => s.replace(/[\u0000-\u001f\u007f]/g, " ").trim().replace(/\s+/g, " ");
const cut = (s, max) => {
  const chars = [...s];
  return chars.length <= max ? s : `${chars.slice(0, max - 1).join("")}…`;
};
const validDoorToken = (s, maxBytes) =>
  typeof s === "string" && s.length > 0 && byteLen(s) <= maxBytes && !/[\s\u0000-\u001f\u007f]/.test(s);
const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

// Strictly parses optional `relayed`, `heard`, `room`, and `context` off a
// `chat` message body. Trust (whether the poster may carry them) is checked
// separately against `members` at read time.
export function parseDoorFields(body, principal, turn, code = false) {
  const empty = { relayed: null, heard: null, room: null, context: [] };
  if (turn || code || !isObject(body)) return empty;
  const hasRelayed = body.relayed !== undefined && body.relayed !== null;
  const hasHeard = body.heard !== undefined && body.heard !== null;
  if (hasRelayed && hasHeard) return empty;

  let room = null;
  let roomMalformed = false;
  if (body.room !== undefined && body.room !== null) {
    const r = body.room;
    if (isObject(r) && typeof r.id === "string" && ROOM_ID.test(r.id)) {
      const nameOk = r.name === undefined || r.name === null || typeof r.name === "string";
      const threadOk = r.thread === undefined || r.thread === null || validDoorToken(r.thread, ROOM_THREAD_MAX_BYTES);
      if (nameOk && threadOk) {
        const name = typeof r.name === "string" ? cut(oneLine(r.name), ROOM_NAME_MAX_CHARS) : "";
        const thread = typeof r.thread === "string" ? r.thread : null;
        room = { id: r.id, name, thread };
      } else {
        roomMalformed = true;
      }
    } else {
      roomMalformed = true;
    }
  }

  let relayed = null;
  if (hasRelayed && !roomMalformed) {
    const r = body.relayed;
    if (isObject(r) && doorId(r.via) && typeof r.for === "string" && r.for.startsWith("npub1")) {
      relayed = { for: r.for, by: principal, via: r.via };
    }
  }

  let heard = null;
  if (hasHeard && room) {
    const h = body.heard;
    if (isObject(h) && doorId(h.via) && validDoorToken(h.from, HEARD_FROM_MAX_BYTES) && typeof h.name === "string") {
      const name = oneLine(h.name);
      if (name) heard = { via: h.via, from: h.from, name: cut(name, HEARD_NAME_MAX_CHARS) };
    }
  }

  let context = [];
  if (room && (heard || relayed) && Array.isArray(body.context) && body.context.length > 0 && body.context.length <= CONTEXT_MAX_MESSAGES) {
    let bytes = 0;
    const items = [];
    let ok = true;
    for (const c of body.context) {
      if (!isObject(c) || typeof c.name !== "string" || typeof c.text !== "string") {
        ok = false;
        break;
      }
      if (c.at !== undefined && c.at !== null && !Number.isSafeInteger(c.at)) {
        ok = false;
        break;
      }
      const name = oneLine(c.name);
      const text = oneLine(c.text);
      if (!name || !text) {
        ok = false;
        break;
      }
      bytes += byteLen(c.name) + byteLen(c.text);
      if (bytes > CONTEXT_MAX_BYTES) {
        ok = false;
        break;
      }
      items.push({
        name: cut(name, CONTEXT_NAME_MAX_CHARS),
        text: cut(text, CONTEXT_TEXT_MAX_CHARS),
        at: Number.isSafeInteger(c.at) ? c.at : null,
      });
    }
    if (ok) context = items;
  }

  return { relayed, heard, room: heard || relayed ? room : null, context };
}

const doorAuthorOk = (m) => !m.code && (!m.author || m.author === "door");
const byDoor = (m) => !m.code && m.author === "door";

export function relayedOwner(m, members) {
  if (!m.relayed || !members.length || !doorAuthorOk(m)) return null;
  const by = members.find((x) => x.principal === m.relayed.by);
  return !by || (by.kind === "agent" && by.owner === m.relayed.for) ? m.relayed.for : null;
}

// Outside speaker in a hosted door room: only from a non-member carrier key
// (`npub1…`, never a member or anonymous visitor) stamped a door's, with a
// valid room.
export function heardSpeaker(m, members) {
  if (!m.heard || !m.room || m.relayed || !members.length || !byDoor(m)) return null;
  if (typeof m.principal !== "string" || !m.principal.startsWith("npub1")) return null;
  if (members.some((x) => x.principal === m.principal)) return null;
  return m.heard;
}

export function roomOf(m, members) {
  if (!m.room || !members.length || !byDoor(m)) return null;
  return heardSpeaker(m, members) || relayedOwner(m, members) ? m.room : null;
}

export function contextOf(m, members) {
  if (!Array.isArray(m.context) || !m.context.length) return [];
  return roomOf(m, members) ? m.context : [];
}

// Human-readable origin label: `"Slack · #deploys"`, `"Slack · C0123"`,
// `"Telegram"`, or `null` for an ordinary chat message.
export function viaLabel(m, members) {
  const h = heardSpeaker(m, members);
  if (h) {
    const place = m.room.name || m.room.id;
    return `${doorName(h.via)} · ${place}`;
  }
  if (relayedOwner(m, members)) {
    const via = doorName(m.relayed.via);
    const r = roomOf(m, members);
    return r ? `${via} · ${r.name || r.id}` : via;
  }
  return null;
}

export const speakerOf = (m, members) => relayedOwner(m, members) ?? m.principal;

// When each poster (and whom a relay was for) last said something not of
// a turn: kept by who posted it, so whose words they are is read as the
// members say when it is asked, not as they said when it came (a relay
// come before the members were read is its owner's once they are).
export function noteSaid(spoke, m) {
  if (m.turn) return;
  const key = `${m.principal} ${m.relayed?.by ?? ""} ${m.relayed?.for ?? ""} ${m.heard?.from ?? ""}`;
  if (!(spoke.get(key)?.at >= m.at)) {
    spoke.set(key, {
      m: { principal: m.principal, relayed: m.relayed ?? null, code: !!m.code, author: m.author ?? null },
      at: m.at,
    });
  }
}

// Whether `who` said something not of a turn at `since` or after.
export const saidSince = (spoke, members, who, since) => [...spoke.values()].some((x) => x.at >= since && speakerOf(x.m, members) === who);
