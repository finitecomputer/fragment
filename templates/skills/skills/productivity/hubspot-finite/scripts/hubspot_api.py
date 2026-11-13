#!/usr/bin/env python3
"""HubSpot through the person's connected HubSpot account.

The computer holds no HubSpot credential: its environment holds the
connection's placeholder, which names the agent (`HUBSPOT_OAUTH_ACCESS_TOKEN`),
each request sends it as a bearer token to api.hubapi.com, and the computer's
intercept swaps in a short-lived token for the agent's owner's account
(docs/computers.md, Connections and operator keys). The calls are HubSpot's
2026-09 API (its date-based versions; the v1 to v4 paths go unsupported in
2027). Python's standard library only; every command prints JSON.

Usage:
  python hubspot_api.py check
  python hubspot_api.py search contacts "ada lovelace" [--where email=ada@example.com …] [--properties email,firstname] [--sort lastmodifieddate [--desc]] [--max 10] [--after …]
  python hubspot_api.py get deals 123 [--properties dealname,amount] [--associations contacts,companies]
  python hubspot_api.py create contacts --set email=ada@example.com --set firstname=Ada
  python hubspot_api.py update deals 123 --set dealstage=closedwon
  python hubspot_api.py associations contacts 123 companies [--max 100] [--after …]
  python hubspot_api.py associate contacts 123 companies 456
  python hubspot_api.py notes deals 123 [--max 20]
  python hubspot_api.py note deals 123 --body "Called; they sign Friday." [--owner 41629779]
  python hubspot_api.py owners [--email ada@example.com] [--max 100] [--after …]
  python hubspot_api.py request GET "/crm/pipelines/2026-09/0-3" [--json '{…}']

Environment:
  HUBSPOT_OAUTH_ACCESS_TOKEN  the HubSpot connection's placeholder (set on a
                              fragment computer once the person connects
                              HubSpot), or any HubSpot OAuth access token
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

API = "https://api.hubapi.com"
VERSION = "2026-09"
TIMEOUT_S = 60
# HubSpot pages search at most 200 records; a helper's lists stay small
MAX_ITEMS = 100
# each object the helper names: its type id (associations' paths), its
# name in a search's `associations.<name>`, a note's association type to
# it (HubSpot-defined), and what a list or a get shows of it by default
OBJECTS = {
    "contacts": ("0-1", "contact", 202, "firstname,lastname,email,phone,company,jobtitle,lifecyclestage,hubspot_owner_id"),
    "companies": ("0-2", "company", 190, "name,domain,industry,city,country,phone,lifecyclestage,hubspot_owner_id"),
    "deals": ("0-3", "deal", 214, "dealname,amount,dealstage,pipeline,closedate,hubspot_owner_id"),
    "tickets": ("0-5", "ticket", 228, "subject,content,hs_pipeline,hs_pipeline_stage,hs_ticket_priority,hubspot_owner_id"),
}
NOTE_FIELDS = "hs_note_body,hs_timestamp,hubspot_owner_id"
# a record's and an owner's id: digits; a page's cursor, as HubSpot gives it
ID = re.compile(r"^[0-9]{1,20}$")
CURSOR = re.compile(r"^[A-Za-z0-9_\-=.]{1,256}$")
# a property's internal name (`associations.contact` only in a filter)
PROPERTY = re.compile(r"^[A-Za-z0-9_]{1,100}$")
FILTER_PROPERTY = re.compile(r"^[A-Za-z0-9_]{1,100}(\.[A-Za-z0-9_]{1,100})?$")
# a search's filter: a property, an operator, a value
WHERE = re.compile(r"^([A-Za-z0-9_.]{1,201})(!=|>=|<=|=|>|<|~)(.*)$", re.S)
OPERATORS = {"=": "EQ", "!=": "NEQ", ">": "GT", ">=": "GTE", "<": "LT", "<=": "LTE", "~": "CONTAINS_TOKEN"}
# a path on api.hubapi.com: an API's name, then its resource
PATH = re.compile(r"^/[a-z][a-z0-9-]*(/[A-Za-z0-9_\-.:,;=@~!$'()*+%]*)*$")
EMAIL = re.compile(r"^[^@\s]{1,64}@[^@\s]{1,255}$")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


class HubSpotError(Exception):
    def __init__(self, status, code, message):
        super().__init__(message)
        self.status = status
        self.code = code


def token():
    value = os.getenv("HUBSPOT_OAUTH_ACCESS_TOKEN", "").strip()
    if not value:
        raise HubSpotError(0, "not_connected", "HUBSPOT_OAUTH_ACCESS_TOKEN is not set: the person has not connected HubSpot, or narrowed you from it")
    return value


def checked_path(path):
    """`path` on api.hubapi.com, refused unless it is one: no host, no `.`
    or `..` segment and none hiding a `/` (as written or percent-encoded),
    no empty segment but a trailing slash's."""
    segments = [urllib.parse.unquote(s) for s in path.split("/")[1:]]
    if not PATH.match(path) or "" in segments[:-1] or any(s in (".", "..") or "/" in s or "\\" in s for s in segments):
        raise HubSpotError(0, "bad_path", "a path on api.hubapi.com, from its API's name (e.g. /crm/objects/2026-09/contacts)")
    return path


