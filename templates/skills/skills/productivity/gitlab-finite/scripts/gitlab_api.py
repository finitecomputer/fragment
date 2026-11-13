#!/usr/bin/env python3
"""GitLab.com through the person's connected GitLab account.

The computer holds no GitLab credential: its environment holds the
connection's placeholder, which names the agent (`GITLAB_OAUTH_ACCESS_TOKEN`),
each request sends it as a bearer token to gitlab.com, and the computer's
intercept swaps in a short-lived token for the agent's owner
(docs/computers.md, Connections and operator keys). The token is swapped on
every path of gitlab.com, so the helper calls GitLab's REST API, under
/api/v4/, alone. Python's standard library only; every command prints JSON.

Usage:
  python gitlab_api.py check
  python gitlab_api.py projects list [--search api] [--max 20]
  python gitlab_api.py projects get group/project
  python gitlab_api.py issues list [--project group/project] [--mine] [--state opened] [--search "…"] [--labels bug,ui] [--max 20]
  python gitlab_api.py issues get group/project 42
  python gitlab_api.py issues create group/project --title "…" [--description "…"] [--labels bug] [--assign-me]
  python gitlab_api.py issues comment group/project 42 --body "…"
  python gitlab_api.py mrs list [--project group/project] [--mine | --reviewing] [--state opened] [--search "…"] [--max 20]
  python gitlab_api.py mrs get group/project 17
  python gitlab_api.py mrs diffs group/project 17 [--max 20]
  python gitlab_api.py mrs comment group/project 17 --body "…"
  python gitlab_api.py pipelines list group/project [--ref main] [--status failed] [--max 20]
  python gitlab_api.py pipelines get group/project 123456
  python gitlab_api.py jobs list group/project 123456 [--max 50]
  python gitlab_api.py jobs get group/project 987654
  python gitlab_api.py jobs log group/project 987654 [--tail 200] [--out job.log]
  python gitlab_api.py files tree group/project [src] [--ref main] [--recursive] [--max 100]
  python gitlab_api.py files get group/project src/app.py [--ref main] [--out app.py]
  python gitlab_api.py files archive group/project [--ref main] [--format tar.gz] --out project.tar.gz
  python gitlab_api.py request GET "/api/v4/projects/group%2Fproject/repository/branches?per_page=50" [--json '{…}']

Environment:
  GITLAB_OAUTH_ACCESS_TOKEN  the GitLab connection's placeholder (set on a
                             fragment computer once the person connects
                             GitLab), or any GitLab.com OAuth access token
"""

import argparse
import contextlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

GITLAB = "https://gitlab.com"
TIMEOUT_S = 60
# GitLab pages at most 100 items; a helper's lists stay small
MAX_ITEMS = 100
# an answer read into memory, and a download written to a file, at most
MAX_READ_BYTES = 64 * 1024 * 1024
MAX_SAVE_BYTES = 2 * 1024 * 1024 * 1024
# a project's numeric id, an issue's or merge request's number (iid), a
# pipeline's or job's id
NUMBER = re.compile(r"^[1-9][0-9]{0,19}$")
# a group's or project's path segment, as GitLab allows one
SEGMENT = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_.\-]{0,254}$")
# a branch, tag or commit
REF = re.compile(r"^[^\s\x00-\x1f\x7f]{1,255}$")
# a path on GitLab's REST API, percent-encoded where it must be
API_PATH = re.compile(r"^/api/v4/[A-Za-z0-9/_\-.~%:@!$&'()*+,;=]*$")
# a job log's colors and its collapsible sections' markers
LOG_MARKUP = re.compile(r"\x1b\[[0-9;]*[A-Za-z]|section_(?:start|end):[0-9]+:[A-Za-z0-9_.\-\[\]=,]+\r?")
ISSUE_FIELDS = ("iid", "title", "state", "web_url", "labels", "updated_at")
MR_FIELDS = ISSUE_FIELDS + ("source_branch", "target_branch", "draft")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


class GitLabError(Exception):
    def __init__(self, status, code, message):
        super().__init__(message)
        self.status = status
        self.code = code


