#!/usr/bin/env python3
"""Box through the person's connected Box account.

The computer holds no Box credential: its environment holds the connection's
placeholder, which names the agent (`BOX_OAUTH_ACCESS_TOKEN`), each request
sends it as a bearer token to api.box.com (or, for an upload, upload.box.com),
and the computer's intercept swaps in a short-lived token for the agent's
owner (docs/computers.md, Connections and operator keys). A download's bytes
are on another host (dl.boxcloud.com), fetched with no token. Python's
standard library only; every command prints JSON.

Usage:
  python box_api.py check
  python box_api.py folders list [FOLDER_ID] [--limit 100] [--marker M]
  python box_api.py folders get FOLDER_ID
  python box_api.py folders create NAME [--parent 0]
  python box_api.py search "quarterly plan" [--type file] [--ext pdf,docx] [--in FOLDER_ID] [--limit 30] [--offset 0]
  python box_api.py files get FILE_ID
  python box_api.py files download FILE_ID [--out PATH] [--overwrite]
  python box_api.py files upload LOCAL_PATH [--folder 0] [--name NAME]
  python box_api.py move file|folder ID --to FOLDER_ID
  python box_api.py copy file|folder ID --to FOLDER_ID [--name NAME]
  python box_api.py comments list FILE_ID
  python box_api.py comments add FILE_ID --message "…"
  python box_api.py share get file|folder ID
  python box_api.py share create file|folder ID [--access open|company|collaborators]
  python box_api.py share remove file|folder ID
  python box_api.py request GET /2.0/files/123/versions [--json '{…}']

Environment:
  BOX_OAUTH_ACCESS_TOKEN  the Box connection's placeholder (set on a fragment
                          computer once the person connects Box), or any Box
                          OAuth access token
"""

