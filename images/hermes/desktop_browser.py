"""Regular Chromium on Hermes v0.21.7's Bot Desktop.

Hermes' local browser sessions retain their lease fence and attach to this
browser. agent-browser owns its CDP connection, not the desktop's process.
This is runtime integration with Hermes' Python helpers, not a provisioner
or another browser daemon. See the debt ledger's deletion condition.
"""

import contextlib
import fcntl
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import time
import urllib.request

import psutil
from websockets.sync.client import connect
from websockets.exceptions import WebSocketException

# The dock executes this file from an arbitrary working directory.
if __name__ == '__main__':
    sys.path.insert(0, '/opt/hermes')

BROWSER = '/opt/fragment/bin/chromium'
START_SECONDS = 15
LOCK_SECONDS = 20
REPLY_BYTES = 64 * 1024
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


@contextlib.contextmanager
def _locked(profile):
    profile.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (profile / 'fragment-browser.lock').open('a') as lock:
        deadline = time.monotonic() + LOCK_SECONDS
        while True:  # Bounded by LOCK_SECONDS, including another process's startup.
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise RuntimeError('The desktop browser is busy starting or stopping; retry.')
                time.sleep(0.05)
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def _argument(argv, name):
    return next((arg.split('=', 1)[1] for arg in argv if arg.startswith(name + '=')), None)


def _owned_process(profile):
    """The live browser holding this exact profile; a recycled PID alone proves nothing."""
    try:
        pid = int(os.readlink(profile / 'SingletonLock').rsplit('-', 1)[1])
        process = psutil.Process(pid)
        argv = process.cmdline()
        directory = _argument(argv, '--user-data-dir')
        if (process.status() == psutil.STATUS_ZOMBIE or directory is None
                or Path(directory).resolve() != profile.resolve()
                or Path(process.exe()).name not in {'chrome', 'chromium'}
                or _argument(argv, '--type') is not None):
            return None
        return process
    except (OSError, ValueError, IndexError, psutil.Error):
        return None


def _connection(process):
    """Validate the live endpoint against the profile's process, never a stale port file."""
    port = int(_argument(process.cmdline(), '--remote-debugging-port') or '0')
    if not 0 < port < 65536:
        raise RuntimeError('Restart the desktop to use its regular Chromium browser.')
    with HTTP.open(f'http://127.0.0.1:{port}/json/version', timeout=0.5) as response:
        raw = response.read(REPLY_BYTES + 1)
    if len(raw) > REPLY_BYTES:
        raise RuntimeError('The desktop browser discovery reply is too large.')
    endpoint = json.loads(raw)['webSocketDebuggerUrl']
    prefix = f'ws://127.0.0.1:{port}/devtools/browser/'
    if not endpoint.startswith(prefix) or not re.fullmatch(r'[a-zA-Z0-9-]+', endpoint[len(prefix):]):
        raise RuntimeError('The desktop browser returned an invalid local endpoint.')
    # A port can be reused while a browser starts or crashes. CDP itself must
    # name the profile's browser PID before any page or credentials are reached.
    with connect(endpoint, open_timeout=0.5, close_timeout=0.1, max_size=REPLY_BYTES, proxy=None) as ws:
        ws.send(json.dumps({'id': 1, 'method': 'SystemInfo.getProcessInfo'}))
        for _ in range(16):
            answer = json.loads(ws.recv(timeout=0.5))
            if answer.get('id') == 1:
                processes = answer.get('result', {}).get('processInfo', [])
                if any(p.get('type') == 'browser' and p.get('id') == process.pid for p in processes):
                    return port, endpoint
                break
    raise RuntimeError('The desktop browser endpoint belongs to another process.')


def _endpoint(process):
    return _connection(process)[0]


