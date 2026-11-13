#!/usr/bin/env python3
"""Jira through the person's connected Atlassian account.

The computer holds no Atlassian credential: its environment holds the
connection's placeholder, which names the agent
(`ATLASSIAN_OAUTH_ACCESS_TOKEN`), each request sends it as a bearer token to
api.atlassian.com, and the computer's intercept swaps in a short-lived token
for the agent's owner (docs/computers.md, Connections and operator keys).
Every Jira site is reached there, under /ex/jira/{cloudid}: the helper finds
a site's cloud id in the sites the person granted (accessible-resources).
Python's standard library only; every command prints JSON.

Usage:
  python jira_api.py check
  python jira_api.py sites
  python jira_api.py search 'project = ENG AND statusCategory != Done ORDER BY updated DESC' [--max 25] [--fields summary,status] [--page TOKEN]
  python jira_api.py issue get ENG-123 [--raw]
  python jira_api.py issue create --project ENG --type Task --summary "…" [--description "…"] [--assignee ACCOUNT_ID | --assign-me] [--label x …] [--parent ENG-1] [--priority High]
  python jira_api.py issue comment ENG-123 --body "…"
  python jira_api.py transitions ENG-123
  python jira_api.py transition ENG-123 (--to "Done" | --id 31) [--comment "…"]
  python jira_api.py projects [--query eng] [--max 50]
  python jira_api.py users "ada" [--max 10]
  python jira_api.py request GET /rest/api/3/myself [--json '{…}']

Every command but check and sites takes --site (the site's name, its host,
such as acme.atlassian.net, or its cloud id) where the person granted more
than one site. Text given to --description, --body and --comment becomes an
Atlassian Document Format document; --adf takes one as JSON instead.

Environment:
  ATLASSIAN_OAUTH_ACCESS_TOKEN  the Atlassian connection's placeholder (set on
                                a fragment computer once the person connects
                                Atlassian), or any Atlassian OAuth 2.0 token
"""

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.atlassian.com"
TIMEOUT_S = 60
# Jira pages at most 100 issues on a search, 50 on most lists
MAX_ITEMS = 100
SEARCH_FIELDS = ["summary", "status", "assignee", "priority", "issuetype", "updated"]
ISSUE_FIELDS = "summary,status,assignee,reporter,priority,issuetype,project,labels,created,updated,duedate,parent,description,comment"
# a site's cloud id is a UUID
CLOUD_ID = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
# an issue's key (ENG-123) or its id
ISSUE = re.compile(r"^([A-Za-z][A-Za-z0-9_]{0,254}-[0-9]{1,18}|[0-9]{1,18})$")
# a project's key (ENG) or its id
PROJECT = re.compile(r"^([A-Za-z][A-Za-z0-9_]{0,254}|[0-9]{1,18})$")
ACCOUNT = re.compile(r"^[A-Za-z0-9:_\-]{1,128}$")
TRANSITION = re.compile(r"^[0-9]{1,18}$")
FIELD = re.compile(r"^-?[A-Za-z0-9_*.\-]{1,64}$")
# Jira's labels hold no space
LABEL = re.compile(r"^\S{1,255}$")
# a path on api.atlassian.com, its characters RFC 3986's (no backslash, no #)
PATH = re.compile(r"^/[A-Za-z0-9/_\-.:~%@!$&'()*+,;=]*$")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points (an attachment's content goes to another
    host)."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


class ApiError(Exception):
    def __init__(self, status, code, message):
        super().__init__(message)
        self.status = status
        self.code = code


def token():
    value = os.getenv("ATLASSIAN_OAUTH_ACCESS_TOKEN", "").strip()
    if not value:
        raise ApiError(0, "not_connected", "ATLASSIAN_OAUTH_ACCESS_TOKEN is not set: the person has not connected Atlassian, or narrowed you from it")
    return value


def safe_path(path):
    """`path` as the helper sends it: on api.atlassian.com, and its every
    segment, decoded, neither `.` nor `..`."""
    if not PATH.match(path) or any(urllib.parse.unquote(seg) in (".", "..") for seg in path.split("/")):
        raise ApiError(0, "bad_path", "a path on the site's API, from /rest/ (e.g. /rest/api/3/myself), with no . or .. in it")
    return path


