#!/usr/bin/env python3
"""Dropbox through the person's connected Dropbox account.

The computer holds no Dropbox credential: its environment holds the
connection's placeholder, which names the agent (`DROPBOX_OAUTH_ACCESS_TOKEN`),
each request sends it as a bearer token to api.dropboxapi.com (calls) or
content.dropboxapi.com (uploads and downloads), and the computer's intercept
swaps in a short-lived token for the agent's owner (docs/computers.md,
Connections and operator keys). Python's standard library only; every command
prints JSON.

Usage:
  python dropbox_api.py check
  python dropbox_api.py list [/Folder] [--recursive] [--max 100]
  python dropbox_api.py list --cursor CURSOR
  python dropbox_api.py search "quarterly plan" [--path /Work] [--ext pdf] [--filename-only] [--max 25]
  python dropbox_api.py search --cursor CURSOR
  python dropbox_api.py get /Work/plan.pdf
  python dropbox_api.py download /Work/plan.pdf [--to ./plan.pdf] [--overwrite]
  python dropbox_api.py upload ./notes.txt --to /Work/notes.txt [--mode add|overwrite|update --rev REV] [--autorename]
  python dropbox_api.py mkdir /Work/2026 [--autorename]
  python dropbox_api.py move /Work/a.txt /Archive/a.txt [--autorename]
  python dropbox_api.py copy /Work/a.txt /Work/b.txt [--autorename]
  python dropbox_api.py delete /Work/old.txt
  python dropbox_api.py link /Work/plan.pdf
  python dropbox_api.py request POST /2/files/list_revisions --json '{"path": "/Work/plan.pdf"}'
  python dropbox_api.py --root NAMESPACE_ID list    # a team space's root (check names it)

A path is Dropbox's: "" or / for the root, /Folder/file.txt, or an id
(id:a4ayc_80_OEAAAAAAAAAXw), optionally with a path under it.

Environment:
  DROPBOX_OAUTH_ACCESS_TOKEN  the Dropbox connection's placeholder (set on a
                              fragment computer once the person connects
                              Dropbox), or any Dropbox access token
"""

import argparse
import http.client
import json
import os
import re
import sys
import urllib.error
import urllib.request

API = "https://api.dropboxapi.com"
CONTENT = "https://content.dropboxapi.com"
TIMEOUT_S = 60
CONTENT_TIMEOUT_S = 600
# Dropbox's own limits: list_folder's `limit` and search's `max_results`
LIST_MAX = 2000
SEARCH_MAX = 1000
# files/upload takes a file of at most 150 MiB; larger needs an upload session
UPLOAD_MAX_BYTES = 150 * 1024 * 1024
PATH_MAX_BYTES = 4096
CURSOR_MAX_BYTES = 65536
CHUNK_BYTES = 1 << 16
# a file's or folder's id, as Dropbox names one
ITEM_ID = re.compile(r"^id:[A-Za-z0-9_\-]{1,256}$")
REV = re.compile(r"^[0-9a-f]{9,64}$")
NAMESPACE = re.compile(r"^[0-9]{1,32}$")
EXTENSION = re.compile(r"^[A-Za-z0-9]{1,16}$")
# an RPC route on api.dropboxapi.com: /2/<namespace>/<route>[/<more>]
ROUTE = re.compile(r"^/2/[a-z0-9_]+(/[a-z0-9_]+)+$")
CONTROL = re.compile(r"[\x00-\x1f\x7f]")
BRIEF_FIELDS = (".tag", "name", "path_display", "id", "size", "server_modified", "rev")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


class DropboxError(Exception):
    def __init__(self, status, code, message):
        super().__init__(message)
        self.status = status
        self.code = code


def token():
    value = os.getenv("DROPBOX_OAUTH_ACCESS_TOKEN", "").strip()
    if not value:
        raise DropboxError(0, "not_connected", "DROPBOX_OAUTH_ACCESS_TOKEN is not set: the person has not connected Dropbox, or narrowed you from it")
    return value


