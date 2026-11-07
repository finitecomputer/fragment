"""The activity plugin says each child's start and end beside its work
(FIN-432): a child of a person's turn of ours starts with that turn and its
task in one line, and ends with Hermes' outcome as ours, each on its own (as
`delegation.independent_completions` stops each child as it finishes); a
child of no turn of ours holds the computer and is said nowhere. A child's
end is said even when the bridge dropped the socket, on one opened again.
Method: the plugin, its connection a stub of the bridge's, answering as the
bridge does."""
import importlib.util
import json
import os
import threading

os.environ.update(GATEWAY_RELAY_URL="http://127.0.0.1:1", GATEWAY_RELAY_ID="test", GATEWAY_RELAY_SECRET="0" * 32)
spec = importlib.util.spec_from_file_location("fragment_activity", "/opt/hermes/plugins/fragment-activity/__init__.py")
activity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(activity)
sockets = []


class Socket:
    def __init__(self, *args, **kwargs):
        self.said, self.answers, self.closed = [], ["ready"], False
        sockets.append(self)

    def send(self, text):
        if self.closed:
            raise ConnectionError("closed by the bridge")
        frame = json.loads(text)
        self.said.append(frame)
        self.answers.append("working")

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


def frames():
    return [f for s in sockets for f in s.said]


activity.connect = Socket
# the person's turn: Hermes' turn id, ours (its Relay message's)
activity._turns["hermes-turn-1"] = ["m-1", None]
goal = "Find three venues\n  near Lisbon " + "x" * 400
activity.child_started(child_session_id="s-1", parent_turn_id="hermes-turn-1", child_goal=goal, child_role="leaf")
activity.child_started(child_session_id="s-2", parent_turn_id="hermes-turn-1", child_goal="Compare the quotes")
# a nested child's turn is none of ours; one with no turn (a background
# child's later turn says "") neither
activity.child_started(child_session_id="s-3", parent_turn_id="not-ours")
activity.child_started(child_session_id="s-4", parent_turn_id="")
said = frames()
assert said.pop(0) == {"working": True}, "a new socket says its work first"
assert said[0]["turn"] == "m-1" and said[0]["child"] == "s-1" and said[0]["working"] is True, said[0]
assert said[0]["goal"].startswith("Find three venues near Lisbon x") and len(said[0]["goal"]) == activity._GOAL_MAX, said[0]
assert said[1] == {"working": True, "turn": "m-1", "child": "s-2", "goal": "Compare the quotes"}, said[1]
assert said[2:] == [{"working": True}, {"working": True}], "no turn of ours: work alone"
# each ends on its own, the rest still working; Hermes' outcome as ours
activity.child_stopped(child_session_id="s-2", child_status="timeout", parent_turn_id="a later turn")
assert frames()[-1] == {"working": True, "child": "s-2", "status": "timed_out"}, frames()[-1]
activity.child_stopped(child_session_id="s-3", child_status="completed")
assert frames()[-1] == {"working": True}, "a child said nowhere ends nowhere"
# the bridge drops the socket; the next end opens it again, and is said
# (before the hold worker sees the drop: it waits on the lock the hook holds)
with activity._condition:
    sockets[-1].close()
    activity.child_stopped(child_session_id="s-1", child_status="error")
assert len(sockets) == 2, len(sockets)
assert sockets[1].said == [{"working": True}, {"working": True, "child": "s-1", "status": "failed"}], sockets[1].said
activity.child_stopped(child_session_id="s-4", child_status="interrupted")
assert frames()[-1] == {"working": False}, frames()[-1]
# a stop said twice, and an outcome Hermes may add later
activity.child_stopped(child_session_id="s-4", child_status="completed")
assert frames()[-1] == {"working": False}
activity._turns["hermes-turn-2"] = ["m-2", None]
activity.child_started(child_session_id="s-5", parent_turn_id="hermes-turn-2", child_goal="")
activity.child_stopped(child_session_id="s-5", child_status="something new")
assert frames()[-2:] == [{"working": True, "turn": "m-2", "child": "s-5", "goal": ""}, {"working": False, "child": "s-5", "status": "failed"}], frames()[-2:]
for status, ours in [("completed", "completed"), ("failed", "failed"), ("interrupted", "interrupted")]:
    activity.child_started(child_session_id="s-" + status, parent_turn_id="hermes-turn-2", child_goal="g")
    activity.child_stopped(child_session_id="s-" + status, child_status=status)
    assert frames()[-1]["status"] == ours
assert not activity._children
