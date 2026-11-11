#!/usr/bin/env python3
"""Confluence through the person's connected Atlassian account.

The computer holds no Atlassian credential: its environment holds the
connection's placeholder, which names the agent
(`ATLASSIAN_OAUTH_ACCESS_TOKEN`), each request sends it as a bearer token to
api.atlassian.com, and the computer's intercept swaps in a short-lived token
for the agent's owner (docs/computers.md, Connections and operator keys).
Every Confluence site is reached there, under /ex/confluence/{cloudid}/wiki:
the helper finds a site's cloud id in the sites the person granted
(accessible-resources). Pages and spaces are the v2 API's; CQL search is the
v1 API's (v2 has none). Python's standard library only; every command prints
JSON.

Usage:
  python confluence_api.py check
  python confluence_api.py sites
  python confluence_api.py search 'type = page AND text ~ "roadmap" ORDER BY lastmodified DESC' [--max 25] [--cursor C]
  python confluence_api.py spaces [--keys ENG,OPS] [--max 25] [--cursor C]
  python confluence_api.py pages list --space ENG [--title "…"] [--max 25] [--cursor C]
  python confluence_api.py pages get 123456 [--format storage|atlas_doc_format|view] [--raw]
  python confluence_api.py pages create --space ENG --title "…" (--text "…" | --body "<p>…</p>") [--parent 123456] [--draft]
  python confluence_api.py pages update 123456 [--title "…"] [--text "…" | --body "<p>…</p>"] [--version 4] [--message "…"]
  python confluence_api.py request GET /wiki/api/v2/pages/123456/children [--json '{…}']

Every command but check and sites takes --site (the site's name, its host,
such as acme.atlassian.net, or its cloud id) where the person granted more
than one site. --body is Confluence's storage format (XHTML); --text is plain
text, made paragraphs.

Environment:
  ATLASSIAN_OAUTH_ACCESS_TOKEN  the Atlassian connection's placeholder (set on
                                a fragment computer once the person connects
                                Atlassian), or any Atlassian OAuth 2.0 token
"""

