"""Hermes v0.21.7 keys a tool's interrupt by thread id, and a new thread
usually has the id of the last one to end. Its tool workers clear their
bit only as they end, so a bit set on a thread after it ended is the next
worker's own: that worker's command stops at once, "[Command interrupted]".
It happens when a turn starts while a background memory review runs: the
turn cancels the review by its thread's id, and the review can end first.

Each tool worker clears its thread's bit as it starts, before it is
registered for interrupts, so nothing real can have set it yet.
Delete when upstream's `_registered_tool_worker` starts its worker clean.
"""

import contextlib

from agent import tool_executor
from tools.interrupt import set_interrupt


def install():
    registered = tool_executor._registered_tool_worker

    @contextlib.contextmanager
    def started_clean(agent):
        set_interrupt(False)
        with registered(agent) as tid:
            yield tid

    tool_executor._registered_tool_worker = started_clean
