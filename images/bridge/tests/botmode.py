"""Keeper host tests: Hermes imports stubbed, journal and mailbox at the seam."""
import importlib.util
import sys
import threading
import types
import unittest
from pathlib import Path
from unittest.mock import patch

for name in ('hermes_state', 'hermes_cli', 'hermes_cli.active_sessions', 'tools', 'tools.bot_failure_reasons', 'tools.bot_live_delivery'):
    sys.modules[name] = types.ModuleType(name)
for module, names in {
    'hermes_state': ['SessionDB'],
    'hermes_cli.active_sessions': ['release_active_session', 'try_acquire_active_session'],
    'tools.bot_failure_reasons': ['classify_agent_error'],
    'tools.bot_live_delivery': ['complete_delivery', 'find_canonical_live_owner', 'has_mailbox'],
}.items():
    for name in names:
        setattr(sys.modules[module], name, lambda *a, **k: None)
spec = importlib.util.spec_from_file_location('botmode', Path(__file__).parents[2] / 'hermes/botmode.py')
b = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b)

BOT = dict(agent='maple', identity='npub-maple', chat='maple-chat')
SENDER = dict(agent='juniper', identity='npub-juniper', chat='juniper-chat')

class Recovery(unittest.TestCase):
    def test_replay_reads_original_turn_not_later_turn(self):
        def records(chat, channel, cursor, sender):
            if channel == 'chat':
                return [dict(principal='npub-maple', body=dict(turn='original', text='answer'))], 20
            return [dict(principal='npub-maple', body=x) for x in [
                dict(kind='turn.start', agent='npub-maple', turn='other', cause=dict(channel='chat', seq=9)),
                dict(kind='turn.start', agent='npub-maple', turn='original', cause=dict(channel='chat', seq=7)),
                dict(kind='turn.end', turn='original', outcome='idle'),
            ]], 30000
        with patch.object(b, 'call', return_value=dict(record=dict(seq=7), replayed=True)) as call, patch.object(b, 'bodies_after', side_effect=records) as read:
            self.assertEqual(b.hand_off(BOT, SENDER, 'hello', 'dm-id'), ('answer', None))
            self.assertEqual(read.call_args_list[0].args[2], 0)
            self.assertEqual(call.call_args.args[3]['id'], 'dm-id')

    def test_answer_beyond_first_batch_of_chat_pages(self):
        def records(chat, channel, cursor, sender):
            if channel == 'work':
                return [dict(principal='npub-maple', body=x) for x in [
                    dict(kind='turn.start', agent='npub-maple', turn='original', cause=dict(channel='chat', seq=7)),
                    dict(kind='turn.end', turn='original', outcome='idle'),
                ]], 20
            if cursor == 7:
                return [dict(principal='someone-else', body=dict(text='unrelated'))], 10007
            if cursor == 10007:
                return [dict(principal='npub-maple', body=dict(turn='original', text='late recorded answer'))], 10008
            return [], cursor
        with patch.object(b, 'call', return_value=dict(record=dict(seq=7), replayed=True)), patch.object(b, 'bodies_after', side_effect=records):
            self.assertEqual(b.hand_off(BOT, SENDER, 'hello', 'dm-id'), ('late recorded answer', None))

    def test_unknown_http_outcome_leaves_claim_unsettled(self):
        owner = types.SimpleNamespace(home='home', in_flight={'abc'}, lock=threading.Lock())
        with patch.object(b, 'hand_off', side_effect=OSError('lost response')), patch.object(b, 'complete_delivery') as complete:
            b.deliver(BOT, SENDER, owner, dict(delivery_id='abc', message='hello'))
            complete.assert_not_called()
            self.assertFalse(owner.in_flight)

    def test_notification_retries_same_id_and_body_after_append(self):
        receipt = dict(delivery_id='abc', status='settled', reply='recorded answer')
        owner = types.SimpleNamespace(home='home')
        calls = []
        def sent(*args):
            calls.append(args)
            raise OSError('response lost after append')
        with patch.object(b, 'held', return_value=False), patch.object(b, 'call', side_effect=sent):
            for _ in range(2):
                with self.assertRaises(OSError):
                    b.notify(BOT, SENDER, owner, receipt)
        self.assertEqual(calls[0], calls[1])
        self.assertEqual(calls[0][3]['id'], 'dm-result-abc')
        self.assertEqual(calls[0][3]['body']['to'], ['npub-juniper'])

    def test_old_claims_and_queued_tickets_but_no_foreign_or_notified(self):
        import contextlib
        import tempfile
        import json
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = dict(created_at=1, owner=dict(live_session_id='fragment-bot-mode:maple'))
            for key, status, extra in [('old', 'claimed', {}), ('queue', 'queued', {}),
                                       ('done', 'settled', {}), ('notified', 'settled', dict(fragment_notified=True)),
                                       ('foreign', 'claimed', dict(owner=dict(live_session_id='desktop')))]:
                (root / (key + '.json')).write_text(json.dumps(dict(base, delivery_id=key, status=status, **extra)))
            @contextlib.contextmanager
            def locked(home):
                yield root
            with patch.object(b.mailbox, '_locked', locked, create=True), patch.object(b.mailbox, '_scan_read', lambda p: json.loads(p.read_text()), create=True):
                owner = types.SimpleNamespace(home=directory, live='fragment-bot-mode:maple')
                tickets = b.tickets(owner)
            self.assertEqual({r['delivery_id'] for r in tickets}, {'old', 'queue', 'done'})
            self.assertEqual(json.loads((root / 'queue.json').read_text())['status'], 'queued')

    def test_timeout_is_retryable(self):
        with patch.object(b, 'ANSWER_WAIT_S', 0), patch.object(b, 'call', return_value=dict(record=dict(seq=7))), patch.object(b, 'bodies_after', return_value=([], 0)):
            with self.assertRaises(TimeoutError):
                b.hand_off(BOT, SENDER, 'hello', 'dm-id')

if __name__ == '__main__':
    unittest.main()
