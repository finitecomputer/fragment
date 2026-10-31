"""fragment_ask: one of the owner's other agents asked, its answer back here.

The tool asks as `fragment ask --return-to <this chat>` does, as this agent
(docs/chat-records.md, "An ask"), and returns at once: the asked agent
answers in a chat of the two and their owner, and the bridge brings that
answer back to this chat as a new message, a turn of this agent's own,
however long it takes. Nothing here waits, keeps a mailbox or survives a
restart: the bridge's records do.

Who asks and from where is the turn's own session (its relay chat id,
`<chat>/<agent fragment>`), checked against the profile's `.env`, never
what the model says. Its post id is the tool call's, so a retry of one
call posts once and two calls of one turn post twice.
"""
import hashlib
import json
import os
import subprocess
import uuid
from pathlib import Path

CLI = '/usr/local/bin/fragment'
# An ask is a few platform calls: its roster, its chat (made the first
# time), its post.
CALL_S = 60
API = 'http://api.fragment.internal'

SCHEMA = {
    'name': 'fragment_ask',
    'description': (
        "Ask one of your teammates (your owner's other agents, named in your instructions) something. "
        "It answers in a chat of the two of you and your owner, and its answer comes back to you here "
        "as a new message once it is done. This returns at once: end your turn after asking, and don't "
        "wait for the answer, check on it, or ask again."
    ),
    'parameters': {
        'type': 'object',
        'properties': {
            'agent': {'type': 'string', 'description': 'The teammate: its name, or its fragment (as `maple--k3x9`) when two share a name.'},
            'question': {'type': 'string', 'description': 'What to ask it, whole: it sees nothing else of this chat.'},
        },
        'required': ['agent', 'question'],
    },
}


def register(ctx):
    ctx.register_tool(name='fragment_ask', toolset='fragment', schema=SCHEMA, handler=ask, description=SCHEMA['description'], emoji='💬')


def profile_env(home):
    """The profile's `.env` (hermes-boot's `profile_env`): `NAME=value` lines."""
    out = {}
    try:
        text = (home / '.env').read_text(encoding='utf-8')
    except OSError:
        return out
    for line in text.splitlines():
        name, sep, value = line.partition('=')
        if sep and not name.startswith('#'):
            out[name.strip()] = value.strip()
    return out


def post_id(agent, session, turn, call):
    """`ask-<32 hex>` of the tool call (one per call of one turn), else fresh."""
    if not call:
        return 'ask-' + uuid.uuid4().hex
    return 'ask-' + hashlib.sha256('\0'.join([agent, session or '', turn or '', call]).encode()).hexdigest()[:32]


def error(message):
    from tools.registry import tool_error
    return tool_error(message)


def ask(args, session_id=None, **_):
    from gateway.session_context import get_session_env
    from hermes_constants import get_hermes_home
    from tools.approval_context import _approval_tool_call_id, _approval_turn_id

    target = str(args.get('agent') or '').strip()
    question = str(args.get('question') or '').strip()
    if not target or not question:
        return error('name the teammate (agent) and what to ask it (question)')
    chat, _, agent = get_session_env('HERMES_SESSION_CHAT_ID', '').partition('/')
    # the turn's profile's home, as Hermes scopes it, else by its name
    root = Path(os.environ.get('HERMES_HOME', '/data/hermes'))
    homes = [get_hermes_home(), root / 'profiles' / (get_session_env('HERMES_SESSION_PROFILE', '') or agent)]
    env = next((e for e in map(profile_env, homes) if e.get('FRAGMENT_AS_AGENT') == agent), {})
    if not chat or not agent or not env.get('FRAGMENT_FOR'):
        return error('fragment_ask asks from a fragment chat of yours; this turn is in none')
    pid = post_id(agent, session_id or get_session_env('HERMES_SESSION_ID', ''), _approval_turn_id.get(), _approval_tool_call_id.get())
    cmd = [CLI, '--json', 'ask', '--return-to', chat, '--id', pid, '--', target, question]
    run_env = dict(os.environ, FRAGMENT_AS_AGENT=agent, FRAGMENT_FOR=env['FRAGMENT_FOR'])
    run_env.setdefault('FRAGMENT_API', API)
    try:
        done = subprocess.run(cmd, env=run_env, capture_output=True, text=True, timeout=CALL_S)
    except (OSError, subprocess.TimeoutExpired) as e:
        return error(f'the ask did not finish ({type(e).__name__}); ask again later')
    try:
        said = json.loads(done.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return error(f'the ask failed: {(done.stderr or done.stdout).strip()[:500]}')
    if not said.get('ok'):
        return error(str((said.get('error') or {}).get('message') or 'the ask failed'))
    data = said.get('data') or {}
    asked = data.get('asked') or {}
    return json.dumps({
        'status': 'asked',
        'agent': asked.get('title') or asked.get('name') or target,
        'chat': data.get('chat'),
        'next': 'Its answer comes back to you here as a new message once it is done. End your turn now.',
    })