import argparse
import http.client
import json
import os
import re
import secrets
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.box.com"
UPLOAD = "https://upload.box.com/api"
TIMEOUT_S = 60
# a folder's items page at most 1,000, a search's 200 (Box's own limits)
ITEMS_MAX = 1000
SEARCH_MAX = 200
# Box's direct upload takes a file of at most 50 MB; larger ones need its
# chunked upload
UPLOAD_MAX_BYTES = 50_000_000
CHUNK_BYTES = 1 << 20
ITEM_FIELDS = "type,id,name,size,modified_at"
SEARCH_FIELDS = "type,id,name,size,modified_at,parent"
COMMENT_FIELDS = "message,created_by,created_at,is_reply_comment"
# Box names every file, folder and comment by a number; 0 is the root folder
ID = re.compile(r"^[0-9]{1,20}$")
EXTENSION = re.compile(r"^[A-Za-z0-9]{1,16}$")
PATH = re.compile(r"^/2\.0/[A-Za-z0-9/_\-.~%:@!$&'()*+,;=]*$")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points (a download's goes to another host)."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


# the API's, with the token; a download's bytes, with none
OPENER = urllib.request.build_opener(NoRedirect)
DOWNLOADS = urllib.request.build_opener(NoRedirect)


class BoxError(Exception):
    def __init__(self, status, code, message, ours=True, location=None):
        super().__init__(message)
        self.status = status
        self.code = code
        # the helper's or the computer's refusal, not Box's
        self.ours = ours
        self.location = location


def token():
    value = os.getenv("BOX_OAUTH_ACCESS_TOKEN", "").strip()
    if not value:
        raise BoxError(0, "not_connected", "BOX_OAUTH_ACCESS_TOKEN is not set: the person has not connected Box, or narrowed you from it")
    return value


def send(method, url, data=None, content_type=None):
    """`method` `url` with the placeholder as a bearer token: its open
    answer. A redirect is raised, never followed."""
    headers = {"Authorization": f"Bearer {token()}", "Accept": "application/json"}
    if content_type:
        headers["Content-Type"] = content_type
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        return OPENER.open(request, timeout=TIMEOUT_S)
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            where = exc.headers.get("Location")
            raise BoxError(exc.code, "redirected", f"Box answered a redirect to {where or '?'}, which the helper never follows with the token", location=where) from None
        raise refusal(exc.code, exc.headers, exc.read().decode("utf-8", errors="replace")) from None
    except urllib.error.URLError as exc:
        raise BoxError(0, "unreachable", str(exc.reason)) from None


def api_path(path):
    """A path on Box's API, from /2.0/: no `.` or `..` segment, no other host."""
    segments = [urllib.parse.unquote(seg) for seg in path.split("/")]
    if not PATH.match(path) or "//" in path or any(seg in (".", "..") or "/" in seg for seg in segments):
        raise BoxError(0, "bad_path", "a path on Box's API, from /2.0/ (e.g. /2.0/folders/0/items)")
    return path


def call(method, path, body=None, query=None):
    """`method` `path` (from /2.0/) on api.box.com, its answer read as JSON
    (none for 204)."""
    url = API + api_path(path) + (("?" + urllib.parse.urlencode(query, quote_via=urllib.parse.quote)) if query else "")
    data = None if body is None else json.dumps(body).encode("utf-8")
    with send(method, url, data, "application/json" if data is not None else None) as response:
        text = response.read().decode("utf-8")
    return json.loads(text) if text.strip() else {}


def refusal(status, headers, text):
    """An HTTP refusal: the computer's own (`{error: "not_connected" |
    "forbidden", message}`), Box's (`{type: "error", code, message}`), or
    Box's word in `WWW-Authenticate` (`error="invalid_token"`)."""
    try:
        err = json.loads(text)
    except ValueError:
        err = None
    if isinstance(err, dict) and err.get("type") == "error":
        return BoxError(status, str(err.get("code") or ""), err.get("message") or "Box refused the request", ours=False)
    if isinstance(err, dict) and isinstance(err.get("error"), str):
        return BoxError(status, err["error"], err.get("message") or err.get("error_description") or err["error"])
    challenge = (headers or {}).get("WWW-Authenticate") or ""
    said = re.search(r'error="([^"]+)"', challenge)
    if said:
        described = re.search(r'error_description="([^"]*)"', challenge)
        return BoxError(status, said.group(1), (described.group(1) if described else "") or f"HTTP {status}", ours=False)
    return BoxError(status, "", (text or "")[:500] or f"HTTP {status}", ours=False)


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def item_id(value, what="id"):
    if not ID.match(value or ""):
        raise BoxError(0, "bad_id", f"{value!r} is not a Box {what}: a number (0 is the root folder)")
    return value


def item_name(name):
    """A name Box takes: 1 to 255 characters, no slash, no control
    character, no trailing space, never `.` or `..`."""
    bad = not (1 <= len(name) <= 255) or name in (".", "..") or name != name.rstrip() or any(c in "/\\" or ord(c) < 32 or ord(c) == 127 for c in name)
    if bad:
        raise BoxError(0, "bad_name", f"{name!r} is not a name Box takes (no slash, control character or trailing space; not . or ..)")
    return name


def bounded(n, most):
    return max(1, min(n, most))


def plural(kind):
    return {"file": "files", "folder": "folders"}[kind]


def check(_args):
    me = call("GET", "/2.0/users/me", query={"fields": "id,name,login"})
    out({"status": "connected", "id": me.get("id"), "name": me.get("name"), "login": me.get("login")})


def folders_list(args):
    query = {"fields": ITEM_FIELDS, "usemarker": "true", "limit": bounded(args.limit, ITEMS_MAX)}
    if args.marker:
        query["marker"] = args.marker
    page = call("GET", f"/2.0/folders/{item_id(args.folder, 'folder id')}/items", query=query)
    out({"entries": page.get("entries", []), "next_marker": page.get("next_marker") or None})


def folders_get(args):
    out(call("GET", f"/2.0/folders/{item_id(args.folder, 'folder id')}"))


def folders_create(args):
    body = {"name": item_name(args.name), "parent": {"id": item_id(args.parent, "folder id")}}
    out(call("POST", "/2.0/folders", body))


def search(args):
    term = args.term.strip()
    if not term:
        raise BoxError(0, "bad_search", "say what to search for")
    query = {"query": term, "fields": SEARCH_FIELDS, "limit": bounded(args.limit, SEARCH_MAX), "offset": max(0, args.offset)}
    if args.type:
        query["type"] = args.type
    if args.ext:
        exts = [e.strip().lstrip(".") for e in args.ext.split(",") if e.strip()]
        if not exts or not all(EXTENSION.match(e) for e in exts):
            raise BoxError(0, "bad_extension", f"{args.ext!r} is not a list of file extensions (e.g. pdf,docx)")
        query["file_extensions"] = ",".join(exts)
    if args.within:
        query["ancestor_folder_ids"] = item_id(args.within, "folder id")
    found = call("GET", "/2.0/search", query=query)
    out({"total_count": found.get("total_count"), "offset": found.get("offset"), "entries": found.get("entries", [])})


def files_get(args):
    out(call("GET", f"/2.0/files/{item_id(args.file, 'file id')}"))


def local_name(name):
    """A Box file's name as a local file's, in the folder it goes to."""
    if os.path.basename(name) != name or name in ("", ".", ".."):
        raise BoxError(0, "bad_name", f"the file's name {name!r} is no local file's: name one with --out")
    return name


def writable(target, overwrite):
    """A local path a download may write: in a folder that exists, and no
    file there already unless it may be overwritten."""
    if os.path.exists(target) and not overwrite:
        raise BoxError(0, "exists", f"{target} exists: name another with --out, or pass --overwrite")
    if not os.path.isdir(os.path.dirname(target) or "."):
        raise BoxError(0, "bad_out", f"{target}: no such folder")
    return target


def destination(args, file):
    """Where a download goes: --out, or, where --out is a folder or unnamed,
    the file's own name there (or here)."""
    if args.out and not os.path.isdir(args.out):
        return args.out
    name = call("GET", f"/2.0/files/{file}", query={"fields": "name"}).get("name") or ""
    return writable(os.path.join(args.out or "", local_name(name)), args.overwrite)


def download_url(file):
    """Where Box serves a file's bytes: its redirect's `Location` (Box's
    download URL, needing no token)."""
    try:
        with send("GET", f"{API}/2.0/files/{file}/content") as response:
            if response.status == 202:
                wait = response.headers.get("Retry-After") or "a few"
                raise BoxError(202, "not_ready", f"Box is still preparing the file: download it again in {wait} seconds")
            raise BoxError(response.status, "no_download_url", "Box answered the download without its redirect to the file's bytes")
    except BoxError as e:
        if e.code != "redirected":
            raise
        where = urllib.parse.urlsplit(e.location or "")
        if where.scheme != "https" or not where.hostname:
            raise BoxError(e.status, "redirected", f"Box's download URL {e.location!r} is no https URL") from None
        return e.location


def save(url, target, overwrite):
    """The bytes at `url`, fetched with no token, written to `target` (none
    of them left there should the download fail)."""
    request = urllib.request.Request(url, headers={"Accept": "*/*"}, method="GET")
    try:
        with DOWNLOADS.open(request, timeout=TIMEOUT_S) as response:
            return write(response, target, overwrite)
    except urllib.error.HTTPError as exc:
        raise BoxError(exc.code, "download_failed", f"Box's download URL answered {exc.code}") from None
    except urllib.error.URLError as exc:
        raise BoxError(0, "unreachable", str(exc.reason)) from None
    except (OSError, http.client.HTTPException) as exc:
        raise BoxError(0, "download_failed", f"the download broke off: {exc}") from None


def write(response, target, overwrite):
    try:
        f = open(target, "wb" if overwrite else "xb")
    except OSError as exc:
        raise BoxError(0, "bad_out", f"{target}: {exc.strerror or exc}") from None
    written = 0
    try:
        with f:
            while chunk := response.read(CHUNK_BYTES):
                f.write(chunk)
                written += len(chunk)
    except BaseException:
        os.remove(target)
        raise
    return written


def files_download(args):
    file = item_id(args.file, "file id")
    if args.out and not os.path.isdir(args.out):
        writable(args.out, args.overwrite)
    target = destination(args, file)
    written = save(download_url(file), target, args.overwrite)
    out({"status": "downloaded", "id": file, "path": target, "bytes": written})


def multipart(attributes, name, content):
    """Box's upload body: its `attributes` part first, then the file's."""
    boundary = "box-finite-" + secrets.token_hex(16)
    head = (
        f"--{boundary}\r\nContent-Disposition: form-data; name=\"attributes\"\r\n\r\n{json.dumps(attributes)}\r\n"
        f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{urllib.parse.quote(name)}\"\r\n"
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode("utf-8")
    return boundary, head + content + f"\r\n--{boundary}--\r\n".encode("utf-8")


def files_upload(args):
    if not os.path.isfile(args.path):
        raise BoxError(0, "no_such_file", f"{args.path} is no file")
    size = os.path.getsize(args.path)
    if size > UPLOAD_MAX_BYTES:
        raise BoxError(0, "too_large", f"{args.path} is {size} bytes: the helper uploads files of at most 50 MB (Box's chunked upload takes larger ones)")
    name = item_name(args.name or os.path.basename(args.path))
    attributes = {"name": name, "parent": {"id": item_id(args.folder, "folder id")}}
    with open(args.path, "rb") as f:
        content = f.read()
    boundary, body = multipart(attributes, name, content)
    with send("POST", f"{UPLOAD}/2.0/files/content", body, f"multipart/form-data; boundary={boundary}") as response:
        made = json.loads(response.read().decode("utf-8") or "{}")
    entries = made.get("entries") or [{}]
    out(entries[0])


def move(args):
    path = f"/2.0/{plural(args.kind)}/{item_id(args.id)}"
    out(call("PUT", path, {"parent": {"id": item_id(args.to, "folder id")}}))


def copy(args):
    body = {"parent": {"id": item_id(args.to, "folder id")}}
    if args.name:
        body["name"] = item_name(args.name)
    out(call("POST", f"/2.0/{plural(args.kind)}/{item_id(args.id)}/copy", body))


def comments_list(args):
    page = call("GET", f"/2.0/files/{item_id(args.file, 'file id')}/comments", query={"fields": COMMENT_FIELDS, "limit": 100})
    out(page.get("entries", []))


def comments_add(args):
    if not args.message.strip():
        raise BoxError(0, "bad_message", "say what the comment says")
    out(call("POST", "/2.0/comments", {"item": {"type": "file", "id": item_id(args.file, "file id")}, "message": args.message}))


def share_get(args):
    item = call("GET", f"/2.0/{plural(args.kind)}/{item_id(args.id)}", query={"fields": "shared_link"})
    out(item.get("shared_link"))


def share_create(args):
    link = {"access": args.access} if args.access else {}
    item = call("PUT", f"/2.0/{plural(args.kind)}/{item_id(args.id)}", {"shared_link": link}, query={"fields": "shared_link"})
    out(item.get("shared_link"))


def share_remove(args):
    call("PUT", f"/2.0/{plural(args.kind)}/{item_id(args.id)}", {"shared_link": None}, query={"fields": "shared_link"})
    out({"status": "removed", "id": args.id})


def raw(args):
    try:
        body = json.loads(args.json) if args.json else None
    except ValueError as exc:
        raise BoxError(0, "bad_json", f"--json is not JSON: {exc}") from None
    path, _, query = args.path.partition("?")
    out(call(args.method.upper(), path, body, dict(urllib.parse.parse_qsl(query)) if query else None))


def kind_and_id(p):
    p.add_argument("kind", choices=["file", "folder"])
    p.add_argument("id")


def folder_parsers(sub):
    folders = sub.add_parser("folders").add_subparsers(dest="action", required=True)
    p = folders.add_parser("list", help="a folder's items, a page at a time")
    p.add_argument("folder", nargs="?", default="0", help="its id (0, the default, is All Files)")
    p.add_argument("--limit", type=int, default=100)
    p.add_argument("--marker", help="the previous page's next_marker")
    p.set_defaults(func=folders_list)
    p = folders.add_parser("get")
    p.add_argument("folder")
    p.set_defaults(func=folders_get)
    p = folders.add_parser("create")
    p.add_argument("name")
    p.add_argument("--parent", default="0", help="the folder it goes in (0 is All Files)")
    p.set_defaults(func=folders_create)


def file_parsers(sub):
    files = sub.add_parser("files").add_subparsers(dest="action", required=True)
    p = files.add_parser("get")
    p.add_argument("file")
    p.set_defaults(func=files_get)
    p = files.add_parser("download")
    p.add_argument("file")
    p.add_argument("--out", help="a local path, or a folder (the file's own name, here, by default)")
    p.add_argument("--overwrite", action="store_true")
    p.set_defaults(func=files_download)
    p = files.add_parser("upload")
    p.add_argument("path", help="a local file of at most 50 MB")
    p.add_argument("--folder", default="0", help="the folder it goes in (0 is All Files)")
    p.add_argument("--name", help="its name in Box (its own, by default)")
    p.set_defaults(func=files_upload)


def change_parsers(sub):
    p = sub.add_parser("move")
    kind_and_id(p)
    p.add_argument("--to", required=True, help="the folder it goes in")
    p.set_defaults(func=move)
    p = sub.add_parser("copy")
    kind_and_id(p)
    p.add_argument("--to", required=True, help="the folder the copy goes in")
    p.add_argument("--name", help="the copy's name")
    p.set_defaults(func=copy)
    comments = sub.add_parser("comments").add_subparsers(dest="action", required=True)
    p = comments.add_parser("list")
    p.add_argument("file")
    p.set_defaults(func=comments_list)
    p = comments.add_parser("add")
    p.add_argument("file")
    p.add_argument("--message", required=True)
    p.set_defaults(func=comments_add)
    share = sub.add_parser("share").add_subparsers(dest="action", required=True)
    for action, func in (("get", share_get), ("create", share_create), ("remove", share_remove)):
        p = share.add_parser(action)
        kind_and_id(p)
        if action == "create":
            p.add_argument("--access", choices=["open", "company", "collaborators"], help="who may open it (the account's default, unnamed)")
        p.set_defaults(func=func)


def parser():
    top = argparse.ArgumentParser(description="Box through the person's connected Box account")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the Box connection answers for this agent").set_defaults(func=check)
    folder_parsers(sub)
    p = sub.add_parser("search")
    p.add_argument("term")
    p.add_argument("--type", choices=["file", "folder", "web_link"])
    p.add_argument("--ext", help="file extensions, e.g. pdf,docx")
    p.add_argument("--in", dest="within", help="only under this folder")
    p.add_argument("--limit", type=int, default=30)
    p.add_argument("--offset", type=int, default=0)
    p.set_defaults(func=search)
    file_parsers(sub)
    change_parsers(sub)
    p = sub.add_parser("request", help="any Box API call")
    p.add_argument("method", choices=["GET", "POST", "PUT", "DELETE", "get", "post", "put", "delete"])
    p.add_argument("path", help="from /2.0/, its query after ?")
    p.add_argument("--json", default="", help="the request's JSON body")
    p.set_defaults(func=raw)
    return top


READ_ONLY = "connected read only: Box lets only read and write change anything or download a file's contents. The person allows changes in Settings, under Connections"
# the computer's refusals, and the helper's
HINTS = {
    "not_connected": "the person has not connected Box (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left Box out",
    "redirected": "Box's redirect is never followed with the token: download a file with `files download`",
    "not_ready": "Box is still preparing the file: download it again shortly",
}
# Box's own
BOX_HINTS = {
    "invalid_token": "Box refused the connection's token: ask the person to reconnect Box in Settings, under Connections",
    "unauthorized": "Box refused the connection's token: ask the person to reconnect Box in Settings, under Connections",
    "insufficient_scope": READ_ONLY,
    "access_denied_insufficient_permissions": "the connected Box user may not do this to this item, or " + READ_ONLY,
    "not_found": "the id is wrong, or the connected Box user cannot see it",
    "item_name_in_use": "the folder holds an item of that name already: pick another",
    "rate_limit_exceeded": "Box limits how fast you ask: wait, then ask again",
}


def main():
    args = parser().parse_args()
    try:
        args.func(args)
    except BoxError as e:
        hint = (HINTS if e.ours else BOX_HINTS).get(e.code, "")
        print(json.dumps({"error": e.code or "box_error", "status": e.status, "message": str(e), "hint": hint}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
