"""Mark the active profile after Hermes changes its repo-backed self."""
from hermes_constants import get_hermes_home

SELF_TOOLS = {'memory', 'skill_manage'}


def register(ctx):
    ctx.register_hook('post_tool_call', mark_self_changed)


def mark_self_changed(tool_name='', **_):
    if tool_name not in SELF_TOOLS:
        return
    try:
        (get_hermes_home() / '.fragment-self-sync-dirty').touch()
    except OSError:
        pass
