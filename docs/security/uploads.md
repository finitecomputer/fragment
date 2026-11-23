# Untrusted uploads: how files are taken and served

For the CASA assessment (OWASP ASVS 4.0 V12, files and resources;
FIN-583). It covers what people and agents upload, the limits on it, how
it's stored, and how it's served back so it never runs as one of the
platform's pages. It was written from the code on 2026-11-23, with `master`
at `06e158bc7`. Limits are named by their constants, whose values live in
`fragment_proto::limits` (`crates/proto/src/lib.rs`) or the module named.

## The rule

An uploaded file is data. The platform's own origin, where people's
sessions live, never answers one as a page or a script. Each fragment's
files are served from that fragment's own origin, on a site apart from the
platform's (decision 5; docs/MODEL.md). There, a file that is data can't
run either: HTML is sandboxed, scriptable media is sandboxed, and every
answer carries `X-Content-Type-Options: nosniff`.

The one thing that does run is a fragment's `site/`, its author's own
pages. That code runs on the fragment's own origin by design, as a website
does, and it reaches only what its visitor may reach there (docs/api.md,
Which cookies count; the `isolation` e2e section).

## What is taken

| Upload | Route | Limits and checks | Where |
|---|---|---|---|
| A large file's bytes, and a chat's attachments | `PUT /api/f/{name}/blobs/{sha256}` (editor) | `content-length` required, at most `UploadLimits.upload_max_bytes` (a deployment's `upload_max_bytes`, else `UPLOAD_MAX_BYTES_DEFAULT`; an agent's is `BLOB_MAX_BYTES`), checked by the router before the body is read (`lib.rs` `blob_length`) and again under `BLOB_MAX_BYTES`. Streamed, never held: `FixedLengthStream` refuses a body of another length, and the bytes' SHA-256 must be the one named, or the object is deleted (`js::blob_put`, `blobs.rs` `store_blob`). Files per message: `FILES_PER_MESSAGE`. | R2 `BLOBS`, keyed `<fragment npub>/<sha256>` |
| Files into a fragment's repo | `POST /api/f/{name}/files` (editor), git push with an editor's storage token | Every path passes `valid_repo_path`: relative, no `.` or `..` part, no empty part, no `\`, no control byte, no drive letter, at most `PATH_MAX_BYTES`. Bodies at most `BODY_MAX_BYTES`. A file of `BLOB_MIN_BYTES` or more is a pointer to a blob. | code.storage git |
| A preview card | `PUT /api/f/{name}/card` (editor) | A whole JPEG of the card's size, within its cap (`card::check_upload`) | a blob typed `image/jpeg` |
| An icon | `PUT /api/f/{name}/icon` (editor) | At most `icon::UPLOAD_MAX_BYTES`, an image by its magic bytes (`face::picture_type`), then re-encoded by Cloudflare Images to a PNG, unless it is already one of the icon's size within `ICON_MAX_BYTES` | a blob typed `image/png` |
| A person's picture | `PUT /api/identities/me/picture` | At most `PICTURE_MAX_BYTES`, PNG, JPEG, GIF or WebP by magic bytes (`face::check_picture`) | R2 `pictures/<sha256>` |
| An agent's picture | `avatar.png` in its fragment | Checked as main moves, as a picture is | git, then its face |
| Audio to transcribe | `POST /api/models/v1/audio/transcriptions` | Body at most `transcribe::BODY_MAX_BYTES`, audio at most `AUDIO_MAX_BYTES` | passed to the model, not kept |
| A search source | `POST /api/f/{name}/search/upsert` (editor) | At most `search::UPSERT_MAX_BYTES`, checked by `search::rules::check` | the fragment's index |
| A finite.computer move's zip | `PUT /api/migrations/mover/stages/{stage}/parts/{n}` | The mover's key alone; each part's hash in its signed URL | R2, the stage's |
| Any other JSON body | every route | At most `BODY_MAX_BYTES`, refused from its declared length before it is read (`lib.rs` `read_body`; the `lockdown` e2e section) | |

A blob's declared type is kept only if it is passive media:
`blob::served_type` allows JPEG, PNG, WebP, GIF, MP4, WebM, MPEG/WAV/WebM/Ogg/MP4
audio and PDF. Anything else, SVG and HTML among them, is stored with no
type and served as `application/octet-stream`.

## How it is served

| Where | Answers | Headers |
|---|---|---|
| `https://{name}.<suffix>/__blob/{sha}` (the fragment's origin, viewer) | its bytes, the kept type or `application/octet-stream` | `nosniff`, immutable cache, `referrer-policy: same-origin` (`blobs.rs` `serve_blob`, `lib.rs` `fragment_origin`) |
| `…/__file?path=` (the content plane: a file opened from the Files view) | a repo file, typed by its extension (`site::mime_for_path`) | HTML gets `content-security-policy: sandbox` (no script, an opaque origin: `serve.rs` `content_file`). SVG and the XML types get `sandbox` on every answer of the origin (`fragment_origin`, `site::opened_as`). Everything gets `nosniff`. The fragment's own organs (`fragment.json`, `app.mjs`, dot-files and the like: `site::is_machinery`) aren't served. |
| `…/` (the site, from `site/` at the live pin) | the author's pages and assets | `nosniff`, `referrer-policy`, and `sandbox` for scriptable media. The platform's service worker is the only one (`serve.rs`). |
| `GET /api/f/{name}/blobs/{sha}` (the platform's origin) | the bytes, always `application/octet-stream` | `nosniff` (`blobs.rs` `get_blob`, `lib.rs` `platform_origin`) |
| `GET /api/f/{name}/file?path=` (the platform's origin) | a repo file, typed by its extension | `nosniff`, `no-store`. It needs a signature or the shell's header with `Sec-Fetch-Site: same-origin`, so a navigation gets a 401 and never renders it. The shell reads these files as text or JSON. FIN-650 adds `sandbox` here too, as defence in depth. |
| `GET /api/f/{name}/card`, `/icon`, `/api/identities/{id}/picture` | a JPEG, a PNG, a checked image | a fixed type, `nosniff` |
| `GET /api/export/{id}/download` | the person's tar | `application/x-tar`, `Content-Disposition: attachment`, `nosniff`, `private, no-store` |
| `GET /api/migrations/stages/{stage}/zip` | the stage's zip, for its agent | `application/zip`, `nosniff` (`platform_origin`) |

Content-Disposition: the fragment's origin serves a blob inline on
purpose, because chats show pictures and play audio, and a PDF opens in
the browser's viewer. That is safe because its type is passive or
`application/octet-stream`, `nosniff` holds the browser to it, and the
origin is the fragment's, never the platform's.

## Names and paths

- A repo path from outside, whether from the API, git, the CLI's folder
  scan or a listing, becomes a `RepoPath` (`valid_repo_path`) before
  anything reads, writes or joins it. `RepoPath::under` is the one join
  onto a local folder (`crates/proto/src/lib.rs`).
- An attachment's `name` is a label. The bridge saves its bytes in its
  scratch folder under the blob's checked SHA-256, never under the name
  (`images/bridge/src/transfer.rs` `download`).
- A computer's file delete (`POST /api/computers/{id}/files/delete`, its
  owner) takes a path under `work/` with no `.`, `..`, control character
  or `?`. It is quoted for the shell, resolved with `readlink -f`, and
  refused unless the result is a regular file under `/data/work`
  (`fragment_core::computer::delete_read`).

## Gaps and observations

- **Not run here.** This pass used no e2e lanes. The behaviour above is
  covered by the `blobs`, `appfiles`, `files`, `site`, `isolation`,
  `icons`, `faces`, `export` and `lockdown` sections on `master`'s CI, not
  re-run for this write-up.
- **FIN-650:** the platform's file route answers an editor's HTML or SVG
  with its own type. A navigation can't reach it, but `sandbox` there
  would make that hold by itself.
- **PDF:** `application/pdf` is served inline and not sandboxed, because
  Chrome's viewer doesn't render under a CSP sandbox. A PDF's script runs
  in the browser's PDF viewer, on the fragment's origin, not the
  platform's.
- **Malware scanning:** uploads aren't scanned for malware. Files are
  never executed by the platform. A computer's agent may open what its
  person sends it, inside that person's own container.
