# The Hermes image's Bot Mode keeper (images/hermes/boot/src/bots.rs;
# docs/computers.md, "Bot Mode in our Hermes image"), started by hermes-boot
# as the hermes user, in Hermes' own Python, for the computer's life.
#
# A bot's `message_agent` reaches its teammate's Bot Chat (its own chat with
# its owner, whose session the image's hook titles "Bot Chat") through
# Hermes' own path: a turn of Hermes' own outside the gateway, so outside
# the bridge, unless that Bot Chat has a live owner (v0.21.7:
# tools/bot_mode_dm.py, tools/bot_live_delivery.py; Hermes Desktop is one).
# The keeper is that owner, for each bot in the boot's bots file, about
# once a second:
#
# - it holds the bot's Bot Chat session (the compression tip of the session
#   titled "Bot Chat", when that session is its own chat's) as its live
#   owner: an active-session lease marked `bot_live_delivery_consumer`,
#   so a teammate's message is queued in the bot's mailbox;
# - it takes each message queued there and posts it into the bot's own chat
#   as the teammate that sent it, naming the bot (`to`) and the sender's
#   turn it was sent from (`back`): an agent's ask, as `fragment ask` posts
#   one (docs/chat-records.md, "An agent's ask"), but adding the sender to
#   no chat. So the bot's answer is an ordinary turn of the bridge's in its
#   own chat, which its owner sees, and once that turn ends the bridge
#   gives the sender a turn in its own chat with the answer (images/bridge,
#   engine.rs `answer_back`), from its state and journal, which a sleep or
#   a restart keeps; nothing waits for it;
# - it settles the delivery at once. That ends Hermes' wait for it (the
#   sender's background process), whose end wakes the sender: a turn of its
#   own after the one that sent the message, told to say nothing.
#
# Hermes' delivery names no turn of the sender's, so the keeper reads it
# from the sender's own chat, where `message_agent` runs (a Bot Chat only):
# the sender's newest turn there, since the bridge runs one turn of an
# agent's in a chat at a time. A turn too old for the bridge to answer back
# to (a message taken long after it was sent) gives no `back`: the sender
# is told the answer stays in its teammate's chat.
#
# A bot answers a teammate once. Hermes' roster tells a bot that a teammate
# messaged to answer with `message_agent`, but its answer goes back on its
# own. So the posted message ends with a line telling the bot to answer
# there; and a message it sends that teammate anyway while answering it
# (its newest turn's cause is that teammate's message) is posted as a
# reply: with no `back`, so nothing more goes back and forth.
#
# A fenced bot runs in Bot Mode too (decision 63, proposed), as itself
# alone: its `message_agent` reaches its owner's lead (their main agent, the
# bots file's `lead`), and answers whoever asked it; the keeper refuses any
# other, saying so. Its message is posted into its own chat (the one chat it
# is in that holds nothing of another bot's), as itself, naming the
# teammate, whom the keeper adds to that chat (an unfenced agent, which
# reads its owner's chats already), with a line saying the sender is fenced.
# A message to a fenced bot is posted into its own chat as to any bot.
#
# A bot whose own chat has had no turn yet has no Bot Chat session. Hermes
# lists it on every roster all the same (every profile is), and would run a
# message to it as a turn of its own, outside the bridge, which no one sees.
# So the keeper holds a stand-in for it: a session of its own in the bot's
# state.db (id STANDIN), titled "Bot Chat", which the hook takes the title
# from at the first turn in the bot's own chat, and which the keeper makes
# again when Hermes prunes it, empty.
#
# A delivery the keeper claimed and never settled (it stopped between the
# two: a sleep, a crash) is finished once the keeper holds that bot's Bot
# Chat again: posted under the same id (`dm-<delivery>`, which the platform
# answers as a replay), then settled.
#
# It writes nothing while the platform holds the computer (`FRAGMENT_HOLD`),
# so a save never copies a mailbox or a lease it is writing.
#
# This file is the image's, not the repo's tooling: it runs inside Hermes'
# own Python, whose session and mailbox code only it can call.
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

