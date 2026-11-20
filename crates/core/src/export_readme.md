# Your Fragment export

This is everything that is yours on a Fragment deployment: the fragments you
own and the ones your agents own, your computer's newest good save, and the
names of the fragments other people share with you. `manifest.json` says
whose it is, which deployment it is from, and when it was made.

It is one tar file, not compressed. Open it with your system's archive tool,
or `tar -xf <file>.tar`; everything is in the folder `fragment-export/`.

## Layout

    fragment-export/
      README.md                     this file
      manifest.json                 what the export holds, and whose it is
      fragments/<name>/             each fragment you own, and each one your agents own
        meta.json                   what it is, who is in it, how it is shared
        files/<path>                its files, as main (its working copy) holds them
        database/schema.sql         its app's database: the statements that make it
        database/<table>.jsonl      that table's rows
        channels/<channel>.jsonl    a channel's records, in order
        transcript.md               a chat's conversation, to read (chats only)
        blobs/<sha256>              the files its records attach
      computers/<id>/               your computer (its id's 24 hex digits)
        meta.json                   its image, its agents, and the save exported
        data.tar.zst                the save of /data, but /data/work
        data-work.tar.zst           the same save's /data/work: its agents' workspace

A `<name>` is a fragment's name (`todo--k3x9`). A name in `database/` is its
table's, with any byte but `A-Z a-z 0-9 . _ -` written `%XX`.

## manifest.json

    {
      "format": "fragment-export", "version": 1,
      "deployment": { "id": "...", "platform": "https://..." },
      "madeAt": 1791000000000, "made": "2026-10-13T16:00:00Z",
      "export": "<16 hex>",
      "person": { "npub": "npub1...", "email": "you@example.com" },
      "agents": ["npub1..."],
      "fragments": [{ "name", "kind", "title", "owner", "path" }],
      "computers": [{ "id": "computer:<24 hex>", "path" }],
      "sharedWithMe": [{ "name", "kind", "title", "owner", "role", "member" }]
    }

`kind` is `app`, `chat`, `agent`, `brain` or `skills`. People and agents are
named by their npubs. `sharedWithMe` names each fragment someone else owns
that you (or an agent of yours: `member`) are in, with your role there; it
copies none of them. Ask their owners for theirs. Every `path` and `file` a
JSON file names is relative to its own folder.

## A fragment

`meta.json` holds what its owner reads of it: its kind and title, the blessed
template it runs (if one), its npub, its owner, its visibility, whether a
share link or an inbox token exists (never the tokens), the commits its
`main` and `live` are at, the code live runs, its members (a person with
their email and their agents, who reach what they do: a share is a
person's and their agents'; an agent with its name, its title and its owner's email),
the invites waiting on an email, its secrets'
names (never their values), and how many files, rows, records and blobs it
holds. `skipped` names anything the export could not take, and why.

`files/` is the fragment's files at `main`, its working copy, byte for byte:
a file of 1 MiB or more is there whole (git holds it as a pointer to its
blob). An agent's soul, memories, skills and picture are its fragment's files
(`SOUL.md`, `memories/`, `skills/`, `avatar.png`). Git's history is not in the archive.

`database/schema.sql` is the `CREATE` statements of the app's SQLite
database, each ending in `;`. Each `database/<table>.jsonl` holds one row a
line: a JSON object of its columns. A text is a string; an integer or a real
is a number (one past 2^53 may be rounded); a blob is
`{"$base64": "..."}`; a null is `null`. The platform's own table (its
operation ledger) is left out.

Each `channels/<channel>.jsonl` holds one record a line, oldest first:

    { "seq": 1, "at": 1791000000000, "principal": "npub1...", "kind": "message", "body": { ... } }

`at` is milliseconds since 1970 (UTC); `principal` is who wrote it (an npub,
`anon:...` for a visitor, `platform` for the platform). `events` is the
fragment's audit trail, `ops` its applied operations, `inbox` its webhooks,
and the rest are its app's. A chat's are its template's: `chat` holds
what was said (a message `{"text", "attachments"?, "reply_to"?}`, an
agent's reply `{"text", "turn"}`, `{"kind": "stop"}`, a prompt's answer),
and `work` each agent's turns (`turn.start`, `turn.step`, `turn.prompt`,
`turn.prompt.closed`, `turn.notice`, `turn.end`); Fragment's repository
documents them whole (docs/chat-records.md).

`transcript.md` is a chat's `chat` and `work` records, read in order: who
said what and when, each agent's steps, its questions and their answers.

`blobs/` holds each file a record attaches (a record's `attachments`), named
by its SHA-256, as it was uploaded.

## A computer

`meta.json` holds the computer's view as its owner reads it (its image, its
version, its agents, its saves) and the save exported: its number, when it
was taken, and its records.

Each `.tar.zst` is one record of that save as it is stored: a tar of the
directory, compressed with zstd (`tar --zstd -xf data.tar.zst`, or
`zstd -d data.tar.zst && tar -xf data.tar`). `data.tar.zst` is `/data` less
`/data/work`: the agent runtime's own state (our Hermes image keeps its
sessions, databases and profiles there). `data-work.tar.zst` is
`/data/work`: what its agents' tools wrote (their workspace). A save taken
before the two were apart is one `data.tar.zst`. Each record's `sha256` in
`meta.json` is that file's. What the computer did after that save is in no
save yet.

## What is not here

Your sign-ins and keys, your connections' tokens, your own keys (sealed in
your computer), secrets' values, share links' and inbox tokens, your usage
ledger, git history, the runs of jobs, apps' preview cards, and anything
of the fragments others share with you.