def token():
    value = os.getenv("GITLAB_OAUTH_ACCESS_TOKEN", "").strip()
    if not value:
        raise GitLabError(0, "not_connected", "GITLAB_OAUTH_ACCESS_TOKEN is not set: the person has not connected GitLab, or narrowed you from it")
    return value


def api_path(path):
    """`path`, when it stays under /api/v4/ on gitlab.com: no `.` or `..`
    segment, however often it is percent-decoded."""
    seen = path
    for _ in range(4):
        if any(seg in (".", "..") for seg in seen.split("/")):
            break
        seen = urllib.parse.unquote(seen)
    else:
        if API_PATH.match(path):
            return path
    raise GitLabError(0, "bad_path", "a path on GitLab's API, from /api/v4/ (e.g. /api/v4/projects/group%2Fproject/issues)")


def call(method, path, query=None, body=None, raw=False, out=None):
    """`method` `path` (from /api/v4/) on gitlab.com: its answer read as
    JSON, as bytes (`raw`), or written to the file `out`."""
    url = GITLAB + api_path(path) + (("?" + urllib.parse.urlencode(query, quote_via=urllib.parse.quote)) if query else "")
    data = None if body is None else json.dumps(body).encode("utf-8")
    headers = {"Authorization": f"Bearer {token()}", "Accept": "*/*" if raw or out else "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with OPENER.open(request, timeout=TIMEOUT_S) as response:
            if out:
                return {"saved": out, "bytes": save(response, out)}
            answer = response.read(MAX_READ_BYTES + 1)
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise GitLabError(exc.code, "redirected", f"GitLab answered a redirect to {exc.headers.get('Location', '?')}, which the helper never follows with the token") from None
        raise refusal(exc.code, exc.read().decode("utf-8", errors="replace")) from None
    except urllib.error.URLError as exc:
        raise GitLabError(0, "unreachable", str(exc.reason)) from None
    except OSError as exc:
        # the answer cut off as it was read (a timeout, a reset)
        raise GitLabError(0, "unreachable", str(exc) or type(exc).__name__) from None
    if len(answer) > MAX_READ_BYTES:
        raise GitLabError(0, "too_large", f"GitLab's answer is over {MAX_READ_BYTES} bytes: ask for less, or write it to a file with --out")
    if raw:
        return answer
    try:
        return json.loads(answer.decode("utf-8")) if answer.strip() else {}
    except ValueError:
        return {"text": answer.decode("utf-8", errors="replace")}


def save(response, out):
    """The answer written to `out`, at most `MAX_SAVE_BYTES`: its size. A
    download that fails leaves no part of itself behind."""

    def unwritable(exc):
        return GitLabError(0, "unwritable", f"{out}: {exc.strerror or exc}")

    try:
        f = open(out, "wb")
    except OSError as exc:
        raise unwritable(exc) from None
    written = 0
    try:
        with f:
            while chunk := response.read(1 << 20):
                written += len(chunk)
                if written > MAX_SAVE_BYTES:
                    raise GitLabError(0, "too_large", f"the download is over {MAX_SAVE_BYTES} bytes")
                try:
                    f.write(chunk)
                except OSError as exc:
                    raise unwritable(exc) from None
    except BaseException:
        with contextlib.suppress(OSError):
            os.remove(out)
        raise
    return written


# GitLab's refusals that name no word of their own, by status
BY_STATUS = {400: "bad_request", 401: "unauthorized", 403: "not_allowed", 404: "not_found", 409: "conflict", 422: "unprocessable", 429: "rate_limited"}
WORD = re.compile(r"^[a-z][a-z_]{0,63}$")


def refusal(status, text):
    """An HTTP refusal: the computer's own (`{error: "not_connected" |
    "forbidden", message}`), GitLab's OAuth one (`{error: "invalid_token" |
    "insufficient_scope", error_description}`), or its API's (`{message}`,
    or `{error}` naming a parameter)."""
    try:
        err = json.loads(text)
    except ValueError:
        err = None
    if isinstance(err, dict):
        word = err.get("error")
        message = err.get("message") or err.get("error_description") or word or f"HTTP {status}"
        if not isinstance(message, str):
            message = json.dumps(message, ensure_ascii=False)
        if isinstance(word, str) and WORD.match(word):
            return GitLabError(status, word, message)
        return GitLabError(status, BY_STATUS.get(status, ""), message)
    return GitLabError(status, BY_STATUS.get(status, ""), text[:500] or f"HTTP {status}")


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def bounded(n):
    return max(1, min(n, MAX_ITEMS))


def project(value):
    """A project's numeric id, or its path (group/subgroup/project)
    URL-encoded: group%2Fsubgroup%2Fproject."""
    if NUMBER.match(value):
        return value
    parts = value.split("/")
    if not 2 <= len(parts) <= 21 or not all(SEGMENT.match(p) for p in parts):
        raise GitLabError(0, "bad_project", f"{value!r} is not a project: its path, as in its URL (group/project), or its numeric id")
    return urllib.parse.quote(value, safe="")


def number(value, what):
    if not NUMBER.match(value):
        raise GitLabError(0, "bad_" + what, f"{value!r} is not a {what}: a number")
    return value


def repo_path(value):
    """A path in a repository: no empty, `.` or `..` segment."""
    path = value.strip("/")
    if not path or len(path) > 4096 or any(p in ("", ".", "..") for p in path.split("/")) or re.search(r"[\x00-\x1f\x7f]", path):
        raise GitLabError(0, "bad_path", f"{value!r} is not a path in the repository (e.g. src/app.py)")
    return path


def ref(value):
    if not REF.match(value):
        raise GitLabError(0, "bad_ref", f"{value!r} is not a branch, tag or commit")
    return value


def username(person):
    return (person or {}).get("username")


def brief(item, fields):
    """An issue or merge request, shortened."""
    kept = {k: item.get(k) for k in fields}
    kept["author"] = username(item.get("author"))
    kept["assignees"] = [username(a) for a in item.get("assignees") or []]
    return kept


def comments(path):
    """The comments under an issue or merge request, oldest first (not
    GitLab's own notes of changes)."""
    notes = call("GET", path, {"sort": "asc", "order_by": "created_at", "per_page": MAX_ITEMS})
    return [{"author": username(n.get("author")), "body": n.get("body"), "created_at": n.get("created_at")} for n in notes if isinstance(n, dict) and not n.get("system")] if isinstance(notes, list) else []


def note(answer):
    return {"id": answer.get("id"), "author": username(answer.get("author")), "created_at": answer.get("created_at")}


def check(_args):
    me = call("GET", "/api/v4/user")
    out({"status": "connected", "username": me.get("username"), "name": me.get("name"), "email": me.get("email"), "web_url": me.get("web_url")})


def projects_list(args):
    query = {"membership": "true", "simple": "true", "order_by": "last_activity_at", "per_page": bounded(args.max)}
    if args.search:
        query["search"] = args.search
    fields = ("id", "path_with_namespace", "name", "description", "default_branch", "web_url", "last_activity_at")
    out([{k: p.get(k) for k in fields} for p in call("GET", "/api/v4/projects", query)])


def projects_get(args):
    out(call("GET", f"/api/v4/projects/{project(args.project)}"))


def listing(args, scope):
    query = {"state": args.state, "order_by": "updated_at", "per_page": bounded(args.max)}
    if args.search:
        query["search"] = args.search
    if scope:
        query["scope"] = scope
    return query


def issues_list(args):
    where = f"/api/v4/projects/{project(args.project)}/issues" if args.project else "/api/v4/issues"
    query = listing(args, "assigned_to_me" if args.mine else None)
    if args.labels:
        query["labels"] = args.labels
    out([brief(i, ISSUE_FIELDS) for i in call("GET", where, query)])


def issues_get(args):
    base = f"/api/v4/projects/{project(args.project)}/issues/{number(args.iid, 'iid')}"
    issue = call("GET", base)
    out({**brief(issue, ISSUE_FIELDS + ("description", "created_at", "due_date")), "milestone": (issue.get("milestone") or {}).get("title"), "notes": comments(base + "/notes")})


def me_id():
    return call("GET", "/api/v4/user")["id"]


def issues_create(args):
    where = f"/api/v4/projects/{project(args.project)}/issues"
    body = {"title": args.title}
    if args.description:
        body["description"] = args.description
    if args.labels:
        body["labels"] = args.labels
    if args.assign_me:
        body["assignee_ids"] = [me_id()]
    out(brief(call("POST", where, body=body), ISSUE_FIELDS))


def issues_comment(args):
    where = f"/api/v4/projects/{project(args.project)}/issues/{number(args.iid, 'iid')}/notes"
    out(note(call("POST", where, body={"body": args.body})))


def mrs_list(args):
    where = f"/api/v4/projects/{project(args.project)}/merge_requests" if args.project else "/api/v4/merge_requests"
    scope = "assigned_to_me" if args.mine else "reviews_for_me" if args.reviewing else None
    out([brief(m, MR_FIELDS) for m in call("GET", where, listing(args, scope))])


def mrs_get(args):
    base = f"/api/v4/projects/{project(args.project)}/merge_requests/{number(args.iid, 'iid')}"
    mr = call("GET", base)
    pipeline = mr.get("head_pipeline") or {}
    out({
        **brief(mr, MR_FIELDS + ("description", "created_at", "sha", "detailed_merge_status")),
        "reviewers": [username(r) for r in mr.get("reviewers") or []],
        "head_pipeline": {k: pipeline.get(k) for k in ("id", "status", "web_url")} if pipeline else None,
        "notes": comments(base + "/notes"),
    })


def mrs_diffs(args):
    where = f"/api/v4/projects/{project(args.project)}/merge_requests/{number(args.iid, 'iid')}/diffs"
    out(call("GET", where, {"per_page": bounded(args.max)}))


def mrs_comment(args):
    where = f"/api/v4/projects/{project(args.project)}/merge_requests/{number(args.iid, 'iid')}/notes"
    out(note(call("POST", where, body={"body": args.body})))


def pipelines_list(args):
    where = f"/api/v4/projects/{project(args.project)}/pipelines"
    query = {"per_page": bounded(args.max)}
    if args.ref is not None:
        query["ref"] = ref(args.ref)
    if args.status:
        query["status"] = args.status
    out(call("GET", where, query))


def pipelines_get(args):
    out(call("GET", f"/api/v4/projects/{project(args.project)}/pipelines/{number(args.pipeline, 'pipeline_id')}"))


def jobs_list(args):
    where = f"/api/v4/projects/{project(args.project)}/pipelines/{number(args.pipeline, 'pipeline_id')}/jobs"
    fields = ("id", "name", "stage", "status", "ref", "allow_failure", "failure_reason", "duration", "finished_at", "web_url")
    out([{k: j.get(k) for k in fields} for j in call("GET", where, {"per_page": bounded(args.max)})])


def jobs_get(args):
    out(call("GET", f"/api/v4/projects/{project(args.project)}/jobs/{number(args.job, 'job_id')}"))


def jobs_log(args):
    where = f"/api/v4/projects/{project(args.project)}/jobs/{number(args.job, 'job_id')}/trace"
    if args.out:
        return out(call("GET", where, out=args.out))
    lines = LOG_MARKUP.sub("", call("GET", where, raw=True).decode("utf-8", errors="replace")).splitlines()
    shown = lines[-args.tail:] if args.tail > 0 else lines
    out({"project": args.project, "job": int(args.job), "lines": len(lines), "log": "\n".join(shown)})


def files_tree(args):
    where = f"/api/v4/projects/{project(args.project)}/repository/tree"
    query = {"per_page": bounded(args.max)}
    if args.path:
        query["path"] = repo_path(args.path)
    if args.ref is not None:
        query["ref"] = ref(args.ref)
    if args.recursive:
        query["recursive"] = "true"
    out(call("GET", where, query))


def files_get(args):
    path = repo_path(args.path)
    # encoded whole, its dots too, as GitLab's docs show (lib%2Fclass%2Erb)
    where = f"/api/v4/projects/{project(args.project)}/repository/files/{urllib.parse.quote(path, safe='').replace('.', '%2E')}/raw"
    query = {"ref": ref(args.ref)} if args.ref is not None else None
    if args.out:
        return out(call("GET", where, query, out=args.out))
    try:
        content = call("GET", where, query, raw=True).decode("utf-8")
    except UnicodeDecodeError:
        raise GitLabError(0, "binary", f"{path} is not UTF-8 text: write it to a file with --out") from None
    out({"project": args.project, "path": path, "ref": args.ref, "content": content})


def files_archive(args):
    where = f"/api/v4/projects/{project(args.project)}/repository/archive.{args.format}"
    out(call("GET", where, {"sha": ref(args.ref)} if args.ref is not None else None, out=args.out))


def raw(args):
    try:
        body = json.loads(args.json) if args.json else None
    except ValueError as exc:
        raise GitLabError(0, "bad_json", f"--json is not JSON: {exc}") from None
    path, _, query = args.path.partition("?")
    out(call(args.method.upper(), path, urllib.parse.parse_qsl(query, keep_blank_values=True) if query else None, body))


def parser():
    top = argparse.ArgumentParser(description="GitLab.com through the person's connected GitLab account")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the GitLab connection answers for this agent").set_defaults(func=check)
    a_project = "its path (group/project) or numeric id"

    projects = sub.add_parser("projects").add_subparsers(dest="action", required=True)
    p = projects.add_parser("list", help="the projects the account is a member of, latest activity first")
    p.add_argument("--search")
    p.add_argument("--max", type=int, default=20)
    p.set_defaults(func=projects_list)
    p = projects.add_parser("get")
    p.add_argument("project", help=a_project)
    p.set_defaults(func=projects_get)

    issues = sub.add_parser("issues").add_subparsers(dest="action", required=True)
    p = issues.add_parser("list", help="a project's issues; with no --project, those you opened (or, --mine, are assigned)")
    p.add_argument("--project", help=a_project)
    p.add_argument("--mine", action="store_true", help="assigned to the connected account")
    p.add_argument("--state", default="opened", choices=["opened", "closed", "all"])
    p.add_argument("--search", help="in titles and descriptions")
    p.add_argument("--labels", help="comma-separated: issues with all of them")
    p.add_argument("--max", type=int, default=20)
    p.set_defaults(func=issues_list)
    p = issues.add_parser("get", help="with its comments")
    p.add_argument("project", help=a_project)
    p.add_argument("iid", help="its number in the project (#42)")
    p.set_defaults(func=issues_get)
    p = issues.add_parser("create")
    p.add_argument("project", help=a_project)
    p.add_argument("--title", required=True)
    p.add_argument("--description", help="Markdown")
    p.add_argument("--labels", help="comma-separated")
    p.add_argument("--assign-me", action="store_true", help="assign it to the connected account")
    p.set_defaults(func=issues_create)
    p = issues.add_parser("comment")
    p.add_argument("project", help=a_project)
    p.add_argument("iid", help="its number in the project (#42)")
    p.add_argument("--body", required=True, help="Markdown")
    p.set_defaults(func=issues_comment)

    mrs = sub.add_parser("mrs", aliases=["merge-requests"]).add_subparsers(dest="action", required=True)
    p = mrs.add_parser("list", help="a project's merge requests; with no --project, those you opened (or, --mine, are assigned, or --reviewing, review)")
    p.add_argument("--project", help=a_project)
    whose = p.add_mutually_exclusive_group()
    whose.add_argument("--mine", action="store_true", help="assigned to the connected account")
    whose.add_argument("--reviewing", action="store_true", help="the connected account is a reviewer of")
    p.add_argument("--state", default="opened", choices=["opened", "closed", "locked", "merged", "all"])
    p.add_argument("--search", help="in titles and descriptions")
    p.add_argument("--max", type=int, default=20)
    p.set_defaults(func=mrs_list)
    for name, func, help_ in (("get", mrs_get, "with its comments"), ("diffs", mrs_diffs, "its changes, file by file")):
        p = mrs.add_parser(name, help=help_)
        p.add_argument("project", help=a_project)
        p.add_argument("iid", help="its number in the project (!17)")
        if name == "diffs":
            p.add_argument("--max", type=int, default=20, help="files")
        p.set_defaults(func=func)
    p = mrs.add_parser("comment")
    p.add_argument("project", help=a_project)
    p.add_argument("iid", help="its number in the project (!17)")
    p.add_argument("--body", required=True, help="Markdown")
    p.set_defaults(func=mrs_comment)

    pipelines = sub.add_parser("pipelines").add_subparsers(dest="action", required=True)
    p = pipelines.add_parser("list", help="newest first")
    p.add_argument("project", help=a_project)
    p.add_argument("--ref", help="a branch or tag")
    p.add_argument("--status", choices=["created", "waiting_for_resource", "preparing", "waiting_for_callback", "pending", "running", "success", "failed", "canceling", "canceled", "skipped", "manual", "scheduled"])
    p.add_argument("--max", type=int, default=20)
    p.set_defaults(func=pipelines_list)
    p = pipelines.add_parser("get")
    p.add_argument("project", help=a_project)
    p.add_argument("pipeline", help="its id")
    p.set_defaults(func=pipelines_get)

    jobs = sub.add_parser("jobs").add_subparsers(dest="action", required=True)
    p = jobs.add_parser("list", help="a pipeline's jobs")
    p.add_argument("project", help=a_project)
    p.add_argument("pipeline", help="its id")
    p.add_argument("--max", type=int, default=50)
    p.set_defaults(func=jobs_list)
    p = jobs.add_parser("get")
    p.add_argument("project", help=a_project)
    p.add_argument("job", help="its id")
    p.set_defaults(func=jobs_get)
    p = jobs.add_parser("log", help="its log, without colors")
    p.add_argument("project", help=a_project)
    p.add_argument("job", help="its id")
    p.add_argument("--tail", type=int, default=200, help="its last lines; 0 for all")
    p.add_argument("--out", help="write the whole log, as GitLab sends it, to this file")
    p.set_defaults(func=jobs_log)

    files = sub.add_parser("files").add_subparsers(dest="action", required=True)
    p = files.add_parser("tree", help="a folder's files and folders")
    p.add_argument("project", help=a_project)
    p.add_argument("path", nargs="?", default="", help="a folder in the repository; its root if none")
    p.add_argument("--ref", help="a branch, tag or commit; the default branch if none")
    p.add_argument("--recursive", action="store_true")
    p.add_argument("--max", type=int, default=100)
    p.set_defaults(func=files_tree)
    p = files.add_parser("get", help="a file's content")
    p.add_argument("project", help=a_project)
    p.add_argument("path", help="its path in the repository, e.g. src/app.py")
    p.add_argument("--ref", help="a branch, tag or commit; the default branch if none")
    p.add_argument("--out", help="write it to this file (a binary file must be)")
    p.set_defaults(func=files_get)
    p = files.add_parser("archive", help="the repository's files at a ref, as one archive (no history)")
    p.add_argument("project", help=a_project)
    p.add_argument("--ref", help="a branch, tag or commit; the default branch if none")
    p.add_argument("--format", default="tar.gz", choices=["tar.gz", "tar.bz2", "tar", "zip"])
    p.add_argument("--out", required=True, help="the file to write it to")
    p.set_defaults(func=files_archive)

    p = sub.add_parser("request", help="any GitLab REST API call")
    p.add_argument("method", choices=["GET", "POST", "PUT", "PATCH", "DELETE", "get", "post", "put", "patch", "delete"])
    p.add_argument("path", help="from /api/v4/, a project's path URL-encoded (group%%2Fproject), its query after ?")
    p.add_argument("--json", default="", help="the request's JSON body")
    p.set_defaults(func=raw)
    return top


READ_ONLY = "the person connected GitLab read only: they allow changes in Settings, under Connections"
HINTS = {
    "not_connected": "the person has not connected GitLab (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left GitLab out",
    "invalid_token": "GitLab refused the connection's token: ask the person to reconnect GitLab in Settings, under Connections",
    "unauthorized": "GitLab refused the connection's token: ask the person to reconnect GitLab in Settings, under Connections",
    "insufficient_scope": READ_ONLY,
    "not_allowed": "the person's role in that project or group does not allow it",
    "not_found": "the project, number or path is wrong, or the person's account cannot see it",
    "rate_limited": "GitLab.com's rate limit: wait a minute, then ask for less",
}


def main():
    args = parser().parse_args()
    try:
        args.func(args)
    except GitLabError as e:
        print(json.dumps({"error": e.code or "gitlab_error", "status": e.status, "message": str(e), "hint": HINTS.get(e.code, "")}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
