"""The activity plugin remembers the person's turns, at most `_TURNS_MAX`
runs: a turn whose calls go on past that many newer runs keeps its steps,
and one that called nothing since goes first."""
import importlib.util

from gateway.session_context import clear_session_vars, set_session_vars

spec = importlib.util.spec_from_file_location("fragment_activity", "/opt/hermes/plugins/fragment-activity/__init__.py")
activity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(activity)
said = []
activity._send = lambda frame: said.append(frame["turn"]) or "observed"


def started(turn_id, message):
    tokens = set_session_vars(platform="relay", message_id=message)
    try:
        activity.turn_started(turn_id=turn_id)
    finally:
        clear_session_vars(tokens)


def ended(turn_id, n):
    activity.tool_finished(tool_name="terminal", args={"command": "make"}, tool_call_id=f"call_{n}", turn_id=turn_id, status="ok", result="{}")


started("long", "m-long")
started("idle", "m-idle")
newer = 3 * activity._TURNS_MAX
for n in range(newer):
    ended("long", n)
    started(f"run-{n}", f"m-{n}")
ended("long", newer)
ended("idle", newer + 1)
assert said == ["m-long"] * (newer + 1), said[-3:]
assert len(activity._turns) == activity._TURNS_MAX, len(activity._turns)