ROOT = {"namespace": None}


def headers(extra=None):
    h = {"Authorization": f"Bearer {token()}"}
    if ROOT["namespace"]:
        h["Dropbox-API-Path-Root"] = json.dumps({".tag": "root", "root": ROOT["namespace"]})
    return {**h, **(extra or {})}


def send(request, timeout):
    """`request` answered, or its refusal raised as a DropboxError."""
    try:
        return OPENER.open(request, timeout=timeout)
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise DropboxError(exc.code, "redirected", f"Dropbox answered a redirect to {exc.headers.get('Location', '?')}, which the helper never follows with the token") from None
        raise refusal(exc.code, exc.read().decode("utf-8", errors="replace")) from None
    except urllib.error.URLError as exc:
        raise DropboxError(0, "unreachable", str(exc.reason)) from None


def rpc(route, args=None):
    """An RPC call (`route` from /2/) on api.dropboxapi.com: its arguments as
    a JSON body (none for a route that takes none), its answer JSON."""
    data = None if args is None else json.dumps(args).encode("utf-8")
    h = headers({"Content-Type": "application/json"} if data is not None else None)
    with send(urllib.request.Request(API + route, data=data, headers=h, method="POST"), TIMEOUT_S) as response:
        raw = response.read()
    return json.loads(raw.decode("utf-8")) if raw.strip() else None


def arg_header(args):
    """Dropbox-API-Arg: JSON made HTTP-header safe, every character from
    0x7F up as \\uXXXX (json.dumps' default ensure_ascii does exactly that)."""
    return json.dumps(args, ensure_ascii=True)


def refusal(status, text):
    """An HTTP refusal: the computer's own (`{error: "not_connected" |
    "forbidden", message}`), Dropbox's (`{error_summary, error, user_message}`),
    or Dropbox's plain text for a malformed call (400)."""
    try:
        err = json.loads(text)
    except ValueError:
        err = None
    if isinstance(err, dict) and "error_summary" in err:
        e = err["error"] if isinstance(err.get("error"), dict) else {}
        # error_summary ends in a random run of dots: Dropbox asks for prefix matching
        code = str(err.get("error_summary") or e.get(".tag") or "").rstrip("./") or "dropbox_error"
        user = err.get("user_message")
        message = (user.get("text") if isinstance(user, dict) else None) or code
        if e.get(".tag") == "missing_scope" and e.get("required_scope"):
            message = f"the connection lacks Dropbox's {e['required_scope']} scope"
        return DropboxError(status, code, message)
    if isinstance(err, dict) and isinstance(err.get("error"), str):
        return DropboxError(status, err["error"], err.get("message") or err["error"])
    return DropboxError(status, "bad_request" if status == 400 else "", text[:500] or f"HTTP {status}")


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def bounded(n, most):
    return max(1, min(n, most))


def dropbox_path(value, root_ok=False):
    """A path in the person's Dropbox, as Dropbox takes one: the root ("",
    where `root_ok`), /a/b with no empty, . or .. segment (a trailing slash
    dropped), or an item's id, optionally with such a path under it."""
    value = value or ""
    if value in ("", "/"):
        if root_ok:
            return ""
        raise DropboxError(0, "bad_path", "name a file or folder, not the root")
    if CONTROL.search(value) or len(value.encode("utf-8")) > PATH_MAX_BYTES:
        raise DropboxError(0, "bad_path", f"{value!r} is not a Dropbox path")
    if value.startswith("id:"):
        head, _, rest = value.partition("/")
        if not ITEM_ID.match(head):
            raise DropboxError(0, "bad_path", f"{head!r} is not a Dropbox id (id:…)")
    elif value.startswith("/"):
        head, rest = "", value[1:]
    else:
        raise DropboxError(0, "bad_path", f"{value!r} is not a Dropbox path: /Folder/file.txt, or an id (id:…)")
    segments = rest.rstrip("/").split("/") if rest.rstrip("/") else []
    if any(s in ("", ".", "..") for s in segments) or not (head or segments):
        raise DropboxError(0, "bad_path", f"{value!r}: a path with no empty, . or .. segment in it")
    return head + "".join("/" + s for s in segments)