def _stop(process):
    from tools.bot_desktop import runtime
    try:
        # Signal a normal browser shutdown before touching the process group:
        # SIGTERM of the whole group can kill the cookie writer before it
        # flushes. agent-browser does this too for browsers it owns.
        _, endpoint = _connection(process)
        with connect(endpoint, open_timeout=0.5, close_timeout=0.1, max_size=REPLY_BYTES, proxy=None) as ws:
            ws.send(json.dumps({'id': 2, 'method': 'Browser.close'}))
        process.wait(timeout=5)
        return
    except psutil.NoSuchProcess:
        return
    except (OSError, ValueError, KeyError, RuntimeError, TimeoutError, psutil.TimeoutExpired, WebSocketException):
        # An unresponsive browser still cannot outlive its desktop. Use
        # Hermes' bounded group stop only after graceful shutdown failed.
        try:
            if process.is_running() and process.status() != psutil.STATUS_ZOMBIE:
                group = process.pid if os.getpgid(process.pid) == process.pid else None
                runtime._kill_group_then_wait(group, process.pid, grace=1.0)
        except (psutil.NoSuchProcess, ProcessLookupError):
            return  # It finished its shutdown while the fallback checked it.


def _check_version(profile):
    """Refuse a future downgrade instead of asking Chromium to rewrite a newer profile."""
    saved = profile / 'Last Version'
    if not saved.exists():
        return
    with saved.open() as source:
        previous = source.read(128).strip().split('.', 1)[0]
    version = subprocess.check_output([BROWSER, '--version'], text=True, timeout=5)
    current = re.search(r'Chromium (\d+)\.', version)
    if not previous.isdigit() or current is None:
        raise RuntimeError('Cannot check the desktop browser profile version.')
    if int(previous) > int(current[1]):
        raise RuntimeError('This browser profile needs a newer Chromium image; restore the matching image or save.')


def _start(profile, env):
    from tools.bot_desktop import browser
    if not env.get('DISPLAY') or not env.get('XAUTHORITY'):
        raise RuntimeError('The agent desktop is not running; start its screen and retry.')
    with _locked(profile):
        process = _owned_process(profile)
        if process is not None:
            return _endpoint(process), False
        _check_version(profile)
        # Ask the kernel for a free nonzero loopback port. If another process
        # races this release, startup fails; PID validation prevents attaching
        # to the winner. Separate agents never pin a shared debug port.
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        args = [arg for arg in browser.dock_argv(BROWSER, str(profile), sandbox_bypass=True)
                if not arg.startswith('--remote-debugging-port=')]
        args += ['--remote-debugging-address=127.0.0.1', f'--remote-debugging-port={port}', 'about:blank']
        child = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, start_new_session=True, close_fds=True)
        deadline = time.monotonic() + START_SECONDS
        try:
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise RuntimeError(f'The desktop Chromium exited during startup ({child.returncode}).')
                process = _owned_process(profile)
                if process is not None:
                    if process.pid != child.pid:
                        raise RuntimeError('Another browser already holds this desktop profile; restart the desktop.')
                    try:
                        if _endpoint(process) == port:
                            return port, True
                    except (OSError, ValueError, KeyError, TimeoutError):
                        pass  # The browser owns its profile before CDP is ready.
                time.sleep(0.05)
            raise RuntimeError(f'The desktop Chromium did not become ready within {START_SECONDS}s.')
        except BaseException:
            # This Popen, unlike a PID read from disk, is ours even before the
            # profile lock exists. Stop its whole group on a failed launch.
            from tools.bot_desktop import runtime
            runtime._kill_group_then_wait(child.pid, child.pid, grace=5.0)
            raise


def ensure_started():
    from tools.bot_desktop import browser
    from tools.browser_tool import _build_browser_env
    port, _ = _start(browser.profile_dir(), _build_browser_env())
    return port


def stop():
    from tools.bot_desktop import browser
    profile = browser.profile_dir()
    if not profile.exists():
        return
    with _locked(profile):
        process = _owned_process(profile)
        if process is not None:
            _stop(process)


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('desktop browser: expected the profile directory')
    profile = Path(sys.argv[1])
    _, created = _start(profile, dict(os.environ))
    if not created:
        # As the original dock did, a second click opens a window in the
        # existing browser, using Chromium's own profile singleton.
        subprocess.run([BROWSER, f'--user-data-dir={profile}', '--no-first-run',
                        '--no-default-browser-check'], check=True, timeout=10)
