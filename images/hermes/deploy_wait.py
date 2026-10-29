"""Wait for Fragment's signing preflight, inside one Hermes stream attempt.

Only the private model intercept can prove that a refused request never
reached a provider. Do not turn an ordinary 503, a lost response, or SSE
silence into a replay. Delete when Hermes supports this preflight contract.
"""

import time
from urllib.parse import urlsplit

DEPLOY_WAIT_S = 360
RETRY_S = 2


class DeploymentUnavailable(RuntimeError):
    """A deployment exceeded its own budget, not a provider failure."""


def refused(error):
    response = getattr(error, 'response', None)
    if response is None or response.status_code != 503:
        return False
    headers = response.headers
    release = headers.get('x-fragment-release-refused', '')
    if (headers.get('x-fragment-model-not-dispatched') != 'deploying'
            or not 1 <= len(release) <= 64
            or any(not (c.isascii() and (c.isalnum() or c in '._-')) for c in release)):
        return False
    try:
        return response.json().get('error') == 'deploying'
    except (ValueError, AttributeError):
        return False


def active(call, attempt):
    if (call.agent._interrupt_requested or call._request_cancelled['value']
            or call._call_done.is_set()
            or not call._stream_attempt_is_active(attempt)
            or not call._writer_still_current('Fragment deployment')
            or call._route_switched_under_request()):
        # The worker's normal error path recognizes cancellation and must
        # not retry, probe non-streaming, or fall back with this payload.
        call._request_cancelled['value'] = True
        call._monitor_interrupted['yes'] = True
        raise InterruptedError('Fragment model request cancelled while updating')


def open_stream(call, kwargs, original):
    base = urlsplit(call.agent.base_url or '')
    if base.scheme != 'http' or base.hostname != 'model.fragment.internal':
        return original(call, kwargs)
    attempt = call.stream_attempt_state['current']
    # One budget across consecutive confirmed preflight refusals, never
    # a new six minutes per ask. A provider answer/error leaves this phase.
    for _ in range(DEPLOY_WAIT_S // RETRY_S + 1):
        active(call, attempt)
        deadline = getattr(call, '_fragment_deploy_deadline', None)
        if deadline is not None and time.monotonic() >= deadline:
            raise DeploymentUnavailable('Fragment is still updating after six minutes. Please try again.')
        try:
            # Hermes resets last_chunk_time immediately before create():
            # this unconfirmed HTTP request retains its normal stale timer.
            # Once admitted, provider silence therefore still times out.
            stream = original(call, kwargs)
            call._fragment_deploy_deadline = None
            return stream
        except Exception as error:
            if not refused(error):
                call._fragment_deploy_deadline = None
                raise
            active(call, attempt)
            if deadline is None:
                deadline = call._fragment_deploy_deadline = time.monotonic() + DEPLOY_WAIT_S
                # Claim the writer before waiting, so a newer stream can
                # supersede this attempt even before any headers arrive.
                from agent.stream_single_writer import claim_stream_writer
                call._writer_token = claim_stream_writer(call.agent)
            error.response.close()
            call.clients.close_once('fragment_preflight_refused')
            call._fragment_deploy_backoff = True
            try:
                end = min(deadline, time.monotonic() + RETRY_S)
                # Interruptible sleep: cancellation stops the next request,
                # rather than leaving a long Worker wait to dispatch later.
                while time.monotonic() < end:
                    active(call, attempt)
                    call._call_done.wait(min(0.1, end - time.monotonic()))
            finally:
                call.last_chunk_time['t'] = time.time()
                call._fragment_deploy_backoff = False
    raise DeploymentUnavailable('Fragment is still updating. Please try again.')


def install():
    from agent.chat_completion_helpers import _StreamingCall
    if getattr(_StreamingCall, '_fragment_deploy_adapter', False):
        return
    original = _StreamingCall._open_chat_stream
    kill = _StreamingCall._kill_stale_stream
    heartbeat = _StreamingCall._heartbeat

    def waiting(call):
        return (getattr(call, '_fragment_deploy_backoff', False)
                and time.monotonic() < call._fragment_deploy_deadline)

    def kill_stale(call, elapsed):
        if not waiting(call):
            return kill(call, elapsed)

    def beat(call, seconds):
        if waiting(call):
            call.agent._touch_activity('waiting for Fragment deployment')
        else:
            return heartbeat(call, seconds)

    _StreamingCall._open_chat_stream = lambda call, kwargs: open_stream(call, kwargs, original)
    _StreamingCall._kill_stale_stream = kill_stale
    _StreamingCall._heartbeat = beat
    _StreamingCall._fragment_deploy_adapter = True
