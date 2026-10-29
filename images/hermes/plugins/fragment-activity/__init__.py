"""Hermes owns the children; the bridge holds while their activity socket is open.

The supported lifecycle hooks identify each child by its session id. Nothing
is saved: a gateway exit closes the socket, and a new process has no children.
The signal's contract is in docs/bridge.md, Runtime work outside a turn.
"""
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


def _connect():
    url = os.environ["GATEWAY_RELAY_URL"].rstrip("/")
    url = url.replace("https://", "wss://", 1).replace("http://", "ws://", 1)
    token = make_token(os.environ["GATEWAY_RELAY_ID"], os.environ["GATEWAY_RELAY_SECRET"], ttl_seconds=60)
    socket = connect(url + "/relay/activity", additional_headers={"Authorization": "Bearer " + token},
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
    except Exception as error:
        _log.warning("Runtime activity connect failed (%s); retrying", type(error).__name__)


def child_started(*, child_session_id=None, **kwargs):
    global _worker
    if not isinstance(child_session_id, str) or not child_session_id:
        raise ValueError("a delegated child has a session id")
    with _condition:
        _children.add(child_session_id)
        if _socket is None:
            # A successful signal is queued before the parent can finish.
            _open()
        if _worker is None:
            _worker = threading.Thread(target=_hold, name="fragment-activity", daemon=True)
            _worker.start()
        _condition.notify_all()


def child_stopped(*, child_session_id=None, **kwargs):
    global _socket
    with _condition:
        _children.discard(child_session_id)
        socket = None
        if not _children:
            socket, _socket = _socket, None
        _condition.notify_all()
    if socket is not None:
        socket.close()


def _hold():
    global _socket
    # Bounded by the gateway's life. With no children the worker sleeps;
    # a disconnected bridge is retried once a second while children run.
    while True:
        with _condition:
            _condition.wait_for(lambda: bool(_children))
            if _socket is None:
                _open()
            socket = _socket
            if socket is None:
                _condition.wait(timeout=1)
                continue
        try:
            socket.recv(timeout=1)  # WebSocket pings/pongs run in its reader.
        except TimeoutError:
            continue
        except Exception as error:
            with _condition:
                if _socket is socket:
                    _socket = None
                    _log.warning("Runtime activity disconnected (%s); retrying", type(error).__name__)
            socket.close()