def call(method, path, body=None, query=None):
    """`method` `path` on HubSpot's API, its answer read as JSON (none for
    204)."""
    url = API + checked_path(path) + (("?" + urllib.parse.urlencode(query)) if query else "")
    data = None if body is None else json.dumps(body).encode("utf-8")
    headers = {"Authorization": f"Bearer {token()}", "Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with OPENER.open(request, timeout=TIMEOUT_S) as response:
            text = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise HubSpotError(exc.code, "redirected", f"HubSpot answered a redirect to {exc.headers.get('Location', '?')}, which the helper never follows with the token") from None
        raise refusal(exc.code, exc.read().decode("utf-8", errors="replace")) from None
    except urllib.error.URLError as exc:
        raise HubSpotError(0, "unreachable", str(exc.reason)) from None
    try:
        return json.loads(text) if text.strip() else {}
    except ValueError:
        raise HubSpotError(200, "bad_answer", f"HubSpot answered something other than JSON: {text[:200]}") from None


def refusal(status, text):
    """An HTTP refusal: HubSpot's (`{status: "error", message, category,
    errors}`, a rate limit's `errorType`), or the computer's own (`{error:
    "not_connected" | "forbidden", message}`)."""
    try:
        err = json.loads(text)
    except ValueError:
        err = {}
    if not isinstance(err, dict):
        err = {}
    kind = err.get("category") or err.get("errorType")
    if isinstance(kind, str):
        message = err.get("message") or "HubSpot refused the request"
        details = err.get("errors") if isinstance(err.get("errors"), list) else []
        contexts = [e["context"] for e in details if isinstance(e, dict) and isinstance(e.get("context"), dict)]
        needed = [str(s) for c in contexts if isinstance(c.get("requiredGranularScopes"), list) for s in c["requiredGranularScopes"]]
        if needed:
            message += f" (it needs {', '.join(needed)})"
        return HubSpotError(status, kind.lower(), message)
    if isinstance(err.get("error"), str):
        return HubSpotError(status, err["error"], err.get("message") or err["error"])
    fallback = {401: "invalid_authentication", 404: "object_not_found", 429: "rate_limit"}.get(status, "")
    return HubSpotError(status, fallback, text[:500] or f"HTTP {status}")


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def bounded(n):
    return max(1, min(n, MAX_ITEMS))


def record_id(value, what="record_id"):
    if not ID.match(value or ""):
        raise HubSpotError(0, "bad_" + what, f"{value!r} is not a HubSpot id (its digits, e.g. 123456)")
    return value


def cursor(value):
    if not CURSOR.match(value or ""):
        raise HubSpotError(0, "bad_after", f"{value!r} is not a page's cursor: pass a list's `after` as it came")
    return value


def properties(value):
    names = [p.strip() for p in (value or "").split(",") if p.strip()]
    if not names or not all(PROPERTY.match(p) for p in names):
        raise HubSpotError(0, "bad_property", f"{value!r} is not a list of property names (e.g. email,firstname)")
    return ",".join(names)


def settings(pairs):
    """`--set name=value` pairs, as a record's `properties`."""
    props = {}
    for pair in pairs or []:
        name, eq, value = pair.partition("=")
        if not eq or not PROPERTY.match(name):
            raise HubSpotError(0, "bad_property", f"{pair!r} is not name=value (e.g. lifecyclestage=customer)")
        props[name] = value
    if not props:
        raise HubSpotError(0, "nothing_to_change", "name at least one --set name=value")
    return props


def page(answer):
    """A list's answer: its results, the total where HubSpot counts it, and
    the cursor of the next page (`--after`), none on the last."""
    shown = {"results": answer.get("results", [])}
    if "total" in answer:
        shown["total"] = answer["total"]
    shown["after"] = ((answer.get("paging") or {}).get("next") or {}).get("after")
    return shown


def check(_args):
    a = call("GET", f"/account-info/{VERSION}/details")
    out({"status": "connected", "portal_id": a.get("portalId"), "account_type": a.get("accountType"), "time_zone": a.get("timeZone"), "currency": a.get("companyCurrency"), "data_hosting_location": a.get("dataHostingLocation"), "ui_domain": a.get("uiDomain")})


def where(filters):
    made = []
    for f in filters or []:
        m = WHERE.match(f)
        if not m or not FILTER_PROPERTY.match(m.group(1)):
            raise HubSpotError(0, "bad_filter", f"{f!r} is not property=value (or !=, >, >=, <, <=, ~ for a token it contains)")
        made.append({"propertyName": m.group(1), "operator": OPERATORS[m.group(2)], "value": m.group(3)})
    return made


def search(args):
    body = {"limit": bounded(args.max), "properties": properties(args.properties or OBJECTS[args.type][3]).split(",")}
    if args.query:
        body["query"] = args.query
    filters = where(args.where)
    if filters:
        body["filterGroups"] = [{"filters": filters}]
    if args.sort:
        if not PROPERTY.match(args.sort):
            raise HubSpotError(0, "bad_property", f"--sort {args.sort!r} is not a property's name")
        body["sorts"] = [{"propertyName": args.sort, "direction": "DESCENDING" if args.desc else "ASCENDING"}]
    if args.after:
        body["after"] = cursor(args.after)
    out(page(call("POST", f"/crm/objects/{VERSION}/{args.type}/search", body)))


def get(args):
    query = {"properties": properties(args.properties or OBJECTS[args.type][3])}
    if args.associations:
        query["associations"] = ",".join(object_type(t) for t in args.associations.split(","))
    out(call("GET", f"/crm/objects/{VERSION}/{args.type}/{record_id(args.id)}", query=query))


def create(args):
    out(call("POST", f"/crm/objects/{VERSION}/{args.type}", {"properties": settings(args.set)}))


def update(args):
    out(call("PATCH", f"/crm/objects/{VERSION}/{args.type}/{record_id(args.id)}", {"properties": settings(args.set)}))


def object_type(name):
    name = name.strip()
    if name not in OBJECTS:
        raise HubSpotError(0, "bad_object_type", f"{name!r} is not one of {', '.join(OBJECTS)}")
    return name


def associations(args):
    frm, to = OBJECTS[args.type][0], OBJECTS[object_type(args.to_type)][0]
    query = {"limit": bounded(args.max)}
    if args.after:
        query["after"] = cursor(args.after)
    out(page(call("GET", f"/crm/objects/{VERSION}/{frm}/{record_id(args.id)}/associations/{to}", query=query)))


def associate(args):
    frm, to = OBJECTS[args.type][0], OBJECTS[object_type(args.to_type)][0]
    out(call("PUT", f"/crm/objects/{VERSION}/{frm}/{record_id(args.id)}/associations/default/{to}/{record_id(args.to_id)}"))


def notes(args):
    body = {
        "limit": bounded(args.max),
        "properties": NOTE_FIELDS.split(","),
        "filterGroups": [{"filters": [{"propertyName": f"associations.{OBJECTS[args.type][1]}", "operator": "EQ", "value": record_id(args.id)}]}],
        "sorts": [{"propertyName": "hs_timestamp", "direction": "DESCENDING"}],
    }
    out(page(call("POST", f"/crm/objects/{VERSION}/notes/search", body)))


def note(args):
    if not args.body.strip():
        raise HubSpotError(0, "bad_body", "say what the note says")
    props = {"hs_timestamp": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"), "hs_note_body": args.body}
    if args.owner:
        props["hubspot_owner_id"] = record_id(args.owner, "owner_id")
    to = {"to": {"id": record_id(args.id)}, "types": [{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": OBJECTS[args.type][2]}]}
    out(call("POST", f"/crm/objects/{VERSION}/notes", {"properties": props, "associations": [to]}))


def owners(args):
    query = {"limit": bounded(args.max)}
    if args.email:
        if not EMAIL.match(args.email):
            raise HubSpotError(0, "bad_email", f"{args.email!r} is not an email address")
        query["email"] = args.email
    if args.after:
        query["after"] = cursor(args.after)
    out(page(call("GET", f"/crm/owners/{VERSION}", query=query)))


def raw(args):
    try:
        body = json.loads(args.json) if args.json else None
    except ValueError as exc:
        raise HubSpotError(0, "bad_json", f"--json is not JSON: {exc}") from None
    path, _, query = args.path.partition("?")
    out(call(args.method.upper(), path, body, urllib.parse.parse_qsl(query, keep_blank_values=True) if query else None))


def parser():
    top = argparse.ArgumentParser(description="HubSpot through the person's connected HubSpot account")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the HubSpot connection answers for this agent, and which account").set_defaults(func=check)
    types = list(OBJECTS)

    p = sub.add_parser("search", help="a type's records by text, filters or both")
    p.add_argument("type", choices=types)
    p.add_argument("query", nargs="?", default="", help="text in the type's default searchable properties (name, email, domain…)")
    p.add_argument("--where", action="append", help="property=value; or !=, >, >=, <, <=, ~ (contains the token); repeat for AND")
    p.add_argument("--properties", help="the properties to show, comma-separated")
    p.add_argument("--sort", help="a property to order by, e.g. hs_lastmodifieddate")
    p.add_argument("--desc", action="store_true", help="--sort's highest or newest first")
    p.add_argument("--max", type=int, default=10)
    p.add_argument("--after", help="the next page's cursor, a list's `after`")
    p.set_defaults(func=search)

    p = sub.add_parser("get", help="one record")
    p.add_argument("type", choices=types)
    p.add_argument("id")
    p.add_argument("--properties", help="the properties to show, comma-separated")
    p.add_argument("--associations", help="types whose associated ids to show, e.g. contacts,deals")
    p.set_defaults(func=get)

    p = sub.add_parser("create", help="a record")
    p.add_argument("type", choices=types)
    p.add_argument("--set", action="append", metavar="NAME=VALUE", required=True)
    p.set_defaults(func=create)

    p = sub.add_parser("update", help="a record's properties")
    p.add_argument("type", choices=types)
    p.add_argument("id")
    p.add_argument("--set", action="append", metavar="NAME=VALUE", required=True, help="a property's new value; empty clears it")
    p.set_defaults(func=update)

    p = sub.add_parser("associations", help="the records of a type a record is associated with")
    p.add_argument("type", choices=types)
    p.add_argument("id")
    p.add_argument("to_type")
    p.add_argument("--max", type=int, default=100)
    p.add_argument("--after", help="the next page's cursor, a list's `after`")
    p.set_defaults(func=associations)

    p = sub.add_parser("associate", help="associate two records (HubSpot's default association)")
    p.add_argument("type", choices=types)
    p.add_argument("id")
    p.add_argument("to_type")
    p.add_argument("to_id")
    p.set_defaults(func=associate)

    p = sub.add_parser("notes", help="the notes on a record, newest first")
    p.add_argument("type", choices=types)
    p.add_argument("id")
    p.add_argument("--max", type=int, default=20)
    p.set_defaults(func=notes)

    p = sub.add_parser("note", help="add a note to a record")
    p.add_argument("type", choices=types)
    p.add_argument("id")
    p.add_argument("--body", required=True, help="the note's text")
    p.add_argument("--owner", help="the owner's id the note is by (`owners`)")
    p.set_defaults(func=note)

    p = sub.add_parser("owners", help="the account's owners: who records can be assigned to")
    p.add_argument("--email")
    p.add_argument("--max", type=int, default=100)
    p.add_argument("--after", help="the next page's cursor, a list's `after`")
    p.set_defaults(func=owners)

    p = sub.add_parser("request", help="any HubSpot API call")
    p.add_argument("method", choices=["GET", "POST", "PATCH", "PUT", "DELETE", "get", "post", "patch", "put", "delete"])
    p.add_argument("path", help="from its API's name (/crm/…), its query after ?")
    p.add_argument("--json", default="", help="the request's JSON body")
    p.set_defaults(func=raw)
    return top


READ_ONLY = "the connection may not do this: connected read only (the person allows changes in Settings, under Connections), or HubSpot's app lacks the scope"
HINTS = {
    "not_connected": "the person has not connected HubSpot (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left HubSpot out",
    "invalid_authentication": "HubSpot refused the connection's token: ask the person to reconnect HubSpot in Settings, under Connections",
    "expired_authentication": "HubSpot refused the connection's token: ask the person to reconnect HubSpot in Settings, under Connections",
    "missing_scopes": READ_ONLY,
    "object_not_found": "the id is wrong, or not in this HubSpot account",
    "rate_limit": "HubSpot's rate limit: wait, then ask for less at a time",
    "rate_limits": "HubSpot's rate limit: wait, then ask for less at a time",
}


def main():
    args = parser().parse_args()
    try:
        args.func(args)
    except HubSpotError as e:
        print(json.dumps({"error": e.code or "hubspot_error", "status": e.status, "message": str(e), "hint": HINTS.get(e.code, "")}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
