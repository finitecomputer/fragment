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
  | `reader` | which read made it: 11 the old Finite's export (10 before it read the files messages held), 20 Hermes' own database (a later, better or fixed read takes a higher number: An import) |
  | `agent` | the agent it was with, as its origin named it (optional) |
  | `kind` | `chat` (a person's conversation with the agent), `cron` (a scheduled job's run), `subagent` (a side task the agent ran) |
  | `title` | its own, else the first line its person said (80 characters), else `Untitled` |
  | `source`, `with`, `model` | where it was said (`telegram`, `cli`), whom with, on which model, as its origin named them (optional) |
  | `started_at`, `ended_at` | when it began, and when it was last said (its last message's time, else its end) |
  | `parts` | its origin's own records it joins, in order (Hermes starts a new session each time it compresses a long conversation) |
  | `messages` | `[{role, at?, text?, tool?, call?, tool_calls?: [{id?, name, arguments}], reasoning?, summary?, attachments?}]`, in the order they were said |

  Times are UTC, `YYYY-MM-DDTHH:MM:SS.sssZ`, so they sort as text. A
  message's `role` is `user`, `assistant`, `tool` or `system`; `tool` is
  the tool a result came from and `call` the call it answers (a call's
  `id`, where its origin kept one); `summary` marks the summary its origin
  wrote when it compressed. A file a message held whose bytes came with it
  is one of its `attachments`, `{name, media_type, size, sha256, path}`,
  its bytes the archive's `path`, `media/<sha256>.<ext>` (each file once,
  however many messages hold it; at most 25 MiB, and counted toward the
  archive's bytes); one whose bytes did not is a mark in its text
  (`[image]`, `[audio]`, `[video]`, `[file]`), as its origin left it.
  A tool's output and an assistant's reasoning are cut to 2,000
  characters and a call's arguments to 1,000, saying how many more there
  were (`… [N more characters not kept]`); what a person and the agent
  said never is.
- `index/<kind>.json`: `{format, kind, sessions: [entry]}`, the
  conversations of one kind, newest first (by `ended_at`, else
  `started_at`). An entry is `{key, id, origin, reader, kind, title,
  agent?, source?, with?, started_at?, ended_at?, messages, preview,
  sha256, bytes}`:
  `agent` is the agent it was with, as its origin named it (an entry an
  import made before it has none), `messages` counts what was said (a
  person's or the agent's words, not tool calls, results or summaries),
  `preview` is the first words its person said (160 characters), and
  `sha256` and `bytes` are its file's.

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
  before), or as late from as good a read holding more of its files (one
  missing before, found since). The same bytes, an older copy, or one as
  late from a read no better, write nothing.
- **Files go with what keeps them.** A message's file is read only for a
  conversation of a kind the import asks for; it counts toward the
  archive's bytes with the conversation that keeps it, and goes up only
  with a conversation written: none of a refused conversation's, nor of a
  copy set aside for a newer one (the import's own or the archive's).
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

## Search

Each import also writes the archive's search, through the platform's
search write API (`/api/f/{name}/search/…`), as the writer holding the
conversations' bytes: one source a conversation's file, its revision
the file's sha256, and one section a turn (fragment_core::session_search),
each checked as the platform checks it (`searchable::check`) before it is
sent.

- **A turn** is a person's message and what the agent said until their
  next one, as "Person: … / Agent: …": a tool's output left out and its
  call only named, reasoning left out, the files a message held named,
  and the summary a compression wrote a section of its own. Turns are
  numbered from the start (`t<n>`), so a conversation that grew keeps its
  earlier sections, and only its new turns are embedded again. A turn
  past 8,000 characters splits at its messages (a message past that,
  within it); a conversation past 2,000 sections or 4 MiB keeps its first,
  saying how many it left out. Its fields are `kind`, `origin` and
  `source`; a section's context its source and agent, its location the
  places of its first and last message.