RUN = os.environ.get("FRAGMENT_RUN", "/var/lib/fragment-run")
BOTS = os.path.join(RUN, "bots.json")
HOLD = os.environ.get("FRAGMENT_HOLD", "/run/computer/hold")
# The platform's API, as the computer reaches it (docs/computers.md, "The
# fragment API"): a request names the agent it acts as, and the computer
# signs it as that agent.
API = os.environ.get("FRAGMENT_API", "http://api.fragment.internal").rstrip("/")
TITLE = "Bot Chat"
# The keeper's stand-in session, in each bot's own state.db (`bot_chat`).
STANDIN = "fragment-bot-mode-standin"
EVERY_S = 1.0
# One request to the platform, at most this long.
CALL_S = 30
# The sender's chat's `work`, read back from its newest record for the
# sender's newest turn: this many records a page, at most PAGES_MAX pages.
PAGE_RECORDS = 200
PAGES_MAX = 10
# The bridge answers back to a turn that runs, or that ended within 5
# minutes (images/bridge, limits.rs `ENDED_HOPS_MS`); a turn ended longer
# ago than this is taken as too old (the margin: the platform's clock,
# which stamps a record, is not the computer's).
FRESH_MS = 4 * 60 * 1000
# Messages taken from one bot's mailbox in a pass, at most.
TAKE_MAX = 16
# The line a teammate's message ends with, in the chat it is posted to.
ANSWER_HERE = "\n\n(Your reply here goes back to {name} on its own: answer here, and don't message_agent {name} about this.)"
# The line a reply ends with: a message sent by a bot answering its asker.
REPLY = "\n\n({name} sent you this while answering what you asked it. Its answer also comes back to you on its own: pass on only what is new.)"
# What the sender is told when its delivery is settled, as a turn of its own
# after the one that sent it (`{name}` is the teammate's).
SETTLED = {
    "back": "{name} has your message, and its answer comes back to you on its own, as a turn of yours: so end this turn with exactly [SILENT].",
    "reply": "{name} has your message, and your answer goes back to {name} on its own: so end this turn with exactly [SILENT].",
    "none": "{name} has your message, in {chat}, and answers there: its answer comes back to you only if it messages you. If your owner is waiting on it, tell them where to look.",
}
# The line a fenced sender's message ends with (decision 63, proposed).
FENCED = "\n\n({name} is fenced, on a model outside the platform: what you send it leaves the platform, so give it placeholders for your owner's private details.)"
# Why a fenced sender's message to another teammate is refused.
REFUSED = "You are fenced: your message_agent reaches only {lead}, and answers whoever asked you. Ask {lead} to involve {name}."

# Hermes' attribution of a teammate's message (tools/bot_mode_dm.py): the
# sender's friendly name.
ATTRIBUTION = re.compile(r"Message from \S+ (.+?) \(@[^)]+\): ")
# A delivery's id (tools/bot_live_delivery.py, `_delivery_id`).
DELIVERY_ID = re.compile(r"[0-9a-f]{32,64}")

from hermes_state import SessionDB  # noqa: E402
from hermes_cli.active_sessions import release_active_session, try_acquire_active_session  # noqa: E402
from tools.bot_failure_reasons import classify_agent_error  # noqa: E402
from tools.bot_live_delivery import (  # noqa: E402
    DELIVERY_DIR_NAME, claim_pending_delivery, complete_delivery, find_canonical_live_owner, has_mailbox,
    read_delivery_result,
)


def ev(event, **fields):
    """One JSON line on stderr, as the boot's and the bridge's events: ids, never text."""
    sys.stderr.write(json.dumps({"at": int(time.time() * 1000), "event": "botmode." + event, **fields}) + "\n")
    sys.stderr.flush()


def held():
    return os.path.exists(HOLD)


def until_unheld():
    # bounded by the platform's hold, which it lifts after its save
    while held():
        time.sleep(0.2)


def read_bots():
    try:
        with open(BOTS, encoding="utf-8") as f:
            bots = json.load(f)["bots"]
    except (OSError, ValueError, KeyError, TypeError):
        return []
    keys = ("agent", "identity", "owner", "profile", "home", "chat", "sessionKey")
    return [b for b in bots if isinstance(b, dict) and all(isinstance(b.get(k), str) and b[k] for k in keys) and all(isinstance(b.get(k), bool) for k in ("fenced", "lead"))]


