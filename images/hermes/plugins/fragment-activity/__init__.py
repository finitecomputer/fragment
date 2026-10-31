"""Hermes owns the children; one socket carries their work, and each tool
call's intent and end.

The supported lifecycle hooks identify each child by its session id. Nothing
is saved: a gateway exit closes the socket, and a new process has no children.
The signal's contract is in docs/bridge.md, Runtime work outside a turn.
"""
import json
import logging
import os
import threading

from gateway.relay.auth import make_token
from websockets.sync.client import connect

_log = logging.getLogger(__name__)
_condition = threading.Condition()
# Bounded by Hermes' configured delegation concurrency, across its profiles.
_children = set()
_socket = None
_worker = None
# The person's turns this gateway runs, by Hermes' turn id: each our turn's
# id (its Relay message's) and its latest model response that asked for
# calls, whose calls run until the next. Bounded: the least recently busy
# go (a turn's tool call makes it the newest, however long it runs).
_turns = {}
_TURNS_MAX = 64
# Each model response that asks for calls is named once: this gateway's life
# and its count (`<life>-<n>`). A call's intent is its response's and place
# there, so a provider's id a later response says again (some name every
# response's first call `call_0`) is another call; a call's end names its
# response, so it ends only that response's call.
_LIFE = os.urandom(6).hex()
_responses = 0
# Hermes' tool outcomes, as a step's (docs/chat-records.md).
_STATUS = {"ok": "completed", "error": "failed", "timeout": "timed_out", "cancelled": "cancelled", "blocked": "blocked"}


def register(ctx):
    ctx.register_hook("subagent_start", child_started)
    ctx.register_hook("subagent_stop", child_stopped)
    ctx.register_hook("pre_llm_call", turn_started)
    ctx.register_hook("post_api_request", tools_asked)
    ctx.register_hook("post_tool_call", tool_finished)


def _connect():
    """Called under the condition: a socket the bridge acknowledged, which
    holds while children run. A new socket holds nothing until it says so,
    so it does, whichever hook opens it (never replaying a tool's end)."""
    url = os.environ["GATEWAY_RELAY_URL"].rstrip("/")
    url = url.replace("https://", "wss://", 1).replace("http://", "ws://", 1)
    token = make_token(os.environ["GATEWAY_RELAY_ID"], os.environ["GATEWAY_RELAY_SECRET"], ttl_seconds=60)
    socket = connect(url + "/relay/activity", additional_headers={"Authorization": "Bearer " + token},
                     open_timeout=2, close_timeout=1, ping_interval=10, ping_timeout=10, proxy=None)
    try:
        if socket.recv(timeout=2) != "ready":
            raise ValueError("the bridge did not acknowledge runtime activity")
        if _children:
            socket.send(json.dumps({"working": True}))
            if socket.recv(timeout=2) != "working":
                raise ValueError("the bridge did not hold for the children")
    except Exception:
        socket.close()
        raise
    return socket


def _send(frame):
    """Called under the condition: one frame, and the bridge's answer to it
    (None when it has none). A socket found closed (the bridge's idle bound,
    or its restart) is opened again once; a frame written is never resent."""
    global _socket
    for _ in range(2):
        if _socket is None:
            try:
                _socket = _connect()
            except Exception as error:
                _log.warning("Runtime activity connect failed (%s)", type(error).__name__)
                return None
        try:
            _socket.send(json.dumps(frame))
        except Exception:
            _socket.close()
            _socket = None
            continue
        try:
            return _socket.recv(timeout=2)
        except Exception as error:
            _socket.close()
            _socket = None
            _log.warning("Runtime activity unanswered (%s)", type(error).__name__)
            return None
    return None


def child_started(*, child_session_id=None, **kwargs):
    global _worker
    if not isinstance(child_session_id, str) or not child_session_id:
        raise ValueError("a delegated child has a session id")
    with _condition:
        _children.add(child_session_id)
        # Acknowledged before the parent can finish.
        _send({"working": True})
        if _worker is None:
            _worker = threading.Thread(target=_hold, name="fragment-activity", daemon=True)
            _worker.start()
        _condition.notify_all()


def child_stopped(*, child_session_id=None, **kwargs):
    with _condition:
        _children.discard(child_session_id)
        if _socket is not None:
            _send({"working": bool(_children)})
        _condition.notify_all()


def _hold():
    global _socket
    # Bounded by the gateway's life. With no children the worker sleeps;
    # a disconnected bridge is retried once a second while children run.
    while True:
        with _condition:
            _condition.wait_for(lambda: bool(_children))
            if _socket is None:
                _send({"working": True})
            socket = _socket
            if socket is None:
                _condition.wait(timeout=1)
                continue
        try:
            if not socket.ping().wait(timeout=2):
                raise ConnectionError("activity ping timed out")
            with _condition:
                _condition.wait(timeout=1)
        except TimeoutError:
            continue
        except Exception as error:
            with _condition:
                if _socket is socket:
                    _socket = None
                    _log.warning("Runtime activity disconnected (%s); retrying", type(error).__name__)
            socket.close()