- **Reconciled, each import, with the archive as it stands then.** After
  its writes, an import reads search's sources and the archive at main's
  head (its indexes, and the files changed since them: another import may
  have written meanwhile), and writes each conversation whose sha256 is
  not the revision search holds, its bytes read at that head and its
  revision theirs. Search is read first, so what it holds is never of a
  newer copy than the archive read after. Each write names the revision
  it replaces, as search was read holding it, so another import's write
  since is never written over; refused, that conversation is read anew
  from search and the archive and written again if they still differ (a
  few passes at most, those left counted `unresolved` for the next
  import). Whichever import's write lands first, search ends holding the
  archive's copy. Nothing is deleted: the
  platform forgets a conversation's source when a move of main deletes its
  file. So an import that writes nothing puts search right too, and an
  archive made before its host had search is searched from its next
  import. A host without search, or one that cannot be reached, is said
  and skipped: the import stands. A search that is full (its index's
  chunks are capped: 507) is sent no more; each conversation not written
  is counted refused, and the next import writes it once there is room.

### Section text

As written (fragment_core::session_search: `PERSON`, `AGENT`,
`SUMMARY`): one line a message kept, in order, joined by line ends. A
person's is `Person: <text>`, then ` [<file>, …]` when it held files.
The agent's is `Agent: <text>`, then ` [<file>, …]`, then
` (used <tool>, …)` when it called tools, each part left out when empty
(a call alone reads `Agent: (used web_search)`). A compression's
summary, a section of its own (heading `Summary`), is `Summary: <text>`.
`<text>` is the message trimmed, with no control character but a line's
end and a tab, so a message's own line breaks are lines with no prefix
after its first; a name is at most 64 bytes, its spaces collapsed and
`=` a space. A turn split past 8,000 characters, and a search's snippet
(at most 300 bytes of one chunk, around the words asked, `…` where cut),
may begin within a line. For example (a test holds these to what is
written):

```
Person: when do I plant beans? [plot.png]
Agent: (used web_search)
Agent: In spring, after the frost.
```

```
Summary: Ann plants beans in spring.
```

It is for whoever reads search: an agent reads it as it is; a shell
renders it for people (History: `You`, the agent's name from the
archive's index, the markers dropped).

## Where they come from

- **The old Finite's export** (format `finite-chat-archive/1`: its
  `chats/`, or the folder holding it; cli/src/session/chats.rs), whose
  records are Hermes' sessions as its `export.py` joined them, keyed by
  the same ids. A message's files come with it when it names them in
  `attachments` (the archive's own fields, `path` under `chats/`), each
  checked against its `sha256` and `size`; one that is not what it names,
  missing, or outside `chats/` is left out of its message, said. It reads
  only regular files inside `chats/`, no link out of it and no device,
  each at most 512 MiB (a file a message held, 25 MiB), read through a
  message at a time (a picture's base64, which `export.py` copied, is not
  held).
- **Hermes' `state.db`** (or a Hermes home holding it), read-only, in one
  read transaction (cli/src/session/hermes.rs, which says how it follows
  Hermes v0.21.7, checked against what that Hermes writes:
  `hermes-v0.21.7.sql`): a conversation is a session and the ones Hermes
  continued it in each time it compressed, as Hermes' own session list
  joins them; its messages are what Hermes shows a person, each once. A
  better read than the export's (20 to 11), which kept a compression's
  copied tail twice. Bounded by what is kept: SQLite hands over only a
  message's text parts and a mark for each medium (never a picture's
  data), each call's id, name and the start of its arguments, and a
  tool's output and reasoning their cut's worth (a list that is no list
  of typed parts, as a person's `["red", "blue"]`, kept as the text it
  is); a conversation whose kept rows (each row counted, empty or not)
  pass twice its limit is refused alone, and so is one SQLite cannot read
  whole, never kept as the part read; a database of more than 200,000
  sessions is refused whole. On a platform computer (the CLI as its
  agent) its own databases (its Hermes root's, any profile's) are
  refused: their conversations are chats here already.

Another agent runtime's sessions (Claude Code's, Codex's) are another
read into the same format.
