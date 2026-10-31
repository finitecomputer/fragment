# sessions

The blessed session archive template (decision 40): the conversations a
person had with an agent somewhere else, kept in a fragment of theirs of
kind `sessions`, titled Session Archive, private to them (`members`).
`fragment session import` makes it at their first import, as they run it
or as their agent acting for them (the archive is then theirs, with the
agent an editor of it), and writes it; the shell reads it. A fragment on
it names it and holds nothing else of the template's: `{"template":
"sessions", "meta": {"title": "Session Archive"}}`, then its data.

Its format is `session-archive/1`; its rules are
`fragment_core::session` (crates/core/src/session.rs), and the stores an
import reads are cli/src/session/.

## The files

- `sessions/<key>.json`: one conversation, pretty JSON:

  | field | what it is |
  |---|---|
  | `format` | `session-archive/1` |
  | `id` | its id where it came from (Hermes': the session it began as) |
  | `origin` | where it came from: `hermes` |
  | `reader` | which read made it: 10 the old Finite's export (a later, better or fixed read takes a higher number: An import) |
  | `agent` | the agent it was with, as its origin named it (optional) |
  | `kind` | `chat` (a person's conversation with the agent), `cron` (a scheduled job's run), `subagent` (a side task the agent ran) |
  | `title` | its own, else the first line its person said (80 characters), else `Untitled` |
  | `source`, `with`, `model` | where it was said (`telegram`, `cli`), whom with, on which model, as its origin named them (optional) |
  | `started_at`, `ended_at` | when it began, and when it was last said (its last message's time, else its end) |
  | `parts` | its origin's own records it joins, in order (Hermes starts a new session each time it compresses a long conversation) |
  | `messages` | `[{role, at?, text?, tool?, call?, tool_calls?: [{id?, name, arguments}], reasoning?, summary?}]`, in the order they were said |

  Times are UTC, `YYYY-MM-DDTHH:MM:SS.sssZ`, so they sort as text. A
  message's `role` is `user`, `assistant`, `tool` or `system`; `tool` is
  the tool a result came from and `call` the call it answers (a call's
  `id`, where its origin kept one); `summary` marks the summary its origin
  wrote when it compressed. An image, a recording or a file a message
  held is a mark in its text (`[image]`, `[audio]`, `[video]`, `[file]`).
  A tool's output and an assistant's reasoning are cut to 2,000
  characters and a call's arguments to 1,000, saying how many more there
  were (`… [N more characters not kept]`); what a person and the agent
  said never is.
- `index/<kind>.json`: `{format, kind, sessions: [entry]}`, the
  conversations of one kind, newest first (by `ended_at`, else
  `started_at`). An entry is `{key, id, origin, reader, kind, title,
  source?, with?, started_at?, ended_at?, messages, preview, sha256,
  bytes}`:
  `messages` counts what was said (a person's or the agent's words, not
  tool calls, results or summaries), `preview` is the first words its
  person said (160 characters), and `sha256` and `bytes` are its file's.

A conversation's key is its origin and its id there,
`hermes-20261029_183012_ab12cd`; an id that is no file name's (another
character, over 100 bytes, not starting with a letter or digit) is keyed
by its hash instead, `hermes--<32 hex>`. A file of 1 MiB or more is one
of the fragment's blobs (a pointer in git, docs/api.md, Blobs), as any
fragment's is.

## An import

- **Keyed by the original id.** The same conversation imported again (a
  re-run, a retry after a crash, the same chats from another of its
  origin's stores) is found by its key and replaced only by a newer copy
  of it: one last said later, or as late from a better read (a higher
  `reader`: a reader fixed since, so its fixes reach archives made
  before). The same bytes, an older copy, or one as late from a read no
  better, write nothing.
- **Conversations first, then the indexes.** An import writes its new and
  newer conversations a batch a commit (at most 256 files and 16 MiB of
  them), then every index they change in one last commit, made from main
  as it is then. That commit also lists by its file any conversation the
  indexes lack, or whose file changed in a commit after theirs (an import
  stopped before its last commit, whatever the copy's size: a file's last
  commit, with its path, names its bytes), or that they describe as
  another copy (its size in git, or a blob's pointer, is not their
  entry's), and drops an entry whose file is gone, so the next import of
  anything puts a crash right. Each commit names the head it read; one
  another writer beat, or whose answer was lost, is made again on main as
  it is then, the indexes listing what this import wrote as it wrote it.
- **Bounded as it reads.** Each conversation is filed as it is read, its
  file spooled to a folder of the import's own (gone with it), so an
  import never holds every conversation's bytes.
- **Limits.** An archive holds at most 18,000 conversations and 1 GiB of
  their files, inside what a person's export takes of one fragment (its
  files and its bytes) with room to spare, so an archive never fails its
  person's export: an import that would pass either is refused whole,
  before an archive is made or anything written (`too_large`; `--kind
  chat` brings only the chats). A conversation's file is at most 8 MiB:
  one larger is left out, saying so, and the rest imported. Its id is at
  most 200 bytes, and one refused is said escaped and cut.

## Where they come from

- **The old Finite's export** (format `finite-chat-archive/1`: its
  `chats/`, or the folder holding it; cli/src/session/chats.rs), whose
  records are Hermes' sessions as its `export.py` joined them, keyed by
  the same ids. It reads only regular files inside `chats/`, no link out
  of it and no device, each at most 512 MiB, read through a message at a
  time (a picture's base64, which `export.py` copied, is not held).

Another store (Hermes' own database, another agent runtime's sessions) is
another read into the same format.
