#!/usr/bin/env python3
"""Salesforce through the person's connected Salesforce org.

The computer holds no Salesforce credential: its environment holds the
connection's placeholder, which names the agent (`SALESFORCE_ACCESS_TOKEN`),
each request sends it as a bearer token, and the computer's intercept swaps
in a short-lived token for the agent's owner (docs/computers.md, Connections
and operator keys). The token goes to login.salesforce.com, and to the org's
own host (`<mydomain>.my.salesforce.com`) alone: the helper learns that host
from the org's identity (`check`). Python's standard library only; every
command prints JSON.

Usage:
  python salesforce_api.py check
  python salesforce_api.py query "SELECT Id, Name FROM Account ORDER BY CreatedDate DESC LIMIT 10"
  python salesforce_api.py search "FIND {Acme} IN NAME FIELDS RETURNING Account(Id, Name), Contact(Id, Name)"
  python salesforce_api.py get Account 001xx000003DGb2AAG [--fields Id,Name,Industry]
  python salesforce_api.py create Contact --json '{"LastName": "Lovelace", "Email": "ada@example.com"}'
  python salesforce_api.py update Opportunity 006xx000001a2b3AAA --json '{"StageName": "Closed Won"}'
  python salesforce_api.py describe Opportunity
  python salesforce_api.py objects
  python salesforce_api.py request GET /services/data/v63.0/limits

Environment:
  SALESFORCE_ACCESS_TOKEN  the Salesforce connection's placeholder (set on a
                           fragment computer once the person connects
                           Salesforce), or any Salesforce OAuth access token
"""

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