import argparse
import html
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.atlassian.com"
TIMEOUT_S = 60
# Confluence pages its lists at most 250, its search at most 100
MAX_ITEMS = 100
# a site's cloud id is a UUID
CLOUD_ID = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
# a page's or a space's id is a number
NUMBER = re.compile(r"^[0-9]{1,20}$")
# a space's key (ENG, or ~accountid for a personal space)
SPACE_KEY = re.compile(r"^~?[A-Za-z0-9_:\-]{1,255}$")
# a path on api.atlassian.com, its characters RFC 3986's (no backslash, no #)
PATH = re.compile(r"^/[A-Za-z0-9/_\-.:~%@!$&'()*+,;=]*$")
FORMATS = ["storage", "atlas_doc_format", "view"]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points (an attachment's download goes to another
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
        raise ApiError(0, "bad_path", "a path on the site's Confluence API, from /wiki/api/v2/ or /wiki/rest/api/, with no . or .. in it")
    return path


def call(method, path, body=None, query=None):
    """`method` `path` on api.atlassian.com (the sites, the person, or a
    site's Confluence API), its answer read as JSON (none for 204)."""
    safe_path(path)
    if not (path in ("/oauth/token/accessible-resources", "/me") or re.match(r"^/ex/confluence/[0-9a-fA-F-]{36}/wiki/(api/v2|rest/api)/", path)):
        raise ApiError(0, "bad_path", "a path on the site's Confluence API, from /wiki/api/v2/ or /wiki/rest/api/")
    url = API + path + (("?" + urllib.parse.urlencode(query, doseq=True, quote_via=urllib.parse.quote)) if query else "")
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
    Confluence's (v2's `{errors: [{title, detail}]}`, v1's `{message}`),
    named by its status."""
    try:
        err = json.loads(text)
    except ValueError:
        err = None
    if isinstance(err, dict) and isinstance(err.get("error"), str):
        return ApiError(status, err["error"], err.get("message") or err["error"])
    message = text[:500] or f"HTTP {status}"
    if isinstance(err, dict):
        said = [": ".join(str(e[k]) for k in ("title", "detail") if e.get(k)) for e in err.get("errors") or [] if isinstance(e, dict)]
        message = "; ".join(s for s in said if s) or str(err.get("message") or message)
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


def is_confluence(site):
    return not site["scopes"] or any("confluence" in scope for scope in site["scopes"])


def site_of(args):
    """The site a command goes to: --site's, else the person's one
    Confluence site; found once a command."""
    if getattr(args, "found_site", None):
        return args.found_site
    args.found_site = find_site(args)
    return args.found_site


def find_site(args):
    want = (args.site or "").strip()
    found = [s for s in granted_sites() if is_confluence(s)]
    if not found:
        raise ApiError(0, "no_site", "the connection reaches no Confluence site")
    if not want:
        if len(found) == 1:
            return found[0]
        raise ApiError(0, "choose_site", "the person granted several Confluence sites; name one with --site: " + ", ".join(f"{s['name']} ({s['url']})" for s in found))
    key = want.lower().rstrip("/")
    for s in found:
        host = urllib.parse.urlsplit(s["url"] or "").hostname or ""
        if key in {s["id"].lower(), (s["name"] or "").lower(), host.lower(), (s["url"] or "").lower().rstrip("/")}:
            return s
    raise ApiError(0, "no_such_site", f"{want!r} is none of the person's Confluence sites: " + ", ".join(f"{s['name']} ({s['url']})" for s in found))


def wiki(args, method, path, body=None, query=None):
    """`method` `path` (from /wiki/api/v2/ or /wiki/rest/api/) on the
    command's site's Confluence API."""
    if not re.match(r"^/wiki/(api/v2|rest/api)/", safe_path(path)):
        raise ApiError(0, "bad_path", "a path on the site's Confluence API, from /wiki/api/v2/ or /wiki/rest/api/ (e.g. /wiki/api/v2/spaces)")
    site = site_of(args)
    return call(method, f"/ex/confluence/{site['id']}{path}", body, query), site


def web(site, link):
    """A page's or a space's address for people: the site's wiki and the
    answer's `webui`."""
    return f"{site['url'].rstrip('/')}/wiki{link}" if site.get("url") and isinstance(link, str) and link.startswith("/") else None


def cursor_of(answer):
    """The cursor of the next page of a list, from its answer's `next`."""
    nxt = (answer.get("_links") or {}).get("next")
    return dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(nxt).query)).get("cursor") if isinstance(nxt, str) else None


def storage(args):
    """The page's body in storage format: --body as given, or --text made
    paragraphs."""
    if args.body is not None:
        return args.body
    paragraphs = re.split(r"\n\s*\n", args.text.strip()) if args.text and args.text.strip() else []
    return "".join("<p>" + "<br/>".join(html.escape(line) for line in p.split("\n")) + "</p>" for p in paragraphs)


def check(_args):
    me = call("GET", "/me")
    sites = granted_sites()
    out({"status": "connected", "name": me.get("name"), "email": me.get("email"), "account_id": me.get("account_id"),
         "sites": [{"id": s["id"], "name": s["name"], "url": s["url"], "confluence": is_confluence(s)} for s in sites]})


def sites(_args):
    out(granted_sites())


def search(args):
    query = {"cql": args.cql, "limit": bounded(args.max)}
    if args.cursor:
        query["cursor"] = args.cursor
    answer, site = wiki(args, "GET", "/wiki/rest/api/search", query=query)
    results = []
    for r in answer.get("results") or []:
        content = r.get("content") or {}
        results.append({"id": content.get("id"), "type": content.get("type") or r.get("entityType"), "title": r.get("title") or content.get("title"),
                        "space": (r.get("resultGlobalContainer") or {}).get("title"), "excerpt": r.get("excerpt"), "lastModified": r.get("lastModified"), "url": web(site, r.get("url"))})
    out({"results": results, "totalSize": answer.get("totalSize"), "cursor": cursor_of(answer)})


def space_id(args, space):
    """A space's id: as given, or its key's."""
    if NUMBER.match(space or ""):
        return space
    key = checked(space, SPACE_KEY, "space")
    answer, _ = wiki(args, "GET", "/wiki/api/v2/spaces", query={"keys": key, "limit": 1})
    found = answer.get("results") or []
    if not found:
        raise ApiError(404, "no_such_space", f"no space {key!r} that the person may see")
    return str(found[0].get("id"))


