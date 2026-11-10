#!/usr/bin/env python3
"""Linear through the person's connected Linear account.

The computer holds no Linear credential: its environment holds the
connection's placeholder, which names the agent (`LINEAR_OAUTH_ACCESS_TOKEN`),
each request sends it as a bearer token to api.linear.app, and the computer's
intercept swaps in a short-lived token for the agent's owner
(docs/computers.md, Connections and operator keys). Python's standard library
only; every command prints JSON.

Usage:
  python linear_api.py check
  python linear_api.py teams
  python linear_api.py issues search "login bug" [--max 10]
  python linear_api.py issues list [--team ENG] [--mine] [--state "In Progress"] [--max 25]
  python linear_api.py issues get ENG-123
  python linear_api.py issues create --team ENG --title "…" [--description "…"] [--priority 2] [--assign-me]
  python linear_api.py issues update ENG-123 [--title …] [--description …] [--state "Done"] [--priority 1] [--assign-me]
  python linear_api.py issues comment ENG-123 --body "…"
  python linear_api.py projects [--max 25]
  python linear_api.py graphql 'query { viewer { id } }' [--variables '{"a": 1}']

Environment:
  LINEAR_OAUTH_ACCESS_TOKEN  the Linear connection's placeholder (set on a
                             fragment computer once the person connects
                             Linear), or any Linear OAuth access token
"""

import argparse
import json
import os
import sys
import urllib.error
import urllib.request

GRAPHQL = "https://api.linear.app/graphql"
TIMEOUT_S = 60
# Linear pages at most 250 nodes; a helper's lists stay small
MAX_NODES = 100
ISSUE_FIELDS = "identifier title url priorityLabel updatedAt state { name } team { key } assignee { name email }"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is never followed: it would carry the Authorization
    header to wherever it points."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


class LinearError(Exception):
    def __init__(self, status, code, message):
        super().__init__(message)
        self.status = status
        self.code = code


def token():
    value = os.getenv("LINEAR_OAUTH_ACCESS_TOKEN", "").strip()
    if not value:
        raise LinearError(0, "not_connected", "LINEAR_OAUTH_ACCESS_TOKEN is not set: the person has not connected Linear, or narrowed you from it")
    return value


def graphql(query, variables=None):
    body = json.dumps({"query": query, "variables": variables or {}}).encode("utf-8")
    headers = {"Authorization": f"Bearer {token()}", "Content-Type": "application/json", "Accept": "application/json"}
    request = urllib.request.Request(GRAPHQL, data=body, headers=headers, method="POST")
    try:
        with OPENER.open(request, timeout=TIMEOUT_S) as response:
            answer = json.loads(response.read().decode("utf-8") or "{}")
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise LinearError(exc.code, "redirected", f"Linear answered a redirect to {exc.headers.get('Location', '?')}, which the helper never follows with the token") from None
        raise refusal(exc.code, exc.read().decode("utf-8", errors="replace")) from None
    except urllib.error.URLError as exc:
        raise LinearError(0, "unreachable", str(exc.reason)) from None
    if answer.get("errors"):
        raise graphql_error(200, answer["errors"])
    return answer.get("data") or {}


def refusal(status, text):
    """An HTTP refusal: the computer's own (`{error: "not_connected" |
    "forbidden", message}`), or Linear's GraphQL errors."""
    try:
        err = json.loads(text)
    except ValueError:
        err = {}
    if isinstance(err, dict) and isinstance(err.get("error"), str):
        return LinearError(status, err["error"], err.get("message") or err["error"])
    if isinstance(err, dict) and err.get("errors"):
        return graphql_error(status, err["errors"])
    return LinearError(status, "", text[:500] or f"HTTP {status}")