def cursor(value):
    """A cursor Dropbox answered, passed back as it came."""
    if not value or CONTROL.search(value) or len(value) > CURSOR_MAX_BYTES:
        raise DropboxError(0, "bad_cursor", "pass the cursor of an earlier answer, as it came")
    return value


def brief(metadata):
    """A file's or folder's metadata, its fields that name and place it."""
    return {k: metadata[k] for k in BRIEF_FIELDS if k in metadata}


def check(_args):
    me = rpc("/2/users/get_current_account") or {}
    roots = me.get("root_info") or {}
    out({
        "status": "connected",
        "name": (me.get("name") or {}).get("display_name"),
        "email": me.get("email"),
        "account_id": me.get("account_id"),
        "account_type": (me.get("account_type") or {}).get(".tag"),
        "team": (me.get("team") or {}).get("name"),
        "home_namespace_id": roots.get("home_namespace_id"),
        "root_namespace_id": roots.get("root_namespace_id"),
        "team_space": bool(roots) and roots.get("home_namespace_id") != roots.get("root_namespace_id"),
    })


def listing(answer):
    return {"entries": [brief(m) for m in answer.get("entries", [])], "has_more": answer.get("has_more", False), "cursor": answer.get("cursor")}


def list_folder(args):
    if args.cursor:
        out(listing(rpc("/2/files/list_folder/continue", {"cursor": cursor(args.cursor)})))
        return
    out(listing(rpc("/2/files/list_folder", {"path": dropbox_path(args.path, root_ok=True), "recursive": args.recursive, "limit": bounded(args.max, LIST_MAX)})))


def search_options(args):
    options = {"max_results": bounded(args.max, SEARCH_MAX), "filename_only": args.filename_only}
    if args.path:
        options["path"] = dropbox_path(args.path, root_ok=True)
    extensions = [e.lstrip(".") for e in args.ext or []]
    if not all(EXTENSION.match(e) for e in extensions):
        raise DropboxError(0, "bad_extension", "--ext is a file extension, e.g. pdf")
    return {**options, "file_extensions": extensions} if extensions else options


def search(args):
    if args.cursor:
        answer = rpc("/2/files/search/continue_v2", {"cursor": cursor(args.cursor)})
    elif not (args.query or "").strip():
        raise DropboxError(0, "bad_search", "say what to search for, or pass --cursor")
    else:
        answer = rpc("/2/files/search_v2", {"query": args.query, "options": search_options(args)})
    matches = [((m.get("metadata") or {}).get("metadata") or {}) for m in answer.get("matches", [])]
    out({"matches": [brief(m) for m in matches], "has_more": answer.get("has_more", False), "cursor": answer.get("cursor")})


def get(args):
    out(rpc("/2/files/get_metadata", {"path": dropbox_path(args.path)}))


def local_destination(path, to, overwrite):
    """Where a download is saved: `to` (in it, where it is a folder), else
    the file's name here. An id names no file: it needs `to`."""
    name = path.rsplit("/", 1)[-1]
    into = bool(to) and os.path.isdir(to)
    if (to is None or into) and path.startswith("id:") and "/" not in path:
        raise DropboxError(0, "bad_destination", "an id names no local file: pass --to and a file's name")
    dest = os.path.join(to, name) if into else (to or name)
    if os.path.exists(dest) and not overwrite:
        raise DropboxError(0, "exists", f"{dest} exists already: pass --overwrite to replace it, or --to another path")
    return dest


