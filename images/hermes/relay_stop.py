"""Hermes v0.23.2's Relay Stop (a connector's `interrupt_inbound`) only sets
the adapter's busy-guard event and stops typing. A relayed message has been
admitted natively since v0.22 (the gateway's session authority), so nothing
watches that event for its turn: a person's Stop left the turn's tools
running to their end, and its next calls starting.

The Stop reaches the running turn as Hermes' own /stop does, through its
`_interrupt_running_turn`: a hard interrupt of the turn's agent, its run
generation invalidated, the tool processes it started reaped. Nothing
happens to a chat with no agent running.
Delete when upstream's `RelayAdapter.on_interrupt` interrupts a natively
admitted turn (the Stop in `a_step_says_how_its_call_ended` and in
`hermes_waits_out_model_preflight_deploying` pass without it).
"""

from gateway.relay import adapter as relay


def install():
    original = relay.RelayAdapter.on_interrupt

    async def on_interrupt(self, session_key, chat_id):
        runner = getattr(self._message_handler, '__self__', None)
        if runner is not None and session_key:
            from gateway.run import _AGENT_PENDING_SENTINEL, _INTERRUPT_REASON_STOP
            state = runner._peek_session_state(session_key)
            agent = state.turn.agent if state else None
            if agent is not None and agent is not _AGENT_PENDING_SENTINEL:
                runner._interrupt_running_turn(
                    session_key, interrupt_reason=_INTERRUPT_REASON_STOP, invalidation_reason='relay_interrupt')
        await original(self, session_key, chat_id)

    relay.RelayAdapter.on_interrupt = on_interrupt