def graphql_error(status, errors):
    first = errors[0] if isinstance(errors, list) and errors and isinstance(errors[0], dict) else {}
    ext = first.get("extensions") or {}
    kind = str(ext.get("code") or ext.get("type") or "").lower().replace(" ", "_")
    message = ext.get("userPresentableMessage") or first.get("message") or "Linear refused the request"
    return LinearError(status, kind or "graphql_error", message)


def out(value):
    print(json.dumps(value, indent=2, ensure_ascii=False))


def bounded(n):
    return max(1, min(n, MAX_NODES))


def check(_args):
    data = graphql("query { viewer { name email } organization { name urlKey } }")
    me, org = data.get("viewer") or {}, data.get("organization") or {}
    out({"status": "connected", "name": me.get("name"), "email": me.get("email"), "workspace": org.get("name"), "url_key": org.get("urlKey")})


def teams(_args):
    out(graphql(f"query {{ teams(first: {MAX_NODES}) {{ nodes {{ id key name }} }} }}")["teams"]["nodes"])


def team_id(key):
    nodes = graphql("query($key: String!) { teams(filter: { key: { eqIgnoreCase: $key } }) { nodes { id } } }", {"key": key})["teams"]["nodes"]
    if not nodes:
        raise LinearError(0, "no_such_team", f"no team with key {key!r}: `teams` lists them")
    return nodes[0]["id"]


def viewer_id():
    return graphql("query { viewer { id } }")["viewer"]["id"]


def issue_search(args):
    q = f"query($term: String!, $first: Int) {{ searchIssues(term: $term, first: $first) {{ nodes {{ {ISSUE_FIELDS} }} }} }}"
    out(graphql(q, {"term": args.term, "first": bounded(args.max)})["searchIssues"]["nodes"])


def issue_list(args):
    f = {}
    if args.team:
        f["team"] = {"key": {"eqIgnoreCase": args.team}}
    if args.mine:
        f["assignee"] = {"isMe": {"eq": True}}
    if args.state:
        f["state"] = {"name": {"eqIgnoreCase": args.state}}
    q = f"query($filter: IssueFilter, $first: Int) {{ issues(filter: $filter, first: $first, orderBy: updatedAt) {{ nodes {{ {ISSUE_FIELDS} }} }} }}"
    out(graphql(q, {"filter": f, "first": bounded(args.max)})["issues"]["nodes"])


def issue_get(args):
    q = f"""query($id: String!) {{ issue(id: $id) {{ {ISSUE_FIELDS} description dueDate project {{ name }}
      labels {{ nodes {{ name }} }} comments(first: 50) {{ nodes {{ body createdAt user {{ name }} }} }} }} }}"""
    out(graphql(q, {"id": args.issue})["issue"])


def state_id(issue, name):
    q = """query($id: String!, $name: String!) { issue(id: $id) { team { states(filter: { name: { eqIgnoreCase: $name } }) { nodes { id } } } } }"""
    nodes = graphql(q, {"id": issue, "name": name})["issue"]["team"]["states"]["nodes"]
    if not nodes:
        raise LinearError(0, "no_such_state", f"the issue's team has no workflow state {name!r}")
    return nodes[0]["id"]


def changes(args):
    c = {k: v for k, v in (("title", args.title), ("description", args.description), ("priority", args.priority)) if v is not None}
    if args.assign_me:
        c["assigneeId"] = viewer_id()
    return c


def issue_create(args):
    fields = {"teamId": team_id(args.team), **changes(args)}
    q = f"mutation($input: IssueCreateInput!) {{ issueCreate(input: $input) {{ success issue {{ {ISSUE_FIELDS} }} }} }}"
    out(graphql(q, {"input": fields})["issueCreate"])


def issue_update(args):
    fields = changes(args)
    if args.state:
        fields["stateId"] = state_id(args.issue, args.state)
    if not fields:
        raise LinearError(0, "nothing_to_change", "name at least one of --title, --description, --priority, --state, --assign-me")
    q = f"mutation($id: String!, $input: IssueUpdateInput!) {{ issueUpdate(id: $id, input: $input) {{ success issue {{ {ISSUE_FIELDS} }} }} }}"
    out(graphql(q, {"id": args.issue, "input": fields})["issueUpdate"])


