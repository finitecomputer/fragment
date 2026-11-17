"""Hermes v0.23.2's Relay Stop (a connector's `interrupt_inbound`) only sets
the adapter's busy-guard event and stops typing. A relayed message has been
admitted natively since v0.22 (the gateway's session authority), so nothing
watches that event for its turn: a person's Stop left the turn's tools
running to their end, and its next calls starting.

The Stop reaches the turn through the session authority that admitted it,
as Hermes' own controls stop a native turn (`SessionAuthority.interrupt` on
the running claim): its agent gets the hard interrupt /stop requests, or,
when the turn has not adopted its agent yet (its first model call can run
before), the Stop is latched for the agent that turn adopts. Its command is
stopped, its next calls never start, a clarify it waits on is withdrawn,
and the turn ends through its own path, which frees the chat for the next
message. Nothing else is touched: no queued message is cancelled and no run
generation bumped (bumped, the stopped run's own end is stale and frees
nothing, and the chat waits behind it). A chat with no turn running, or
none admitted natively, is left to the adapter's own Stop.
Delete when upstream's `RelayAdapter.on_interrupt` interrupts a natively
admitted turn (the Stop in `a_step_says_how_its_call_ended` and in
`hermes_waits_out_model_preflight_deploying` pass without it, and a chat
answers on after a Stop in `a_stop_lets_its_clarify_go`).
"""

import logging

from gateway.relay import adapter as relay

log = logging.getLogger('fragment.relay_stop')


async def stop_native_turn(runner, session_key):
    """Stop the running turn of the native session routed as `session_key`, in whichever
    profile's authority holds it; what was done, for the log."""
    from gateway.session_contract import SessionActor, SessionRef
    from hermes_state_runtime import RuntimeStoreError
    registry = getattr(runner, 'session_authorities', None)
    authorities = list(registry) if registry is not None else [getattr(runner, 'session_authority', None)]
    for authority in authorities:
        if authority is None:
            continue
        for sid, live in list(authority.sessions.items()):
            if getattr(live, 'route', None) != session_key:
                continue
            ref = SessionRef(authority.profile_id, sid)
            handle = authority._handle(ref)
            if handle.execution_state != 'running':
                return 'no turn running'
            actor = SessionActor('messaging-control', authority.profile_id,
                                 frozenset({'session:submit', 'session:control'}), 'relay-stop')
            try:
                await authority.interrupt(actor, ref, handle.execution_generation)
            except RuntimeStoreError as e:
                return f'its turn settled meanwhile ({e})'
            return 'stopped its turn'
    return 'no native session'


def install():
    original = relay.RelayAdapter.on_interrupt

    async def on_interrupt(self, session_key, chat_id):
        runner = getattr(self._message_handler, '__self__', None)
        if runner is not None and session_key:
            try:
                said = await stop_native_turn(runner, session_key)
            except Exception:
                log.exception('relay stop of %s failed', session_key)
            else:
                log.info('relay stop of %s: %s', session_key, said)
        await original(self, session_key, chat_id)

    relay.RelayAdapter.on_interrupt = on_interrupt
