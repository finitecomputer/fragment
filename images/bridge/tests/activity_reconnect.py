"""The activity plugin's socket holds while children run however it is
opened again after the bridge drops it: by a tool's end (tool first) or by
its hold worker (worker first). With no child running it holds nothing, and
a tool's end is never sent twice. Each model response that asks for calls
is named once, so a provider's id a later response says again is another
call, and a call's end names its response. Method: the plugin, its
connection a stub of the bridge's, answering as the bridge does
(alexlwn123's review of #664 found a socket a tool's end opened again never
held; his review of #823, a reused `call_0` recorded once)."""
import importlib.util
import json
import os
import threading
import time

os.environ.update(GATEWAY_RELAY_URL="http://127.0.0.1:1", GATEWAY_RELAY_ID="test", GATEWAY_RELAY_SECRET="0" * 32)
spec = importlib.util.spec_from_file_location("fragment_activity", "/opt/hermes/plugins/fragment-activity/__init__.py")
activity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(activity)
sockets = []


class Socket:
    """A connection the bridge took: what was said on it, each frame
    answered as the bridge answers it; closed, it takes nothing."""

    def __init__(self, *args, **kwargs):
        self.said, self.answers, self.closed = [], ["ready"], False
        sockets.append(self)

    def send(self, text):
        if self.closed:
            raise ConnectionError("closed by the bridge")
        frame = json.loads(text)
        self.said.append(frame)
        self.answers.append("working" if "working" in frame else "called" if "call" in frame else "observed")

    def recv(self, timeout=None):
        if self.closed:
            raise ConnectionError("closed by the bridge")
        return self.answers.pop(0)

    def ping(self):
        if self.closed:
            raise ConnectionError("closed by the bridge")
        pong = threading.Event()
        pong.set()
        return pong

    def close(self):
        self.closed = True

    def holds(self):
        """Whether the bridge holds for this socket: its last word on work."""
        said = [f["working"] for f in self.said if "working" in f]
        return bool(said) and said[-1]

    def ends(self):
        return [f["tool_call_id"] for f in self.said if "tool_call_id" in f]


activity.connect = Socket
activity._turns["turn-1"] = ["m-1", None]


def end(n):
    activity.tool_finished(tool_name="terminal", args={}, tool_call_id=f"call_{n}", turn_id="turn-1", status="ok", result="{}")


def opened(n):
    """Waits, at most 10 s, for the `n`th socket."""
    deadline = time.monotonic() + 10
    while len(sockets) < n:
        assert time.monotonic() < deadline, f"socket {n} never opened"
        time.sleep(0.05)
    return sockets[n - 1]


# no children: a tool's end opens a socket that holds nothing
end(0)
assert (sockets[0].ends(), sockets[0].holds()) == (["call_0"], False), sockets[0].said
# a child starts: the socket holds
activity.child_started(child_session_id="child-1")
assert sockets[0].holds()
# tool first: the bridge drops the socket, and a tool's end opens it again
# before the worker sees the drop (it waits on the lock the hook holds)
with activity._condition:
    sockets[0].close()
    end(1)
again = opened(2)
assert (again.ends(), again.holds()) == (["call_1"], True), again.said
assert sockets[0].ends() == ["call_0"], "a tool's end written once"
# worker first: the bridge drops it again; the worker finds it gone and
# opens it again, and a tool's end then goes on that one
again.close()
worker = opened(3)
assert worker.holds() and worker.ends() == [], worker.said
end(2)
assert (worker.ends(), worker.holds()) == (["call_2"], True), worker.said
# the child stops; dropped then, a tool's end opens a socket holding nothing
activity.child_stopped(child_session_id="child-1")
assert not worker.holds(), worker.said
worker.close()
end(3)
last = opened(4)
assert (last.ends(), last.holds()) == (["call_3"], False), last.said
assert len(sockets) == 4, [s.said for s in sockets]
# a response's calls: one frame each, before any runs, with its response
# (named once: this gateway's life and a count) and its place there
def asked(turn_id, *calls):
    tool_calls = [{"id": i, "function": {"name": "terminal", "arguments": json.dumps({"command": c})}} for i, c in calls]
    activity.tools_asked(turn_id=turn_id, assistant_message={"tool_calls": tool_calls})


def intents():
    return [(f["turn"], f["response"], f["call"]["position"], f["call"]["tool_call_id"], f["call"]["args"]) for f in last.said if "call" in f]


def ends_of():
    return [(f["tool_call_id"], f.get("response")) for f in last.said if "tool_call_id" in f]


asked("turn-1", ("call_0", "ls"))
end(4)
# the next response reuses `call_0`, with another command and the same
# (one id across its batch, as some providers say it)
asked("turn-1", ("call_0", "make deploy"), ("call_0", "ls"))
end(5)
asked("turn-x", ("call_0", "not the person's"))
said = intents()
assert [i[0] for i in said] == ["m-1"] * 3, said
r1, r2 = said[0][1], said[1][1]
assert r1 != r2 and said[2][1] == r2 and all(r.startswith(activity._LIFE + "-") for r in (r1, r2)), said
assert [i[2:] for i in said] == [(0, "call_0", "ls"), (0, "call_0", "make deploy"), (1, "call_0", "ls")], said
assert ends_of() == [("call_3", None), ("call_4", r1), ("call_5", r2)], last.said