def turn_started(*, turn_id=None, **kwargs):
    # The run Hermes starts for a person's Relay message. Its memory review's
    # fork starts none (it persists nothing, so Hermes skips this hook), and
    # a delegated child's run is its own: their tool calls are no steps.
    from agent.delegation_context import is_delegated_child_context
    from gateway.session_context import get_session_env
    turn = get_session_env("HERMES_SESSION_MESSAGE_ID", "")
    if not turn_id or not turn or is_delegated_child_context() or get_session_env("HERMES_SESSION_PLATFORM", "") != "relay":
        return
    with _condition:
        _turns[turn_id] = [turn, None]
        while len(_turns) > _TURNS_MAX:
            del _turns[next(iter(_turns))]


def _field(obj, name):
    """An OpenAI-shaped message's field, whether Hermes hands an object or a dict."""
    return obj.get(name) if isinstance(obj, dict) else getattr(obj, name, None)


# The calls one response may say, at most (a model's parallel calls).
_CALLS_MAX = 32


def tools_asked(*, turn_id=None, assistant_message=None, **kwargs):
    # Each tool call of a model's response, said before Hermes runs any of
    # them: Hermes fires post_api_request in normalize_model_response, before
    # run_tool_round (agent/conversation_loop.py, v0.21.7 and v0.22.5), and
    # runs them all before it asks the model again. Its intent: asked, which
    # a refusal may still stop; only its end says it ran. A turn not the
    # person's (the memory review's, a delegated child's) is none of ours, as
    # for its end.
    global _responses
    calls = (_field(assistant_message, "tool_calls") if assistant_message is not None else None) or []
    with _condition:
        mine = _turns.get(turn_id)
        if mine is None or not calls:
            return
        _responses += 1
        mine[1] = response = f"{_LIFE}-{_responses}"
        turn = mine[0]
    from agent.display import build_tool_preview
    for position, call in enumerate(list(calls)[:_CALLS_MAX]):
        call_id, function = _field(call, "id"), _field(call, "function")
        name = _field(function, "name") if function is not None else None
        if not isinstance(call_id, str) or not call_id or not isinstance(name, str) or not name:
            continue
        raw = _field(function, "arguments")
        try:
            args = json.loads(raw) if isinstance(raw, str) and len(raw) <= 65536 else (raw if isinstance(raw, dict) else {})
        except ValueError:
            args = {}
        try:
            preview = build_tool_preview(name, args if isinstance(args, dict) else {}, max_len=140) or ""
        except Exception:
            preview = ""
        frame = {"turn": turn, "response": response,
                 "call": {"position": position, "tool_call_id": call_id[:128], "tool": name[:140], "args": preview}}
        with _condition:
            # Answered before Hermes runs the call; unanswered, it runs anyway
            # (an observer never holds the agent back).
            if _send(frame) != "called":
                _log.warning("A tool call's intent was not recorded")


def tool_finished(*, tool_name=None, args=None, tool_call_id=None, turn_id=None,
                  duration_ms=0, status=None, result=None, **kwargs):
    with _condition:
        mine = _turns.pop(turn_id, None)
        if mine is not None:
            _turns[turn_id] = mine
        turn, response = mine if mine is not None else (None, None)
    outcome = _STATUS.get(status)
    if turn is None or not tool_call_id or outcome is None:
        return  # another run's call, or no outcome: never a guessed step
    # What a person saw of a call before: Hermes' own preview of it (its
    # display helper, which masks what it knows to), and a command's exit
    # code; never its output or error text.
    from agent.display import build_tool_preview
    try:
        preview = build_tool_preview(str(tool_name), args if isinstance(args, dict) else {}, max_len=140) or ""
    except Exception:
        preview = ""
    parsed = None
    if isinstance(result, str) and len(result) <= 65536:
        try:
            parsed = json.loads(result)
        except ValueError:
            pass
    exit_code = parsed.get("exit_code") if isinstance(parsed, dict) else None
    frame = {"turn": turn, "tool": str(tool_name)[:140], "args": preview,
             "excerpt": "Exit code: " + str(exit_code) if type(exit_code) is int else "",
             "tool_call_id": str(tool_call_id)[:128], "status": outcome, "duration_ms": max(0, int(duration_ms or 0))}
    if response is not None:
        frame["response"] = response
    with _condition:
        # Answered before Hermes can finish its turn.
        if _send(frame) != "observed":
            _log.warning("A tool step was not recorded")