def bot_chat(bot):
    """The session the keeper holds for the bot, as Hermes finds a Bot Chat's owner (the compression
    tip of the session titled "Bot Chat"): its own chat's, once that chat has had a turn; else the
    keeper's stand-in, made and titled here when none holds the title (or Hermes' own does). None while
    the platform holds the computer and the stand-in is to be made."""
    path = Path(bot["home"]) / "state.db"

    def ours(titled):
        return titled is not None and (titled["id"] == STANDIN or titled.get("session_key") == bot["sessionKey"])

    if path.is_file():
        db = SessionDB(db_path=path, read_only=True)
        try:
            titled = db.get_session_by_title(TITLE)
            if ours(titled):
                return db.get_compression_tip(titled["id"]) or titled["id"]
        finally:
            db.close()
    if held():
        return None
    # written only to make the stand-in: looked at again as the writer
    db = SessionDB(db_path=path)
    try:
        titled = db.get_session_by_title(TITLE)
        if ours(titled):
            return db.get_compression_tip(titled["id"]) or titled["id"]
        if titled is not None:
            db.set_session_title(titled["id"], "")
        db.create_session(STANDIN, "fragment-bot-mode")
        db.set_session_title(STANDIN, TITLE)
        ev("standin", agent=bot["agent"], freed=titled["id"] if titled is not None else None)
        return STANDIN
    finally:
        db.close()


class Owner:
    """The keeper's live ownership of one bot's Bot Chat session."""

    def __init__(self, bot):
        self.home = str(Path(bot["home"]).resolve())
        self.live = "fragment-bot-mode:" + bot["profile"]
        self.session = None
        self.lease = None
        # whether a claim a keeper before this one left unsettled was looked for
        self.recovered = False

    def pin(self):
        """The owner a delivery for this bot is pinned to (tools/bot_live_delivery.py)."""
        return {"profile_home": self.home, "session_id": self.session, "lease_id": self.lease.lease_id, "live_session_id": self.live}

    def hold(self, bot, session):
        """Holds `session` as the bot's Bot Chat's live owner: whether it does."""
        if self.lease is not None and self.session == session:
            # still the canonical owner Hermes finds (a lease pruned or taken is taken again)
            found = find_canonical_live_owner(self.home)
            if found is not None and found.get("lease_id") == self.lease.lease_id:
                return True
        if held():
            return False
        if self.lease is not None:
            release_active_session(self.lease)
            self.lease = None
        lease, refusal = try_acquire_active_session(
            session_id=session, surface="fragment-bot-mode", config=None,
            metadata={"live_session_id": self.live, "bot_live_delivery_consumer": True},
            registry_home=self.home,
        )
        if lease is None:
            ev("own_refused", agent=bot["agent"], session=session, error=str(refusal)[:300])
            return False
        self.session, self.lease = session, lease
        ev("owned", agent=bot["agent"], session=session)
        return True


class Refused(Exception):
    """The platform answered a request, and refused it."""

    def __init__(self, status, text):
        super().__init__(text)
        self.status = status


def call(method, path, sender, body=None):
    """One request to the platform as `sender` (an agent of this computer), acting for its owner; a
    fenced one as itself alone, which it only ever is (decision 63, proposed)."""
    url = f"{API}{path}" if sender["fenced"] else f"{API}{path}{'&' if '?' in path else '?'}for={urllib.parse.quote(sender['owner'], safe='')}"
    data = None if body is None else json.dumps(body).encode()
    headers = {"x-fragment-agent": sender["agent"]} | ({"content-type": "application/json"} if data is not None else {})
    try:
        with urllib.request.urlopen(urllib.request.Request(url, data=data, method=method, headers=headers), timeout=CALL_S) as r:
            return json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        raise Refused(e.code, f"{method} {path.split('?')[0]}: {e.code} {e.read()[:300].decode(errors='replace')}") from None


def records_of(page):
    return [r for r in page.get("records") or [] if isinstance(r, dict) and isinstance(r.get("body"), dict)]