def issue_comment(args):
    issue = graphql("query($id: String!) { issue(id: $id) { id } }", {"id": args.issue})["issue"]["id"]
    q = "mutation($input: CommentCreateInput!) { commentCreate(input: $input) { success comment { id url } } }"
    out(graphql(q, {"input": {"issueId": issue, "body": args.body}})["commentCreate"])


def projects(args):
    q = "query($first: Int) { projects(first: $first, orderBy: updatedAt) { nodes { id name status { name } url targetDate lead { name } } } }"
    out(graphql(q, {"first": bounded(args.max)})["projects"]["nodes"])


def raw(args):
    try:
        variables = json.loads(args.variables) if args.variables else {}
    except ValueError as exc:
        raise LinearError(0, "bad_variables", f"--variables is not JSON: {exc}") from None
    out(graphql(args.query, variables))


def edit_flags(p, title_required):
    p.add_argument("--title", required=title_required)
    p.add_argument("--description", help="Markdown")
    p.add_argument("--priority", type=int, choices=range(0, 5), help="0 none, 1 urgent, 2 high, 3 medium, 4 low")
    p.add_argument("--assign-me", action="store_true", help="assign it to the connected account")


def parser():
    top = argparse.ArgumentParser(description="Linear through the person's connected Linear account")
    sub = top.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="whether the Linear connection answers for this agent").set_defaults(func=check)
    sub.add_parser("teams").set_defaults(func=teams)
    p = sub.add_parser("projects")
    p.add_argument("--max", type=int, default=25)
    p.set_defaults(func=projects)
    p = sub.add_parser("graphql", help="any query or mutation")
    p.add_argument("query")
    p.add_argument("--variables", default="")
    p.set_defaults(func=raw)

    issues = sub.add_parser("issues").add_subparsers(dest="action", required=True)
    p = issues.add_parser("search")
    p.add_argument("term")
    p.add_argument("--max", type=int, default=10)
    p.set_defaults(func=issue_search)
    p = issues.add_parser("list")
    p.add_argument("--team", help="a team's key, e.g. ENG")
    p.add_argument("--mine", action="store_true", help="assigned to the connected account")
    p.add_argument("--state", help="a workflow state's name, e.g. 'In Progress'")
    p.add_argument("--max", type=int, default=25)
    p.set_defaults(func=issue_list)
    p = issues.add_parser("get")
    p.add_argument("issue", help="its identifier (ENG-123) or id")
    p.set_defaults(func=issue_get)
    p = issues.add_parser("create")
    p.add_argument("--team", required=True, help="a team's key, e.g. ENG")
    edit_flags(p, title_required=True)
    p.set_defaults(func=issue_create)
    p = issues.add_parser("update")
    p.add_argument("issue", help="its identifier (ENG-123) or id")
    p.add_argument("--state", help="a workflow state's name, e.g. Done")
    edit_flags(p, title_required=False)
    p.set_defaults(func=issue_update)
    p = issues.add_parser("comment")
    p.add_argument("issue", help="its identifier (ENG-123) or id")
    p.add_argument("--body", required=True, help="Markdown")
    p.set_defaults(func=issue_comment)
    return top


HINTS = {
    "not_connected": "the person has not connected Linear (or must again): ask them to, in the shell's Settings, under Connections",
    "forbidden": "the person narrowed this agent's connections and left Linear out",
    "authentication_error": "Linear refused the connection's token: ask the person to reconnect Linear in Settings, under Connections",
}


def main():
    args = parser().parse_args()
    try:
        args.func(args)
    except LinearError as e:
        print(json.dumps({"error": e.code or "linear_error", "status": e.status, "message": str(e), "hint": HINTS.get(e.code, "")}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
