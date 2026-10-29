"""Run in the pinned image: contract, cancellation, fence and budget cases."""
import sys
sys.path.insert(0, '/opt/fragment')
import os
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import httpx
import deploy_wait as adapter


class Clock:
    now = 0.0
    after_wait = None

    def time(self):
        return self.now

    monotonic = time

    def wait(self, seconds):
        self.now += seconds
        if self.after_wait:
            self.after_wait()

    def is_set(self):
        return False


def refusal(status=503, code='deploying', marker='deploying', release='old-release'):
    return SimpleNamespace(response=httpx.Response(status, json={'error': code}, headers={
        'x-fragment-model-not-dispatched': marker,
        'x-fragment-release-refused': release,
    }))


class Refused(Exception):
    def __init__(self):
        self.response = refusal().response


def call(clock):
    return SimpleNamespace(
        agent=SimpleNamespace(base_url='http://model.fragment.internal/v1',
                              _interrupt_requested=False, _claim_stream_writer=lambda: 1),
        stream_attempt_state={'current': 1}, _request_cancelled={'value': False},
        _monitor_interrupted={'yes': False}, _call_done=clock,
        _stream_attempt_is_active=lambda attempt: attempt == 1,
        _writer_still_current=lambda label: True,
        _route_switched_under_request=lambda: False,
        clients=SimpleNamespace(close_once=lambda reason: None), last_chunk_time={'t': 0},
    )


assert adapter.refused(refusal())
for r in [refusal(status=502), refusal(code='upstream_failed'), refusal(marker=''),
          refusal(release=''), refusal(release='x' * 65), refusal(release='old release')]:
    assert not adapter.refused(r)
assert not adapter.refused(ConnectionError('response lost'))

clock = Clock()
c = call(clock)
tries = []
def opens(call, kwargs):
    tries.append(clock.now)
    if len(tries) < 3:
        raise Refused()
    return 'one provider call'
with patch.object(adapter, 'time', clock):
    assert adapter.open_stream(c, {}, opens) == 'one provider call'
assert len(tries) == 3 and clock.now >= 4
assert c._fragment_deploy_deadline is None  # provider retries have their normal budget

for why in ['interrupt', 'cancel', 'attempt', 'writer', 'route']:
    clock = Clock()
    c = call(clock)
    tries = []
    def cancel():
        if why == 'interrupt': c.agent._interrupt_requested = True
        if why == 'cancel': c._request_cancelled['value'] = True
        if why == 'attempt': c._stream_attempt_is_active = lambda attempt: False
        if why == 'writer': c._writer_still_current = lambda label: False
        if why == 'route': c._route_switched_under_request = lambda: True
    clock.after_wait = cancel
    with patch.object(adapter, 'time', clock):
        try:
            adapter.open_stream(c, {}, opens)
            raise AssertionError('cancelled wait dispatched again: ' + why)
        except InterruptedError:
            pass
    assert len(tries) == 1 and c._monitor_interrupted['yes'], why

clock = Clock()
c = call(clock)
tries = []
def always_refused(call, kwargs):
    tries.append(clock.now)
    raise Refused()
with patch.object(adapter, 'time', clock), patch.object(adapter, 'DEPLOY_WAIT_S', 6):
    try:
        adapter.open_stream(c, {}, always_refused)
        raise AssertionError('deployment budget never ended')
    except adapter.DeploymentUnavailable:
        pass
assert 6 <= clock.now < 6.1 and len(tries) == 3, (clock.now, tries)

# Ambiguous errors escape after one call. The adapter adds no retry for
# transport failure, ordinary 503 or a similarly marked public provider.
unmarked = Refused()
del unmarked.response.headers['x-fragment-model-not-dispatched']
for error in [ConnectionError('lost'), RuntimeError('provider failed'), unmarked]:
    c = call(Clock())
    c._fragment_deploy_deadline = adapter.time.monotonic() + 100
    tries = []
    def broken(call, kwargs):
        tries.append(1)
        raise error
    try:
        adapter.open_stream(c, {}, broken)
    except type(error) as caught:
        assert caught is error
    assert len(tries) == 1
    assert c._fragment_deploy_deadline is None
c = call(Clock())
c.agent.base_url = 'https://provider.example/v1'
tries = []
try:
    adapter.open_stream(c, {}, always_refused)
except Refused:
    pass
assert len(tries) == 1

adapter.install()
from agent.chat_completion_helpers import _StreamingCall
installed = _StreamingCall._open_chat_stream
adapter.install()
assert installed is _StreamingCall._open_chat_stream

# The supported plugin classification must make expiry terminal in the
# real pinned classifier, without changing ordinary provider errors.
with tempfile.TemporaryDirectory() as home:
    os.environ['HERMES_HOME'] = home
    Path(home, 'config.yaml').write_text('plugins:\n  enabled: [fragment-deploy-wait]\n')
    from hermes_cli.plugins import discover_plugins
    discover_plugins()
    from agent.error_classifier import classify_api_error
    from agent.turn_api_error import _is_local_validation_error
    error = adapter.DeploymentUnavailable('Fragment is still updating. Please try again.')
    classified = classify_api_error(error, provider='custom')
    assert not classified.retryable and not classified.should_fallback, classified
    assert not _is_local_validation_error(error)  # no unconditional local-error fallback
    ordinary = classify_api_error(RuntimeError('provider failed'), provider='custom')
    assert ordinary.retryable
print('Marked preflight only; bounded budget; cancellation and writer/attempt/route fences; no ambiguous replay.')