def newest_turn(sender):
    """The sender's newest turn in its own chat, the one its `message_agent` ran in: its
    `turn.start`'s body, and whether the bridge still answers back to it (it runs, or it ended
    within FRESH_MS); (None, False) when the sender has no turn there."""
    chat, me = sender["chat"], sender["identity"]
    listed = call("GET", f"/api/f/{chat}/channels", sender)["channels"]
    cursor = next((c.get("seq") or 0 for c in listed if c.get("name") == "work"), 0) + 1
    ends = {}
    # bounded: PAGES_MAX pages, read back from the newest record
    for _ in range(PAGES_MAX):
        if cursor <= 1:
            break
        page = call("GET", f"/api/f/{chat}/channels/work?before={cursor}&limit={PAGE_RECORDS}", sender)
        records = records_of(page)
        for r in reversed(records):
            body = r["body"]
            if body.get("kind") == "turn.end":
                ends.setdefault(body.get("turn"), r)
            elif body.get("kind") == "turn.start" and body.get("agent") == me and isinstance(body.get("turn"), str):
                end = ends.get(body["turn"])
                return body, end is None or int(time.time() * 1000) - int(end.get("at") or 0) <= FRESH_MS
        following = page.get("next", cursor)
        if not records or following >= cursor:
            break
        cursor = following
    return None, False


def asker_of(sender, start):
    """Who wrote the record that caused the turn `start` begins, when that is a message in the
    sender's own chat; None otherwise."""
    cause = start.get("cause") or {}
    seq = cause.get("seq")
    if cause.get("channel") != "chat" or cause.get("fragment") not in (None, sender["chat"]) or not isinstance(seq, int) or seq < 1:
        return None
    page = call("GET", f"/api/f/{sender['chat']}/channels/chat?after={seq - 1}&limit=1", sender)
    return next((r.get("principal") for r in records_of(page) if r.get("seq") == seq), None)


def name_of(message, fallback):
    """The sender's name as Hermes attributes its message, or `fallback`."""
    found = ATTRIBUTION.match(message or "")
    return found.group(1) if found else fallback


def bot_name(bot):
    """The bot's name as its profile says it (its Bot Mode title: bots.rs), else its label."""
    try:
        import hermes_yaml

        data = hermes_yaml.safe_load((Path(bot["home"]) / "profile.yaml").read_text(encoding="utf-8")) or {}
        title = ((data.get("ui_meta") or {}).get("hermes-bots") or {}).get("title") or data.get("display_name")
        return str(title).strip() or bot["agent"].split(".")[0]
    except Exception:
        return bot["agent"].split(".")[0]


def join(chat, agent):
    """`agent` (an unfenced one) an editor of `chat`, acting for its owner, as `fragment ask` adds an
    agent to a chat it is asked in: so it answers a fenced sender there."""
    members = call("GET", f"/api/f/{chat}/members", agent).get("members") or []
    if not any(m.get("principal") == agent["identity"] for m in members):
        call("PUT", f"/api/f/{chat}/members/{urllib.parse.quote(agent['identity'], safe='')}", agent, {"role": "editor"})
        ev("joined", agent=agent["agent"], chat=chat)


