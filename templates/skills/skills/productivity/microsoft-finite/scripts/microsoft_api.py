#!/usr/bin/env python3
"""Microsoft 365 through the person's connected Microsoft account.

The computer holds no Microsoft credential: its environment holds the
connection's placeholder, which names the agent
(`MICROSOFT_OAUTH_ACCESS_TOKEN`), each request sends it as a bearer token to
graph.microsoft.com, and the computer's intercept swaps in a short-lived token
for the agent's owner (docs/computers.md, Connections and operator keys).
Python's standard library only; every command prints JSON.

Usage:
  python microsoft_api.py check
  python microsoft_api.py mail list [--folder inbox] [--unread] [--max 25]
  python microsoft_api.py mail search "invoice from acme" [--max 25]
  python microsoft_api.py mail get MESSAGE_ID
  python microsoft_api.py mail send --to a@example.com [--to …] [--cc …] --subject "…" --body "…" [--html]
  python microsoft_api.py events list [--days 7] [--max 50]
  python microsoft_api.py events create --subject "…" --start 2026-11-12T15:00 --end 2026-11-12T15:30 [--tz "Eastern Standard Time"] [--attendee a@example.com …] [--body "…"]
  python microsoft_api.py files list [PATH] [--max 50]
  python microsoft_api.py files search "quarterly plan" [--max 25]
  python microsoft_api.py files get ITEM_ID
  python microsoft_api.py contacts list [--max 50]
  python microsoft_api.py request GET /v1.0/me/mailFolders [--json '{…}']

Environment:
  MICROSOFT_OAUTH_ACCESS_TOKEN  the Microsoft connection's placeholder (set on
                                a fragment computer once the person connects
                                Microsoft), or any Microsoft Graph token
"""

import argparse
import datetime
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