def spaces(args):
    query = {"limit": bounded(args.max), "sort": "name"}
    if args.keys:
        query["keys"] = ",".join(checked(k.strip(), SPACE_KEY, "space") for k in args.keys.split(",") if k.strip())
    if args.cursor:
        query["cursor"] = args.cursor
    answer, site = wiki(args, "GET", "/wiki/api/v2/spaces", query=query)
    out({"results": [{"id": s.get("id"), "key": s.get("key"), "name": s.get("name"), "type": s.get("type"), "status": s.get("status"), "homepageId": s.get("homepageId"),
                      "url": web(site, (s.get("_links") or {}).get("webui"))} for s in answer.get("results") or []], "cursor": cursor_of(answer)})


def page_summary(p, site):
    return {"id": p.get("id"), "title": p.get("title"), "status": p.get("status"), "spaceId": p.get("spaceId"), "parentId": p.get("parentId"),
            "version": (p.get("version") or {}).get("number"), "url": web(site, (p.get("_links") or {}).get("webui"))}


def pages_list(args):
    space = space_id(args, args.space)
    query = {"limit": bounded(args.max), "sort": "-modified-date"}
    if args.title:
        query["title"] = args.title
    if args.cursor:
        query["cursor"] = args.cursor
    answer, site = wiki(args, "GET", f"/wiki/api/v2/spaces/{space}/pages", query=query)
    out({"results": [page_summary(p, site) for p in answer.get("results") or []], "cursor": cursor_of(answer)})


def read_page(args, page, form="storage"):
    return wiki(args, "GET", f"/wiki/api/v2/pages/{page}", query={"body-format": form})


def pages_get(args):
    page = checked(args.id, NUMBER, "page")
    got, site = read_page(args, page, args.format)
    if args.raw:
        return out(got)
    out({**page_summary(got, site), "format": args.format, "body": ((got.get("body") or {}).get(args.format) or {}).get("value")})


def pages_create(args):
    title = args.title.strip()
    if not title:
        raise ApiError(0, "bad_title", "a page needs a title")
    parent = checked(args.parent, NUMBER, "page") if args.parent else None
    body = storage(args)
    page = {"spaceId": space_id(args, args.space), "status": "draft" if args.draft else "current", "title": title, "body": {"representation": "storage", "value": body}}
    if parent:
        page["parentId"] = parent
    made, site = wiki(args, "POST", "/wiki/api/v2/pages", page)
    out(page_summary(made, site))


def pages_update(args):
    page = checked(args.id, NUMBER, "page")
    if args.version is not None and args.version < 1:
        raise ApiError(0, "bad_version", "--version is the version you read, from 1")
    if args.title is not None and not args.title.strip():
        raise ApiError(0, "bad_title", "a page needs a title")
    given = args.body is not None or args.text is not None
    # what isn't given is the page's own, read now
    current = {} if given and args.title and args.version is not None else read_page(args, page)[0]
    read = args.version if args.version is not None else (current.get("version") or {}).get("number")
    if not isinstance(read, int):
        raise ApiError(0, "no_version", "Confluence named no version for the page: give --version, the one you read")
    body = storage(args) if given else ((current.get("body") or {}).get("storage") or {}).get("value", "")
    update = {"id": page, "status": "current", "title": args.title.strip() if args.title else current.get("title"),
              "body": {"representation": "storage", "value": body}, "version": {"number": read + 1}}
    if args.message:
        update["version"]["message"] = args.message
    made, site = wiki(args, "PUT", f"/wiki/api/v2/pages/{page}", update)
    out(page_summary(made, site))


def raw(args):
    try:
        body = json.loads(args.json) if args.json else None
    except ValueError as exc:
        raise ApiError(0, "bad_json", f"--json is not JSON: {exc}") from None
    path, _, query = args.path.partition("?")
    answer, _ = wiki(args, args.method.upper(), path, body, urllib.parse.parse_qs(query) if query else None)
    out(answer)