def deliver(bot, sender, owner, claimed, bots):
    """A teammate's message, posted as an agent's ask (asked from its newest turn, a reply, or neither:
    `how`), and its delivery settled: into the bot's own chat by the sender for their owner, or a
    fenced sender's into its own chat as itself, when it reaches the lead or answers its asker
    (`refused` otherwise)."""
    delivery = claimed["delivery_id"]
    t = time.time()
    status, reply, error, reason, how = "failed", "", "", "", None
    try:
        if sender is None:
            error = "the sender is no agent of this computer"
        else:
            them = name_of(claimed["message"], sender["agent"].split(".")[0])
            start, fresh = newest_turn(sender)
            replying = start is not None and asker_of(sender, start) == bot["identity"]
            if sender["fenced"] and not bot["lead"] and not replying:
                lead = next((b for b in bots if b["lead"]), None)
                how, error = "refused", REFUSED.format(lead=bot_name(lead) if lead else "your owner's main agent", name=bot_name(bot))
            else:
                body = {"text": claimed["message"], "to": [bot["identity"]]}
                if replying:
                    how = "reply"
                    body["text"] += REPLY.format(name=them)
                elif fresh:
                    how = "back"
                    body["text"] += ANSWER_HERE.format(name=them)
                    body["back"] = start["turn"]
                else:
                    how = "none"
                # a fenced sender's own chat, the one it is in that holds nothing of another's
                chat = bot["chat"]
                if sender["fenced"]:
                    chat = sender["chat"]
                    body["text"] += FENCED.format(name=them)
                    join(chat, bot)
                try:
                    call("POST", f"/api/f/{chat}/channels/chat", sender, {"id": "dm-" + delivery[:32], "body": body})
                except Refused as e:
                    # posted before under this id (a claim finished again), its body then another
                    if e.status != 409:
                        raise
                status, reply = "settled", SETTLED[how].format(name=bot_name(bot), chat=chat)
    except Exception as e:  # the delivery is settled whatever went wrong
        error = f"{type(e).__name__}: {e}"[:600]
    if status == "failed":
        reason = classify_agent_error(error)
    until_unheld()
    try:
        complete_delivery(owner.home, delivery, status=status, reply=reply, error=error, reason=reason)
    except Exception as e:
        ev("settle_failed", agent=bot["agent"], delivery=delivery, error=f"{type(e).__name__}: {e}"[:300])
    ev("delivered", agent=bot["agent"], sender=sender["agent"] if sender else None, delivery=delivery, status=status, how=how, reason=reason or None, ms=int((time.time() - t) * 1000))


def sender_of(claimed, bots):
    """The bot of this computer that sent a delivery, or None."""
    author = str((claimed.get("author") or {}).get("id") or "")
    return next((b for b in bots if "bot:" + b["profile"] == author), None)


def left_claimed(owner):
    """The deliveries in the bot's mailbox claimed and never settled: a keeper's before this one."""
    root = Path(owner.home) / "runtime" / DELIVERY_DIR_NAME
    found = []
    for path in sorted(root.glob("*.json")):
        if DELIVERY_ID.fullmatch(path.stem) is None:
            continue
        try:
            record = read_delivery_result(owner.home, path.stem)
        except Exception:
            continue
        if record is not None and record.get("status") == "claimed" and isinstance(record.get("message"), str):
            found.append(record)
    return found


def take(bot, bots, owner):
    """Each message queued for the bot, delivered (up to TAKE_MAX a pass), after any a keeper before
    this one claimed and left."""
    if not has_mailbox(owner.home):
        return
    if not owner.recovered:
        owner.recovered = True
        for claimed in left_claimed(owner):
            ev("recovered", agent=bot["agent"], delivery=claimed["delivery_id"])
            deliver(bot, sender_of(claimed, bots), owner, claimed, bots)
    # bounded: TAKE_MAX a pass
    for _ in range(TAKE_MAX):
        if held():
            return
        claimed = claim_pending_delivery(owner.home, owner.pin())
        if claimed is None:
            return
        sender = sender_of(claimed, bots)
        ev("taken", agent=bot["agent"], sender=sender["agent"] if sender else None, delivery=claimed["delivery_id"])
        deliver(bot, sender, owner, claimed, bots)


def main():
    ev("started", pid=os.getpid())
    owners = {}
    failing = {}
    # bounded by the computer's life: one pass per EVERY_S
    while True:
        if not held():
            bots = read_bots()
            # an agent no longer on this computer: its Bot Chat let go
            for gone in [a for a in owners if not any(b["agent"] == a for b in bots)]:
                owner = owners.pop(gone)
                if owner.lease is not None:
                    release_active_session(owner.lease)
                ev("released", agent=gone)
            for bot in bots:
                try:
                    session = bot_chat(bot)
                    if session is not None:
                        owner = owners.setdefault(bot["agent"], Owner(bot))
                        if owner.hold(bot, session):
                            take(bot, bots, owner)
                    failing.pop(bot["agent"], None)
                except Exception as e:  # one bot's trouble never stops another's
                    # said once per kind of failure, not once a second
                    said = f"{type(e).__name__}: {e}"[:300]
                    if failing.get(bot["agent"]) != said:
                        failing[bot["agent"]] = said
                        ev("failed", agent=bot["agent"], error=said)
        time.sleep(EVERY_S)


if __name__ == "__main__":
    main()