def call(method, path, body=None, query=None):
    """`method` `path` on api.atlassian.com (the sites, the person, or a
    site's Jira API), its answer read as JSON (none for 204)."""
    safe_path(path)
    if not (path in ("/oauth/token/accessible-resources", "/me") or re.match(r"^/ex/jira/[0-9a-fA-F-]{36}/rest/", path)):
        raise ApiError(0, "bad_path", "a path on the site's Jira API, from /rest/")
    url = API + path + (("?" + urllib.parse.urlencode(query, quote_via=urllib.parse.quote)) if query else "")
    data = None if body is None else json.dumps(body).encode("utf-8")
    headers = {"Authorization": f"Bearer {token()}", "Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with OPENER.open(request, timeout=TIMEOUT_S) as response:
            text = response.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise ApiError(exc.code, "redirected", f"Atlassian answered a redirect to {exc.headers.get('Location', '?')}, which the helper never follows with the token") from None
        raise refusal(exc.code, exc.read().decode("utf-8", errors="replace"), exc.headers.get("Retry-After")) from None
    except urllib.error.URLError as exc:
        raise ApiError(0, "unreachable", str(exc.reason)) from None
    if not text.strip():
        return {}
    try:
        return json.loads(text)
    except ValueError:
        return {"text": text[:20000]}


STATUS_CODES = {400: "invalid_input", 401: "unauthorized", 403: "no_permission", 404: "not_found", 409: "conflict", 429: "rate_limited"}


def refusal(status, text, retry_after=None):
    """An HTTP refusal: the computer's own (`{error: "not_connected" |
    "forbidden", message}`), Atlassian's gateway's (`{code, message}`), or
    Jira's (`{errorMessages, errors}`), named by its status."""
    try:
        err = json.loads(text)
    except ValueError:
        err = None
    if isinstance(err, dict) and isinstance(err.get("error"), str):
        return ApiError(status, err["error"], err.get("message") or err["error"])
    message = text[:500] or f"HTTP {status}"
    if isinstance(err, dict):
        said = [str(m) for m in err.get("errorMessages") or []]
        if isinstance(err.get("errors"), dict):
            said += [f"{k}: {v}" for k, v in err["errors"].items()]
        message = "; ".join(said) or str(err.get("message") or message)
    code = STATUS_CODES.get(status, "atlassian_error")
    if status == 401 and "scope does not match" in message.lower():
        code = "scope_does_not_match"
    if status == 429 and retry_after:
        message += f" (retry after {retry_after} s)"
    return ApiError(status, code, message)


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def bounded(n):
    return max(1, min(n, MAX_ITEMS))


def checked(value, pattern, what):
    if not pattern.match(value or ""):
        raise ApiError(0, "bad_" + what, f"{value!r} is not a {what}")
    return value


def granted_sites():
    """The sites the person granted, each once (Atlassian may list a site
    once for each product), with the scopes granted there."""
    listed = call("GET", "/oauth/token/accessible-resources")
    sites = {}
    for r in listed if isinstance(listed, list) else []:
        cloud = r.get("id") if isinstance(r, dict) else None
        if not isinstance(cloud, str) or not CLOUD_ID.match(cloud):
            continue
        site = sites.setdefault(cloud, {"id": cloud, "name": r.get("name"), "url": r.get("url"), "scopes": []})
        site["scopes"] += [s for s in r.get("scopes") or [] if s not in site["scopes"]]
    return list(sites.values())


def jira_sites():
    return [s for s in granted_sites() if not s["scopes"] or any("jira" in scope for scope in s["scopes"])]


def site_of(args):
    """The site a command goes to: --site's, else the person's one Jira
    site; found once a command."""
    if getattr(args, "found_site", None):
        return args.found_site
    args.found_site = find_site(args)
    return args.found_site


def find_site(args):
    want = (args.site or "").strip()
    found = jira_sites()
    if not found:
        raise ApiError(0, "no_site", "the connection reaches no Jira site")
    if not want:
        if len(found) == 1:
            return found[0]
        raise ApiError(0, "choose_site", "the person granted several Jira sites; name one with --site: " + ", ".join(f"{s['name']} ({s['url']})" for s in found))
    key = want.lower().rstrip("/")
    for s in found:
        host = urllib.parse.urlsplit(s["url"] or "").hostname or ""
        if key in {s["id"].lower(), (s["name"] or "").lower(), host.lower(), (s["url"] or "").lower().rstrip("/")}:
            return s
    raise ApiError(0, "no_such_site", f"{want!r} is none of the person's Jira sites: " + ", ".join(f"{s['name']} ({s['url']})" for s in found))


def jira(args, method, path, body=None, query=None):
    """`method` `path` (from /rest/) on the command's site's Jira API."""
    if not safe_path(path).startswith("/rest/"):
        raise ApiError(0, "bad_path", "a path on the site's Jira API, from /rest/ (e.g. /rest/api/3/myself)")
    site = site_of(args)
    return call(method, f"/ex/jira/{site['id']}{path}", body, query), site


def adf(text, is_adf=False):
    """Atlassian Document Format for `text`: its paragraphs (blank-line
    apart) and line breaks; with `is_adf`, `text` is a document already."""
    if is_adf:
        try:
            doc = json.loads(text)
        except ValueError as exc:
            raise ApiError(0, "bad_json", f"--adf takes an Atlassian Document Format document as JSON: {exc}") from None
        if not isinstance(doc, dict) or doc.get("type") != "doc":
            raise ApiError(0, "bad_json", 'an Atlassian Document Format document is {"type": "doc", "version": 1, "content": [...]}')
        return doc
    content = []
    for para in re.split(r"\n\s*\n", text.strip()) if text.strip() else []:
        nodes = []
        for i, line in enumerate(para.split("\n")):
            if i:
                nodes.append({"type": "hardBreak"})
            if line:
                nodes.append({"type": "text", "text": line})
        content.append({"type": "paragraph", "content": nodes})
    return {"type": "doc", "version": 1, "content": content}


BLOCKS = {"paragraph", "heading", "blockquote", "codeBlock", "rule", "panel", "tableRow", "mediaSingle"}


def adf_text(node):
    """An Atlassian Document Format document's text, for reading."""
    if isinstance(node, list):
        return "".join(adf_text(n) for n in node)
    if not isinstance(node, dict):
        return ""
    kind, attrs = node.get("type"), node.get("attrs") or {}
    if kind == "text":
        return node.get("text", "")
    if kind == "hardBreak":
        return "\n"
    if kind in ("mention", "emoji", "status", "date", "inlineCard"):
        return str(attrs.get("text") or attrs.get("shortName") or attrs.get("url") or attrs.get("timestamp") or "")
    inner = adf_text(node.get("content") or [])
    if kind == "listItem":
        return "- " + inner.strip() + "\n"
    return inner.rstrip("\n") + "\n" if kind in BLOCKS else inner


def person(p):
    return {"displayName": p.get("displayName"), "accountId": p.get("accountId")} if isinstance(p, dict) else None


def named(v, key="name"):
    return v.get(key) if isinstance(v, dict) else None


def browse(site, key):
    return f"{site['url'].rstrip('/')}/browse/{key}" if site.get("url") and key else None


def issue_summary(issue, site):
    f = issue.get("fields") or {}
    return {"key": issue.get("key"), "id": issue.get("id"), "summary": f.get("summary"), "status": named(f.get("status")), "type": named(f.get("issuetype")),
            "priority": named(f.get("priority")), "assignee": person(f.get("assignee")), "updated": f.get("updated"), "url": browse(site, issue.get("key"))}


def check(_args):
    me = call("GET", "/me")
    sites = granted_sites()
    out({"status": "connected", "name": me.get("name"), "email": me.get("email"), "account_id": me.get("account_id"),
         "sites": [{"id": s["id"], "name": s["name"], "url": s["url"], "jira": not s["scopes"] or any("jira" in scope for scope in s["scopes"])} for s in sites]})


def sites(_args):
    out(granted_sites())


def search(args):
    fields = [checked(f.strip(), FIELD, "field") for f in args.fields.split(",") if f.strip()] if args.fields else SEARCH_FIELDS
    body = {"jql": args.jql, "maxResults": bounded(args.max), "fields": fields}
    if args.page:
        body["nextPageToken"] = args.page
    answer, site = jira(args, "POST", "/rest/api/3/search/jql", body)
    issues = answer.get("issues") or []
    out({"issues": issues if args.fields else [issue_summary(i, site) for i in issues], "nextPageToken": answer.get("nextPageToken"), "isLast": answer.get("isLast")})


def issue_get(args):
    key = checked(args.key, ISSUE, "issue")
    issue, site = jira(args, "GET", f"/rest/api/3/issue/{key}", query={"fields": ISSUE_FIELDS})
    if args.raw:
        return out(issue)
    f = issue.get("fields") or {}
    comments = (f.get("comment") or {}).get("comments") or []
    out({**issue_summary(issue, site), "reporter": person(f.get("reporter")), "project": {"key": named(f.get("project"), "key"), "name": named(f.get("project"))},
         "labels": f.get("labels") or [], "created": f.get("created"), "duedate": f.get("duedate"), "parent": named(f.get("parent"), "key"),
         "description": adf_text(f.get("description")).strip() or None,
         "comments": [{"id": c.get("id"), "author": person(c.get("author")), "created": c.get("created"), "body": adf_text(c.get("body")).strip()} for c in comments]})


def my_account():
    account = call("GET", "/me").get("account_id")
    return checked(account, ACCOUNT, "account")


def issue_create(args):
    project = checked(args.project, PROJECT, "project")
    fields = {"project": {"id": project} if project.isdigit() else {"key": project}, "issuetype": {"name": args.type}, "summary": args.summary}
    if args.description:
        fields["description"] = adf(args.description, args.adf)
    if args.label:
        fields["labels"] = [checked(label, LABEL, "label") for label in args.label]
    if args.parent:
        fields["parent"] = {"key": checked(args.parent, ISSUE, "issue")}
    if args.priority:
        fields["priority"] = {"name": args.priority}
    if args.assignee:
        fields["assignee"] = {"accountId": checked(args.assignee, ACCOUNT, "account")}
    site_of(args)
    if args.assign_me:
        fields["assignee"] = {"accountId": my_account()}
    made, site = jira(args, "POST", "/rest/api/3/issue", {"fields": fields})
    out({"key": made.get("key"), "id": made.get("id"), "url": browse(site, made.get("key"))})


def issue_comment(args):
    key = checked(args.key, ISSUE, "issue")
    made, _ = jira(args, "POST", f"/rest/api/3/issue/{key}/comment", {"body": adf(args.body, args.adf)})
    out({"id": made.get("id"), "issue": key, "created": made.get("created"), "author": person(made.get("author"))})


def transitions_of(args, key):
    answer, _ = jira(args, "GET", f"/rest/api/3/issue/{key}/transitions")
    return [{"id": t.get("id"), "name": t.get("name"), "to": named(t.get("to")), "category": named((t.get("to") or {}).get("statusCategory"))} for t in answer.get("transitions") or []]


def transitions(args):
    out(transitions_of(args, checked(args.key, ISSUE, "issue")))


def transition(args):
    key = checked(args.key, ISSUE, "issue")
    if args.id:
        chosen = {"id": checked(args.id, TRANSITION, "transition"), "name": None, "to": None}
    else:
        offered = transitions_of(args, key)
        want = args.to.strip().lower()
        matches = [t for t in offered if want in ((t["name"] or "").lower(), (t["to"] or "").lower())]
        if not matches:
            raise ApiError(0, "no_such_transition", f"{args.to!r} is no transition of {key}'s now: " + ", ".join(f"{t['name']} (to {t['to']})" for t in offered))
        chosen = matches[0]
    body = {"transition": {"id": chosen["id"]}}
    if args.comment:
        body["update"] = {"comment": [{"add": {"body": adf(args.comment, args.adf)}}]}
    jira(args, "POST", f"/rest/api/3/issue/{key}/transitions", body)
    out({"status": "transitioned", "issue": key, "transition": chosen["name"] or chosen["id"], "to": chosen["to"]})


def projects(args):
    query = {"maxResults": bounded(args.max), "orderBy": "name"}
    if args.query:
        query["query"] = args.query
    answer, _ = jira(args, "GET", "/rest/api/3/project/search", query=query)
    out([{"key": p.get("key"), "id": p.get("id"), "name": p.get("name"), "type": p.get("projectTypeKey")} for p in answer.get("values") or []])


def users(args):
    found, _ = jira(args, "GET", "/rest/api/3/user/search", query={"query": args.query, "maxResults": bounded(args.max)})
    out([{"accountId": u.get("accountId"), "displayName": u.get("displayName"), "emailAddress": u.get("emailAddress"), "active": u.get("active")} for u in found if isinstance(u, dict)] if isinstance(found, list) else found)


def raw(args):
    try:
        body = json.loads(args.json) if args.json else None
    except ValueError as exc:
        raise ApiError(0, "bad_json", f"--json is not JSON: {exc}") from None
    path, _, query = args.path.partition("?")
    answer, _ = jira(args, args.method.upper(), path, body, dict(urllib.parse.parse_qsl(query)) if query else None)
    out(answer)


def parser():
    top = argparse.ArgumentParser(description="Jira through the person's connected Atlassian account")
    on_site = argparse.ArgumentParser(add_help=False)
    on_site.add_argument("--site", help="the site's name, its host (acme.atlassian.net) or its cloud id, where the person granted several")
    texts = argparse.ArgumentParser(add_help=False)
    texts.add_argument("--adf", action="store_true", help="the text given is an Atlassian Document Format document, as JSON")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the Atlassian connection answers for this agent, and its sites").set_defaults(func=check)
    sub.add_parser("sites", help="the sites the person granted, each with its cloud id and scopes").set_defaults(func=sites)

    p = sub.add_parser("search", parents=[on_site], help="issues a JQL query finds")
    p.add_argument("jql", help="a bounded JQL query, e.g. 'assignee = currentUser() ORDER BY updated DESC'")
    p.add_argument("--max", type=int, default=25)
    p.add_argument("--fields", help="the fields to answer, comma-separated (Jira's own answer then)")
    p.add_argument("--page", help="the nextPageToken of the page before")
    p.set_defaults(func=search)

    issue = sub.add_parser("issue").add_subparsers(dest="action", required=True)
    p = issue.add_parser("get", parents=[on_site])
    p.add_argument("key", help="its key (ENG-123) or id")
    p.add_argument("--raw", action="store_true", help="Jira's answer as it is")
    p.set_defaults(func=issue_get)
    p = issue.add_parser("create", parents=[on_site, texts])
    p.add_argument("--project", required=True, help="its key (ENG) or id")
    p.add_argument("--type", required=True, help="the issue type's name: Task, Bug, Story, Epic, Subtask")
    p.add_argument("--summary", required=True)
    p.add_argument("--description")
    who = p.add_mutually_exclusive_group()
    who.add_argument("--assignee", help="an account id (`users` finds one)")
    who.add_argument("--assign-me", action="store_true")
    p.add_argument("--label", action="append")
    p.add_argument("--parent", help="the parent issue's key, for a subtask or an epic's child")
    p.add_argument("--priority", help="its name: Highest, High, Medium, Low, Lowest")
    p.set_defaults(func=issue_create)
    p = issue.add_parser("comment", parents=[on_site, texts])
    p.add_argument("key")
    p.add_argument("--body", required=True)
    p.set_defaults(func=issue_comment)

    p = sub.add_parser("transitions", parents=[on_site], help="the transitions an issue may take now")
    p.add_argument("key")
    p.set_defaults(func=transitions)
    p = sub.add_parser("transition", parents=[on_site, texts], help="move an issue on its workflow")
    p.add_argument("key")
    to = p.add_mutually_exclusive_group(required=True)
    to.add_argument("--to", help="the transition's name or its status's (Done, In Progress)")
    to.add_argument("--id", help="the transition's id")
    p.add_argument("--comment")
    p.set_defaults(func=transition)

    p = sub.add_parser("projects", parents=[on_site])
    p.add_argument("--query", help="part of a project's key or name")
    p.add_argument("--max", type=int, default=50)
    p.set_defaults(func=projects)
    p = sub.add_parser("users", parents=[on_site], help="people by name or email, for their account ids")
    p.add_argument("query")
    p.add_argument("--max", type=int, default=10)
    p.set_defaults(func=users)

    p = sub.add_parser("request", parents=[on_site], help="any call on the site's Jira API")
    p.add_argument("method", choices=["GET", "POST", "PUT", "PATCH", "DELETE", "get", "post", "put", "patch", "delete"])
    p.add_argument("path", help="from /rest/ (e.g. /rest/api/3/myself, /rest/agile/1.0/board), its query after ?")
    p.add_argument("--json", default="", help="the request's JSON body")
    p.set_defaults(func=raw)
    return top


READ_ONLY = "connected read only (the person allows changes in Settings, under Connections), or the site they picked isn't this one"
HINTS = {
    "not_connected": "the person has not connected Atlassian (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left Atlassian out",
    "unauthorized": "Atlassian refused the connection's token: ask the person to reconnect Atlassian in Settings, under Connections",
    "scope_does_not_match": "the connection may not do this: " + READ_ONLY,
    "no_permission": "the person's Jira permissions don't allow it in this project",
    "not_found": "no such issue or project, or the person may not see it",
    "choose_site": "name the site with --site; `sites` lists them",
    "no_such_site": "`sites` lists the sites the person granted; they add one by connecting Atlassian again in Settings (Disconnect, then Connect) and picking it",
    "no_site": "the person granted no Jira site: they connect Atlassian again in Settings and pick a site with Jira",
    "rate_limited": "Atlassian limits requests: wait as it says, then try once more",
}


def main():
    args = parser().parse_args()
    try:
        args.func(args)
    except ApiError as e:
        print(json.dumps({"error": e.code or "atlassian_error", "status": e.status, "message": str(e), "hint": HINTS.get(e.code, "")}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