GRAPH = "https://graph.microsoft.com"
TIMEOUT_S = 60
# Graph pages at most 999 items; a helper's lists stay small
MAX_ITEMS = 100
MESSAGE_FIELDS = "id,subject,from,toRecipients,receivedDateTime,isRead,bodyPreview,webLink"
EVENT_FIELDS = "id,subject,start,end,location,organizer,attendees,webLink"
FILE_FIELDS = "id,name,size,lastModifiedDateTime,webUrl,folder,file,parentReference"
# an id Graph names: letters, digits and its few punctuation marks
ID = re.compile(r"^[A-Za-z0-9=_\-!.]{1,512}$")
# a well-known folder's name, or a folder's id
FOLDER = re.compile(r"^[A-Za-z0-9=_\-!.]{1,512}$")
EMAIL = re.compile(r"^[^@\s]{1,64}@[^@\s]{1,255}$")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points (a file's download goes to another host)."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


class GraphError(Exception):
    def __init__(self, status, code, message):
        super().__init__(message)
        self.status = status
        self.code = code


def token():
    value = os.getenv("MICROSOFT_OAUTH_ACCESS_TOKEN", "").strip()
    if not value:
        raise GraphError(0, "not_connected", "MICROSOFT_OAUTH_ACCESS_TOKEN is not set: the person has not connected Microsoft, or narrowed you from it")
    return value


def call(method, path, body=None, query=None):
    """`method` `path` (from /v1.0/ or /beta/) on Microsoft Graph, its answer
    read as JSON (none for 202 or 204)."""
    if not re.match(r"^/(v1\.0|beta)/[A-Za-z0-9/_\-.:'()=,!$@~%]*$", path) or any(seg in (".", "..") for seg in path.split("/")):
        raise GraphError(0, "bad_path", "a path on Microsoft Graph, from /v1.0/ or /beta/ (e.g. /v1.0/me/mailFolders)")
    url = GRAPH + path + (("?" + urllib.parse.urlencode(query, quote_via=urllib.parse.quote)) if query else "")
    data = None if body is None else json.dumps(body).encode("utf-8")
    headers = {"Authorization": f"Bearer {token()}", "Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with OPENER.open(request, timeout=TIMEOUT_S) as response:
            text = response.read().decode("utf-8")
            return json.loads(text) if text.strip() else {}
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise GraphError(exc.code, "redirected", f"Microsoft answered a redirect to {exc.headers.get('Location', '?')}, which the helper never follows with the token") from None
        raise refusal(exc.code, exc.read().decode("utf-8", errors="replace")) from None
    except urllib.error.URLError as exc:
        raise GraphError(0, "unreachable", str(exc.reason)) from None


def refusal(status, text):
    """An HTTP refusal: the computer's own (`{error: "not_connected" |
    "forbidden", message}`), or Graph's (`{error: {code, message}}`)."""
    try:
        err = json.loads(text)
    except ValueError:
        err = {}
    if isinstance(err, dict) and isinstance(err.get("error"), str):
        return GraphError(status, err["error"], err.get("message") or err["error"])
    if isinstance(err, dict) and isinstance(err.get("error"), dict):
        e = err["error"]
        return GraphError(status, str(e.get("code") or "graph_error").lower(), e.get("message") or "Microsoft refused the request")
    return GraphError(status, "", text[:500] or f"HTTP {status}")


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def bounded(n):
    return max(1, min(n, MAX_ITEMS))


def checked(value, pattern, what):
    if not pattern.match(value or ""):
        raise GraphError(0, "bad_" + what, f"{value!r} is not a {what}")
    return value


def check(_args):
    me = call("GET", "/v1.0/me", query={"$select": "displayName,mail,userPrincipalName"})
    out({"status": "connected", "name": me.get("displayName"), "email": me.get("mail") or me.get("userPrincipalName")})


def mail_list(args):
    folder = checked(args.folder, FOLDER, "folder")
    query = {"$top": bounded(args.max), "$select": MESSAGE_FIELDS, "$orderby": "receivedDateTime desc"}
    if args.unread:
        # Graph takes $orderby beside $filter only where the ordered property
        # comes first in the filter (else InefficientFilter)
        query["$filter"] = "receivedDateTime ge 1900-01-01T00:00:00Z and isRead eq false"
    out(call("GET", f"/v1.0/me/mailFolders/{folder}/messages", query=query).get("value", []))


def mail_search(args):
    term = args.term.replace('"', " ").strip()
    if not term:
        raise GraphError(0, "bad_search", "say what to search for")
    out(call("GET", "/v1.0/me/messages", query={"$search": f'"{term}"', "$top": bounded(args.max), "$select": MESSAGE_FIELDS}).get("value", []))


def mail_get(args):
    out(call("GET", f"/v1.0/me/messages/{checked(args.id, ID, 'message_id')}", query={"$select": MESSAGE_FIELDS + ",body,ccRecipients"}))


def recipients(addresses):
    return [{"emailAddress": {"address": checked(a, EMAIL, "email")}} for a in addresses or []]


def mail_send(args):
    if not args.to:
        raise GraphError(0, "bad_email", "name at least one --to")
    message = {"subject": args.subject, "body": {"contentType": "HTML" if args.html else "Text", "content": args.body}, "toRecipients": recipients(args.to), "ccRecipients": recipients(args.cc)}
    call("POST", "/v1.0/me/sendMail", {"message": message, "saveToSentItems": True})
    out({"status": "sent", "to": args.to, "cc": args.cc or []})


def events_list(args):
    now = datetime.datetime.now(datetime.timezone.utc)
    until = now + datetime.timedelta(days=max(1, min(args.days, 90)))
    query = {"startDateTime": now.strftime("%Y-%m-%dT%H:%M:%SZ"), "endDateTime": until.strftime("%Y-%m-%dT%H:%M:%SZ"), "$top": bounded(args.max), "$select": EVENT_FIELDS, "$orderby": "start/dateTime"}
    out(call("GET", "/v1.0/me/calendarView", query=query).get("value", []))


def local_time(value, what):
    try:
        datetime.datetime.fromisoformat(value)
    except ValueError:
        raise GraphError(0, "bad_time", f"--{what} is a local date and time, e.g. 2026-11-12T15:00") from None
    return value


def events_create(args):
    event = {
        "subject": args.subject,
        "start": {"dateTime": local_time(args.start, "start"), "timeZone": args.tz},
        "end": {"dateTime": local_time(args.end, "end"), "timeZone": args.tz},
        "attendees": [{"emailAddress": {"address": checked(a, EMAIL, "email")}, "type": "required"} for a in args.attendee or []],
    }
    if args.body:
        event["body"] = {"contentType": "Text", "content": args.body}
    made = call("POST", "/v1.0/me/events", event)
    out({key: made.get(key) for key in ["id", "subject", "start", "end", "webLink"]})


def drive_path(path):
    parts = [p for p in (path or "").split("/") if p]
    if any(p in (".", "..") for p in parts):
        raise GraphError(0, "bad_path", "a path in your OneDrive, with no . or .. in it")
    return "/".join(urllib.parse.quote(p, safe="") for p in parts)


def files_list(args):
    path = drive_path(args.path)
    where = f"/v1.0/me/drive/root:/{path}:/children" if path else "/v1.0/me/drive/root/children"
    out(call("GET", where, query={"$top": bounded(args.max), "$select": FILE_FIELDS}).get("value", []))


def files_search(args):
    term = args.term.replace("'", "''")
    out(call("GET", f"/v1.0/me/drive/root/search(q='{urllib.parse.quote(term, safe='')}')", query={"$top": bounded(args.max), "$select": FILE_FIELDS}).get("value", []))


def files_get(args):
    out(call("GET", f"/v1.0/me/drive/items/{checked(args.id, ID, 'item_id')}", query={"$select": FILE_FIELDS}))


def contacts_list(args):
    out(call("GET", "/v1.0/me/contacts", query={"$top": bounded(args.max), "$select": "id,displayName,emailAddresses,mobilePhone,companyName"}).get("value", []))


def raw(args):
    try:
        body = json.loads(args.json) if args.json else None
    except ValueError as exc:
        raise GraphError(0, "bad_json", f"--json is not JSON: {exc}") from None
    path, _, query = args.path.partition("?")
    out(call(args.method.upper(), path, body, dict(urllib.parse.parse_qsl(query)) if query else None))


def parser():
    top = argparse.ArgumentParser(description="Microsoft 365 through the person's connected Microsoft account")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the Microsoft connection answers for this agent").set_defaults(func=check)

    mail = sub.add_parser("mail").add_subparsers(dest="action", required=True)
    p = mail.add_parser("list")
    p.add_argument("--folder", default="inbox", help="a well-known folder (inbox, sentitems, drafts, archive) or a folder's id")
    p.add_argument("--unread", action="store_true")
    p.add_argument("--max", type=int, default=25)
    p.set_defaults(func=mail_list)
    p = mail.add_parser("search")
    p.add_argument("term")
    p.add_argument("--max", type=int, default=25)
    p.set_defaults(func=mail_search)
    p = mail.add_parser("get")
    p.add_argument("id")
    p.set_defaults(func=mail_get)
    p = mail.add_parser("send")
    p.add_argument("--to", action="append", required=True)
    p.add_argument("--cc", action="append")
    p.add_argument("--subject", required=True)
    p.add_argument("--body", required=True)
    p.add_argument("--html", action="store_true", help="the body is HTML, not plain text")
    p.set_defaults(func=mail_send)

    events = sub.add_parser("events").add_subparsers(dest="action", required=True)
    p = events.add_parser("list")
    p.add_argument("--days", type=int, default=7, help="from now, up to 90")
    p.add_argument("--max", type=int, default=50)
    p.set_defaults(func=events_list)
    p = events.add_parser("create")
    p.add_argument("--subject", required=True)
    p.add_argument("--start", required=True, help="local date and time, e.g. 2026-11-12T15:00")
    p.add_argument("--end", required=True)
    p.add_argument("--tz", default="UTC", help="a Windows or IANA time zone, e.g. 'Eastern Standard Time'")
    p.add_argument("--attendee", action="append")
    p.add_argument("--body")
    p.set_defaults(func=events_create)

    files = sub.add_parser("files").add_subparsers(dest="action", required=True)
    p = files.add_parser("list")
    p.add_argument("path", nargs="?", default="", help="a folder's path in your OneDrive, e.g. Documents/Plans")
    p.add_argument("--max", type=int, default=50)
    p.set_defaults(func=files_list)
    p = files.add_parser("search")
    p.add_argument("term")
    p.add_argument("--max", type=int, default=25)
    p.set_defaults(func=files_search)
    p = files.add_parser("get")
    p.add_argument("id")
    p.set_defaults(func=files_get)

    contacts = sub.add_parser("contacts").add_subparsers(dest="action", required=True)
    p = contacts.add_parser("list")
    p.add_argument("--max", type=int, default=50)
    p.set_defaults(func=contacts_list)

    p = sub.add_parser("request", help="any Microsoft Graph call")
    p.add_argument("method", choices=["GET", "POST", "PATCH", "PUT", "DELETE", "get", "post", "patch", "put", "delete"])
    p.add_argument("path", help="from /v1.0/ or /beta/, its query after ?")
    p.add_argument("--json", default="", help="the request's JSON body")
    p.set_defaults(func=raw)
    return top


HINTS = {
    "not_connected": "the person has not connected Microsoft (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left Microsoft out",
    "invalidauthenticationtoken": "Microsoft refused the connection's token: ask the person to reconnect Microsoft in Settings, under Connections",
    "erroraccessdenied": "the connection may not do this: connected read only (the person allows changes in Settings, under Connections), or their account or its admin does not allow it",
    "accessdenied": "the connection may not do this: connected read only (the person allows changes in Settings, under Connections), or their account or its admin does not allow it",
    "authorization_requestdenied": "the connection may not do this: connected read only (the person allows changes in Settings, under Connections), or their account or its admin does not allow it",
}


def main():
    args = parser().parse_args()
    try:
        args.func(args)
    except GraphError as e:
        print(json.dumps({"error": e.code or "graph_error", "status": e.status, "message": str(e), "hint": HINTS.get(e.code, "")}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
