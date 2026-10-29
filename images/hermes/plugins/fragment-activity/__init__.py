"""Hermes owns the children; one socket carries their work and tool evidence.

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


def register(ctx):
    ctx.register_hook("subagent_start", child_started)
    ctx.register_hook("subagent_stop", child_stopped)
    ctx.register_hook("pre_tool_call", tool_started)
    ctx.register_hook("post_tool_call", tool_finished)


def _connect():
    url = os.environ["GATEWAY_RELAY_URL"].rstrip("/")
    url = url.replace("https://", "wss://", 1).replace("http://", "ws://", 1)
    token = make_token(os.environ["GATEWAY_RELAY_ID"], os.environ["GATEWAY_RELAY_SECRET"], ttl_seconds=60)
    socket = connect(url + "/relay/activity?events=1", additional_headers={"Authorization": "Bearer " + token},
                     open_timeout=2, close_timeout=1, ping_interval=10, ping_timeout=10, proxy=None)
    try:
        if socket.recv(timeout=2) != "ready":
            raise ValueError("the bridge did not acknowledge runtime activity")
    except Exception:
        socket.close()
        raise
    return socket


def _open():
    """Called under the condition: never race two connects or lose a newer socket."""
    global _socket
    try:
        _socket = _connect()
        _socket.send(json.dumps({"working": bool(_children)}))
        _socket.recv(timeout=2)
    except Exception as error:
        if _socket is not None:
            _socket.close()
            _socket = None
        _log.warning("Runtime activity connect failed (%s); retrying", type(error).__name__)


def child_started(*, child_session_id=None, **kwargs):
    global _worker, _socket
    if not isinstance(child_session_id, str) or not child_session_id:
        raise ValueError("a delegated child has a session id")
    with _condition:
        _children.add(child_session_id)
        if _socket is None:
            # A successful signal is queued before the parent can finish.
            _open()
        if _socket is not None:
            try:
                _socket.send(json.dumps({"working": True}))
                _socket.recv(timeout=2)
            except Exception:
                _socket.close()
                _socket = None
        if _worker is None:
            _worker = threading.Thread(target=_hold, name="fragment-activity", daemon=True)
            _worker.start()
        _condition.notify_all()


def child_stopped(*, child_session_id=None, **kwargs):
    global _socket
    with _condition:
        _children.discard(child_session_id)
        if _socket is not None:
            try:
                _socket.send(json.dumps({"working": bool(_children)}))
                _socket.recv(timeout=2)
            except Exception:
                pass  # Reader detects disconnect; remaining children reconnect.
        _condition.notify_all()


def _hold():
    global _socket
    # Bounded by the gateway's life. With no children the worker sleeps;
    # a disconnected bridge is retried once a second while children run.
    while True:
        with _condition:
            _condition.wait_for(lambda: bool(_children) or _socket is not None)
            if _socket is None:
                _open()
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


def tool_started(**kwargs):
    # Establish structured capability before progress text can arrive. A tool
    # waiting on a person is not runtime work; only children set working.
    global _worker
    with _condition:
        if _socket is None:
            _open()
        if _worker is None:
            _worker = threading.Thread(target=_hold, name="fragment-activity", daemon=True)
            _worker.start()
        _condition.notify_all()


def tool_finished(*, tool_name=None, tool_call_id=None, session_id=None,
                  duration_ms=0, status=None, error_type=None, result=None, **kwargs):
    from gateway.session_context import get_session_env
    if get_session_env("HERMES_SESSION_PLATFORM", "") != "relay":
        return
    turn = get_session_env("HERMES_SESSION_MESSAGE_ID", "")
    mapped = {"ok": "completed", "error": "failed", "cancelled": "cancelled", "blocked": "blocked"}.get(status)
    if not turn or not tool_call_id or mapped is None:
        return  # Missing evidence never implies success.
    # Do not copy arbitrary args, output or exception messages to retained
    # chat records. Keep a source reference and safe numeric result metadata.
    parsed = result
    if isinstance(result, str) and len(result) <= 65536:
        try:
            parsed = json.loads(result)
        except ValueError:
            parsed = None
    exit_code = parsed.get("exit_code") if isinstance(parsed, dict) else None
    excerpt = "Exit code: " + str(exit_code) if type(exit_code) is int else "Hermes reported " + mapped
    safe_type = error_type if isinstance(error_type, str) and error_type.replace("_", "").isalnum() else None
    message = excerpt if mapped != "completed" else None
    frame = {"turn": turn, "tool": str(tool_name)[:140], "excerpt": excerpt[:300], "evidence": {
        "tool_call_id": str(tool_call_id)[:128], "status": mapped,
        "duration_ms": max(0, int(duration_ms)), "error_type": safe_type[:140] if safe_type else None,
        "error_message": message, "result_ref": f"hermes:{session_id}:{tool_call_id}"[:300]}}
    with _condition:
        if _socket is None:
            _open()
        if _socket is not None:
            try:
                _socket.send(json.dumps(frame))
                _socket.recv(timeout=2)  # Queued before Hermes can finish its turn.
            except Exception as error:
                _log.warning("Tool observation delivery failed (%s)", type(error).__name__)