def parser():
    top = argparse.ArgumentParser(description="Confluence through the person's connected Atlassian account")
    on_site = argparse.ArgumentParser(add_help=False)
    on_site.add_argument("--site", help="the site's name, its host (acme.atlassian.net) or its cloud id, where the person granted several")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the Atlassian connection answers for this agent, and its sites").set_defaults(func=check)
    sub.add_parser("sites", help="the sites the person granted, each with its cloud id and scopes").set_defaults(func=sites)

    p = sub.add_parser("search", parents=[on_site], help="content a CQL query finds")
    p.add_argument("cql", help="a CQL query, e.g. 'type = page AND space = ENG AND text ~ \"roadmap\"'")
    p.add_argument("--max", type=int, default=25)
    p.add_argument("--cursor", help="the cursor of the page before's answer")
    p.set_defaults(func=search)

    p = sub.add_parser("spaces", parents=[on_site], help="the spaces the person may see")
    p.add_argument("--keys", help="spaces' keys, comma-separated (ENG,OPS)")
    p.add_argument("--max", type=int, default=25)
    p.add_argument("--cursor")
    p.set_defaults(func=spaces)

    pages = sub.add_parser("pages").add_subparsers(dest="action", required=True)
    p = pages.add_parser("list", parents=[on_site], help="a space's pages, newest change first")
    p.add_argument("--space", required=True, help="its key (ENG) or id")
    p.add_argument("--title", help="a page's exact title")
    p.add_argument("--max", type=int, default=25)
    p.add_argument("--cursor")
    p.set_defaults(func=pages_list)
    p = pages.add_parser("get", parents=[on_site])
    p.add_argument("id", help="the page's id")
    p.add_argument("--format", choices=FORMATS, default="storage", help="the body's format: storage (XHTML), atlas_doc_format (ADF) or view (HTML)")
    p.add_argument("--raw", action="store_true", help="Confluence's answer as it is")
    p.set_defaults(func=pages_get)
    for action, func in [("create", pages_create), ("update", pages_update)]:
        p = pages.add_parser(action, parents=[on_site])
        if action == "create":
            p.add_argument("--space", required=True, help="its key (ENG) or id")
            p.add_argument("--title", required=True)
            p.add_argument("--parent", help="the parent page's id")
            p.add_argument("--draft", action="store_true", help="a draft, not published")
        else:
            p.add_argument("id", help="the page's id")
            p.add_argument("--title", help="a new title (else its own)")
            p.add_argument("--version", type=int, help="the version you read (`pages get`): Confluence refuses the change if the page changed since")
            p.add_argument("--message", help="what changed, kept with the version")
        body = p.add_mutually_exclusive_group(required=action == "create")
        body.add_argument("--text", help="plain text, its blank lines paragraphs")
        body.add_argument("--body", help="the body in Confluence's storage format (XHTML)")
        p.set_defaults(func=func)

    p = sub.add_parser("request", parents=[on_site], help="any call on the site's Confluence API")
    p.add_argument("method", choices=["GET", "POST", "PUT", "PATCH", "DELETE", "get", "post", "put", "patch", "delete"])
    p.add_argument("path", help="from /wiki/api/v2/ or /wiki/rest/api/, its query after ?")
    p.add_argument("--json", default="", help="the request's JSON body")
    p.set_defaults(func=raw)
    return top


HINTS = {
    "not_connected": "the person has not connected Atlassian (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left Atlassian out",
    "unauthorized": "Atlassian refused the connection's token: ask the person to reconnect Atlassian in Settings, under Connections",
    "scope_does_not_match": "the connection may not do this: connected read only, or without Confluence (the person adds either in Settings, under Connections)",
    "no_permission": "the person's Confluence permissions don't allow it in this space",
    "not_found": "no such page or space, or the person may not see it",
    "no_such_space": "`spaces` lists the spaces the person may see",
    "conflict": "the page changed since you read it: get it again, and make your change on that version",
    "choose_site": "name the site with --site; `sites` lists them",
    "no_such_site": "`sites` lists the sites the person granted; they add one by connecting Atlassian again in Settings (Disconnect, then Connect) and picking it",
    "no_site": "the person didn't allow Confluence (they add it in Settings, under Connections: Reconnect to add), or no site they picked has Confluence",
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
