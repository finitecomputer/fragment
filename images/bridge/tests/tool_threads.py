"""Run in the pinned image: a tool worker never takes an ended thread's interrupt."""
import sys
sys.path.insert(0, '/opt/fragment')
import threading
from types import SimpleNamespace

from agent import tool_executor
from agent.interrupt_control import _ic_signal_tool_workers
from tools.daemon_pool import DaemonThreadPoolExecutor
from tools.interrupt import is_interrupted, set_interrupt

agent = SimpleNamespace(_tool_worker_threads=set(), _tool_worker_threads_lock=threading.Lock())
RUNS = 50


def ended_then_worker(inside):
    """A thread ends and an interrupt names it after (a review cancelled as
    it ends); then a tool worker runs `inside` on a fresh thread, as
    Hermes' sequential tool path does. Its id and what `inside` saw."""
    ended = threading.Thread(target=lambda: None)
    ended.start()
    ended.join()
    set_interrupt(True, ended.ident)

    def work():
        with tool_executor._registered_tool_worker(agent) as tid:
            return tid == ended.ident, inside()

    executor = DaemonThreadPoolExecutor(max_workers=1)
    try:
        return executor.submit(work).result(timeout=10)
    finally:
        executor.shutdown(wait=True)
        set_interrupt(False, ended.ident)


def interrupted_by_its_agent():
    """A real interrupt, fanned out to the registered worker by its agent."""
    _ic_signal_tool_workers(agent, True)
    try:
        return is_interrupted()
    finally:
        _ic_signal_tool_workers(agent, False)


# The pinned Hermes: a worker on the ended thread's id reads its interrupt
# as its own. Fails once upstream starts a worker clean: delete the adapter.
before = [ended_then_worker(is_interrupted) for _ in range(RUNS)]
recycled = [seen for same, seen in before if same]
assert recycled, f'no worker had an ended thread\'s id in {RUNS} runs'
assert all(recycled), f'upstream starts a worker clean now: {before}'

import tool_threads
tool_threads.install()

after = [ended_then_worker(is_interrupted) for _ in range(RUNS)]
assert any(same for same, _ in after), f'no worker had an ended thread\'s id in {RUNS} runs'
assert not any(seen for _, seen in after), f'a worker took an ended thread\'s interrupt: {after}'

# Still stopped by its own agent's interrupt.
assert all(seen for _, seen in (ended_then_worker(interrupted_by_its_agent) for _ in range(RUNS)))
assert not agent._tool_worker_threads, agent._tool_worker_threads
print(f'recycled ids: {len(recycled)}/{RUNS} interrupted before, none after')