def download(args):
    path = dropbox_path(args.path)
    dest = local_destination(path, args.to, args.overwrite)
    h = headers({"Dropbox-API-Arg": arg_header({"path": path})})
    part = dest + ".part"
    with send(urllib.request.Request(CONTENT + "/2/files/download", headers=h, method="POST"), CONTENT_TIMEOUT_S) as response:
        try:
            result = json.loads(response.headers.get("Dropbox-API-Result") or "{}")
        except ValueError:
            result = {}
        size = save(response, part)
    os.replace(part, dest)
    out({"saved_to": dest, "bytes": size, "metadata": brief(result)})


def save(response, part):
    """The answer's body written to `part`, its size; nothing left of it
    where the download fails."""
    size = 0
    try:
        with open(part, "wb") as f:
            while chunk := response.read(CHUNK_BYTES):
                f.write(chunk)
                size += len(chunk)
    except (OSError, http.client.HTTPException) as exc:
        if os.path.exists(part):
            os.remove(part)
        raise DropboxError(0, "download_failed", str(exc)) from None
    return size


def write_mode(args):
    if args.mode == "update":
        if not REV.match(args.rev or ""):
            raise DropboxError(0, "bad_rev", "--mode update overwrites only the revision named: pass --rev, the file's rev (get shows it)")
        return {".tag": "update", "update": args.rev}
    if args.rev:
        raise DropboxError(0, "bad_rev", "--rev goes with --mode update alone")
    return args.mode


def upload(args):
    if not os.path.isfile(args.file):
        raise DropboxError(0, "no_such_file", f"{args.file} is not a local file")
    if os.path.getsize(args.file) > UPLOAD_MAX_BYTES:
        raise DropboxError(0, "too_large", "files/upload takes at most 150 MiB; a larger file needs an upload session")
    upload_arg = {"path": dropbox_path(args.to), "mode": write_mode(args), "autorename": args.autorename, "mute": False}
    with open(args.file, "rb") as f:
        body = f.read()
    h = headers({"Dropbox-API-Arg": arg_header(upload_arg), "Content-Type": "application/octet-stream"})
    with send(urllib.request.Request(CONTENT + "/2/files/upload", data=body, headers=h, method="POST"), CONTENT_TIMEOUT_S) as response:
        out(brief(json.loads(response.read().decode("utf-8") or "{}")))


def mkdir(args):
    out(brief((rpc("/2/files/create_folder_v2", {"path": dropbox_path(args.path), "autorename": args.autorename}) or {}).get("metadata", {})))


def relocate(route):
    def run(args):
        moved = rpc(route, {"from_path": dropbox_path(args.source), "to_path": dropbox_path(args.dest), "autorename": args.autorename})
        out(brief((moved or {}).get("metadata", {})))
    return run


def delete(args):
    gone = rpc("/2/files/delete_v2", {"path": dropbox_path(args.path)}) or {}
    out({"status": "deleted", "metadata": brief(gone.get("metadata", {}))})


def link(args):
    answer = rpc("/2/files/get_temporary_link", {"path": dropbox_path(args.path)}) or {}
    out({"link": answer.get("link"), "expires": "in four hours", "metadata": brief(answer.get("metadata") or {})})


def raw(args):
    if not ROUTE.match(args.route):
        raise DropboxError(0, "bad_path", "an RPC route on api.dropboxapi.com, from /2/ (e.g. /2/files/list_revisions)")
    try:
        body = json.loads(args.json) if args.json else None
    except ValueError as exc:
        raise DropboxError(0, "bad_json", f"--json is not JSON: {exc}") from None
    out(rpc(args.route, body))