LOGIN = "https://login.salesforce.com"
VERSION = "v63.0"
TIMEOUT_S = 60
# a record's id, and an object's API name (Account, Custom__c, ns__Thing__c)
RECORD_ID = re.compile(r"^[A-Za-z0-9]{15}(?:[A-Za-z0-9]{3})?$")
OBJECT_NAME = re.compile(r"^[A-Za-z][A-Za-z0-9_]{0,79}$")
# the org's host: the swap sends the token to one under this alone
ORG_HOST = re.compile(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*\.my\.salesforce\.com$")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


class SalesforceError(Exception):
    def __init__(self, status, code, message):
        super().__init__(message)
        self.status = status
        self.code = code


def token():
    value = os.getenv("SALESFORCE_ACCESS_TOKEN", "").strip()
    if not value:
        raise SalesforceError(0, "not_connected", "SALESFORCE_ACCESS_TOKEN is not set: the person has not connected Salesforce, or narrowed you from it")
    return value


def call(method, url, body=None):
    data = None if body is None else json.dumps(body).encode("utf-8")
    headers = {"Authorization": f"Bearer {token()}", "Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with OPENER.open(request, timeout=TIMEOUT_S) as response:
            raw = response.read()
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise SalesforceError(exc.code, "redirected", f"Salesforce answered a redirect to {exc.headers.get('Location', '?')}, which the helper never follows with the token") from None
        raise refusal(exc.code, exc.read().decode("utf-8", errors="replace")) from None
    except urllib.error.URLError as exc:
        raise SalesforceError(0, "unreachable", str(exc.reason)) from None
    return json.loads(raw.decode("utf-8")) if raw else {}


def refusal(status, text):
    """An HTTP refusal: the computer's own (`{error: "not_connected" |
    "forbidden", message}`), or Salesforce's (`[{errorCode, message}]`, or
    OAuth's `{error, error_description}`)."""
    try:
        err = json.loads(text)
    except ValueError:
        err = None
    if isinstance(err, dict) and isinstance(err.get("error"), str):
        return SalesforceError(status, err["error"], err.get("message") or err.get("error_description") or err["error"])
    if isinstance(err, list) and err and isinstance(err[0], dict):
        return SalesforceError(status, str(err[0].get("errorCode", "")).lower(), err[0].get("message") or "Salesforce refused the request")
    return SalesforceError(status, "", text[:500] or f"HTTP {status}")


_identity = None
_instance = None


def identity():
    """Who the connection is, and its org's URLs: asked once a run."""
    global _identity
    if _identity is None:
        _identity = call("GET", f"{LOGIN}/services/oauth2/userinfo")
    return _identity


def instance():
    """The org's own host, as its identity names it (`urls.rest`)."""
    global _instance
    if _instance is None:
        rest = (identity().get("urls") or {}).get("rest", "")
        url = urllib.parse.urlsplit(rest)
        host = (url.hostname or "").lower()
        if url.scheme != "https" or not ORG_HOST.match(host):
            raise SalesforceError(0, "no_org_host", f"the org's identity names no host under my.salesforce.com ({rest!r})")
        _instance = f"https://{host}"
    return _instance


def data(path):
    return f"{instance()}/services/data/{VERSION}{path}"


def object_name(name):
    if not OBJECT_NAME.match(name):
        raise SalesforceError(0, "bad_object", f"{name!r} is not an object's API name (Account, Custom__c)")
    return name


def record_id(value):
    if not RECORD_ID.match(value):
        raise SalesforceError(0, "bad_id", f"{value!r} is not a record id (15 or 18 letters and digits)")
    return value


def fields(text):
    try:
        value = json.loads(text)
    except ValueError as exc:
        raise SalesforceError(0, "bad_json", f"--json is not JSON: {exc}") from None
    if not isinstance(value, dict) or not value:
        raise SalesforceError(0, "bad_json", "--json is an object of field names and values")
    return value


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def check(_args):
    me = identity()
    out({"status": "connected", "name": me.get("name"), "email": me.get("email"), "org": me.get("organization_id"), "instance": instance()})


def query(args):
    answer = call("GET", data(f"/query?{urllib.parse.urlencode({'q': args.soql})}"))
    records = answer.get("records", [])
    while args.all and answer.get("nextRecordsUrl"):
        answer = call("GET", f"{instance()}{answer['nextRecordsUrl']}")
        records += answer.get("records", [])
    out({"totalSize": answer.get("totalSize"), "done": answer.get("done"), "records": records})


def search(args):
    out(call("GET", data(f"/search?{urllib.parse.urlencode({'q': args.sosl})}")))


def get(args):
    suffix = f"?{urllib.parse.urlencode({'fields': args.fields})}" if args.fields else ""
    out(call("GET", data(f"/sobjects/{object_name(args.object)}/{record_id(args.id)}{suffix}")))


def create(args):
    out(call("POST", data(f"/sobjects/{object_name(args.object)}"), fields(args.json)))


def update(args):
    call("PATCH", data(f"/sobjects/{object_name(args.object)}/{record_id(args.id)}"), fields(args.json))
    out({"status": "updated", "object": args.object, "id": args.id})


def describe(args):
    d = call("GET", data(f"/sobjects/{object_name(args.object)}/describe"))
    keep = ("name", "label", "type", "updateable", "createable", "nillable", "picklistValues", "referenceTo")
    out({"name": d.get("name"), "label": d.get("label"), "fields": [{k: f.get(k) for k in keep if f.get(k) not in (None, [], False)} for f in d.get("fields", [])]})


def objects(_args):
    out([{"name": o.get("name"), "label": o.get("label"), "custom": o.get("custom")} for o in call("GET", data("/sobjects")).get("sobjects", []) if o.get("queryable")])


def request(args):
    if not args.path.startswith("/services/"):
        raise SalesforceError(0, "bad_path", "a path on the org's host, from /services/ (e.g. /services/data/v63.0/limits)")
    out(call(args.method, f"{instance()}{args.path}", fields(args.json) if args.json else None))


def parser():
    top = argparse.ArgumentParser(description="Salesforce through the person's connected Salesforce org")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the connection answers for this agent, and its org's host").set_defaults(func=check)
    p = sub.add_parser("query", help="SOQL")
    p.add_argument("soql")
    p.add_argument("--all", action="store_true", help="follow every page")
    p.set_defaults(func=query)
    p = sub.add_parser("search", help="SOSL")
    p.add_argument("sosl")
    p.set_defaults(func=search)
    p = sub.add_parser("get")
    p.add_argument("object")
    p.add_argument("id")
    p.add_argument("--fields", help="comma-separated field API names")
    p.set_defaults(func=get)
    p = sub.add_parser("create")
    p.add_argument("object")
    p.add_argument("--json", required=True, help="the record's fields")
    p.set_defaults(func=create)
    p = sub.add_parser("update")
    p.add_argument("object")
    p.add_argument("id")
    p.add_argument("--json", required=True, help="the fields to change")
    p.set_defaults(func=update)
    p = sub.add_parser("describe")
    p.add_argument("object")
    p.set_defaults(func=describe)
    sub.add_parser("objects", help="the org's queryable objects").set_defaults(func=objects)
    p = sub.add_parser("request", help="any REST call on the org's host")
    p.add_argument("method", choices=["GET", "POST", "PATCH", "PUT", "DELETE"])
    p.add_argument("path")
    p.add_argument("--json", default="")
    p.set_defaults(func=request)
    return top


HINTS = {
    "not_connected": "the person has not connected Salesforce (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left Salesforce out, or the call went to a host not their org's",
    "invalid_session_id": "Salesforce refused the connection's token: ask the person to reconnect Salesforce in Settings, under Connections",
    "insufficient_access_or_readonly": "the person's Salesforce user may not do this: their org's admin grants it, not you",
}


def main():
    args = parser().parse_args()
    try:
        args.func(args)
    except SalesforceError as e:
        print(json.dumps({"error": e.code or "salesforce_error", "status": e.status, "message": str(e), "hint": HINTS.get(e.code, "")}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