def parser():
    top = argparse.ArgumentParser(description="Dropbox through the person's connected Dropbox account")
    top.add_argument("--root", help="a team space's root namespace id (check names it): paths start there, not in the member's own folder")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the Dropbox connection answers for this agent").set_defaults(func=check)
    p = sub.add_parser("list", help="a folder's entries")
    p.add_argument("path", nargs="?", default="", help="a folder: / or empty for the root")
    p.add_argument("--recursive", action="store_true")
    p.add_argument("--max", type=int, default=100)
    p.add_argument("--cursor", help="the cursor of an answer with has_more, for its next page")
    p.set_defaults(func=list_folder)
    p = sub.add_parser("search", help="files and folders by name and content")
    p.add_argument("query", nargs="?", default="")
    p.add_argument("--path", help="a folder to search under")
    p.add_argument("--ext", action="append", help="a file extension, e.g. pdf (more than once for several)")
    p.add_argument("--filename-only", action="store_true")
    p.add_argument("--max", type=int, default=25)
    p.add_argument("--cursor", help="the cursor of an answer with has_more, for its next page")
    p.set_defaults(func=search)
    p = sub.add_parser("get", help="a file's or folder's metadata")
    p.add_argument("path")
    p.set_defaults(func=get)
    p = sub.add_parser("download", help="a file, saved to a local file")
    p.add_argument("path")
    p.add_argument("--to", help="the local file (default: its name, here)")
    p.add_argument("--overwrite", action="store_true", help="replace the local file if it exists")
    p.set_defaults(func=download)
    p = sub.add_parser("upload", help="a local file of at most 150 MiB")
    p.add_argument("file")
    p.add_argument("--to", required=True, help="its path in Dropbox, e.g. /Work/notes.txt")
    p.add_argument("--mode", choices=["add", "overwrite", "update"], default="add", help="add never replaces a file; overwrite does; update replaces --rev alone")
    p.add_argument("--rev", help="with --mode update: the revision it replaces")
    p.add_argument("--autorename", action="store_true", help="on a conflict, Dropbox picks another name")
    p.set_defaults(func=upload)
    p = sub.add_parser("mkdir", help="a new folder")
    p.add_argument("path")
    p.add_argument("--autorename", action="store_true")
    p.set_defaults(func=mkdir)
    for name, route in [("move", "/2/files/move_v2"), ("copy", "/2/files/copy_v2")]:
        p = sub.add_parser(name)
        p.add_argument("source")
        p.add_argument("dest")
        p.add_argument("--autorename", action="store_true")
        p.set_defaults(func=relocate(route))
    p = sub.add_parser("delete", help="a file or folder, with all it holds")
    p.add_argument("path")
    p.set_defaults(func=delete)
    p = sub.add_parser("link", help="a temporary link to a file's content (four hours, no token)")
    p.add_argument("path")
    p.set_defaults(func=link)
    p = sub.add_parser("request", help="any RPC call on api.dropboxapi.com")
    p.add_argument("method", choices=["POST", "post"], help="Dropbox's RPC routes are all POST")
    p.add_argument("route", help="from /2/, e.g. /2/files/list_revisions")
    p.add_argument("--json", default="", help="the call's JSON arguments (none for a route that takes none)")
    p.set_defaults(func=raw)
    return top


READ_ONLY = "the connection may not change files: connected read only (the person allows changes in Settings, under Connections)"
HINTS = {
    "not_connected": "the person has not connected Dropbox (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left Dropbox out",
    "invalid_access_token": "Dropbox refused the connection's token: ask the person to reconnect Dropbox in Settings, under Connections",
    "expired_access_token": "Dropbox refused the connection's token: ask the person to reconnect Dropbox in Settings, under Connections",
    "missing_scope": READ_ONLY,
    "too_many_requests": "Dropbox is limiting requests: wait, then try once more",
    "invalid_root": "the account's root changed (it joined or left a team): run check, and use its root_namespace_id",
}


def main():
    args = parser().parse_args()
    try:
        if args.root is not None and not NAMESPACE.match(args.root):
            raise DropboxError(0, "bad_root", f"{args.root!r} is not a namespace id (digits; check names the root's)")
        ROOT["namespace"] = args.root
        args.func(args)
    except DropboxError as e:
        print(json.dumps({"error": e.code or "dropbox_error", "status": e.status, "message": str(e), "hint": HINTS.get(e.code.split("/")[0], "")}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
