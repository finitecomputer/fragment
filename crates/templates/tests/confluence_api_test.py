"""The Confluence skill's helper (`confluence_api.py`) against a local
stand-in for api.atlassian.com, on 127.0.0.1: no network.

Run: python3 -I crates/templates/tests/confluence_api_test.py

The update keeps the line between a draft and a published page: Confluence's
v2 page update names the status it leaves the page in, `current` being
published, so an update never sends `current` for a draft unless asked to
publish it (--publish).
"""

import contextlib
import http.server
import importlib.util
import io
import json
import os
import pathlib
import threading
import unittest
import urllib.parse
import urllib.request
from unittest import mock

HELPER = pathlib.Path(__file__).resolve().parents[3] / "templates/skills/skills/productivity/confluence-finite/scripts/confluence_api.py"
CLOUD = "11111111-2222-3333-4444-555555555555"
WIKI = f"/ex/confluence/{CLOUD}/wiki"
PAGES = f"{WIKI}/api/v2/pages"


def load_helper():
    spec = importlib.util.spec_from_file_location("confluence_api", HELPER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Atlassian:
    """api.atlassian.com as the helper meets it: one Confluence site, its
    pages, and Confluence's version rule on update (the version sent is the
    page's plus one, else 409)."""

    def __init__(self, pages):
        self.pages = {k: dict(v) for k, v in pages.items()}
        self.requests = []

    def answer(self, method, path, query, body):
        self.requests.append({"method": method, "path": path, "query": query, "body": body})
        if method == "GET" and path == "/oauth/token/accessible-resources":
            return 200, [{"id": CLOUD, "name": "acme", "url": "https://acme.atlassian.net", "scopes": ["read:confluence-content.all", "write:confluence-content"]}]
        if method == "GET" and path == f"{WIKI}/api/v2/spaces":
            return 200, {"results": [{"id": "77", "key": "ENG"}]}
        if method == "POST" and path == PAGES:
            return 200, {"id": "900", "status": body["status"], "title": body["title"], "spaceId": body["spaceId"], "version": {"number": 1}}
        page_id = path[len(PAGES) + 1:] if path.startswith(PAGES + "/") else None
        page = self.pages.get(page_id)
        if page is None:
            return 404, {"errors": [{"status": 404, "title": "Not Found", "detail": "no such page"}]}
        if method == "GET":
            return 200, self.page_answer(page_id, page)
        if method == "PUT":
            if body["version"]["number"] != page["version"] + 1:
                return 409, {"errors": [{"status": 409, "title": "Conflict", "detail": f"Version must be incremented on update. Current version is: {page['version']}"}]}
            page.update(status=body["status"], title=body["title"], body=body["body"]["value"], version=body["version"]["number"])
            return 200, self.page_answer(page_id, page)
        return 405, {"message": "not here"}

    @staticmethod
    def page_answer(page_id, page):
        return {"id": page_id, "status": page["status"], "title": page["title"], "spaceId": "77", "parentId": None,
                "version": {"number": page["version"]}, "body": {"storage": {"representation": "storage", "value": page["body"]}},
                "_links": {"webui": f"/spaces/ENG/pages/{page_id}"}}

    def puts(self):
        return [r for r in self.requests if r["method"] == "PUT"]

    def page_reads(self, page_id):
        return [r for r in self.requests if r["method"] == "GET" and r["path"] == f"{PAGES}/{page_id}"]


def serve(atlassian):
    class Handler(http.server.BaseHTTPRequestHandler):
        def handle_one(self, method):
            url = urllib.parse.urlsplit(self.path)
            length = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(length)) if length else None
            status, answer = atlassian.answer(method, url.path, urllib.parse.parse_qs(url.query), body)
            data = json.dumps(answer).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            self.handle_one("GET")

        def do_PUT(self):
            self.handle_one("PUT")

        def do_POST(self):
            self.handle_one("POST")

        def log_message(self, *_args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True).start()
    return server


class UpdateTest(unittest.TestCase):
    def setUp(self):
        self.helper = load_helper()
        self.atlassian = Atlassian({
            "42": {"status": "draft", "title": "Draft plan", "body": "<p>draft words</p>", "version": 1},
            "50": {"status": "current", "title": "Runbook", "body": "<p>published words</p>", "version": 4},
            "60": {"status": "trashed", "title": "Old notes", "body": "<p>gone</p>", "version": 3},
        })
        self.server = serve(self.atlassian)
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.helper.API = f"http://127.0.0.1:{self.server.server_address[1]}"
        # the helper's own opener, minus any proxy the machine names
        self.helper.OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), self.helper.NoRedirect)
        env = mock.patch.dict(os.environ, {"ATLASSIAN_OAUTH_ACCESS_TOKEN": "placeholder"})
        env.start()
        self.addCleanup(env.stop)

    def run_helper(self, *argv):
        args = self.helper.parser().parse_args(list(argv))
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            args.func(args)
        return json.loads(printed.getvalue())

    def refused(self, *argv):
        with self.assertRaises(self.helper.ApiError) as caught:
            self.run_helper(*argv)
        return caught.exception

    # a draft stays a draft: no update without --publish

    def test_draft_title_only_is_refused(self):
        err = self.refused("pages", "update", "42", "--title", "Renamed")
        self.assertEqual(err.code, "draft")
        self.assertEqual(self.atlassian.puts(), [])
        self.assertEqual(self.atlassian.pages["42"]["status"], "draft")

    def test_draft_body_only_is_refused(self):
        err = self.refused("pages", "update", "42", "--text", "new words")
        self.assertEqual(err.code, "draft")
        self.assertEqual(self.atlassian.puts(), [])

    def test_draft_full_update_reads_the_status_and_is_refused(self):
        err = self.refused("pages", "update", "42", "--title", "Renamed", "--body", "<p>new</p>", "--version", "1")
        self.assertEqual(err.code, "draft")
        self.assertEqual(len(self.atlassian.page_reads("42")), 1)
        self.assertEqual(self.atlassian.puts(), [])

    def test_draft_with_message_is_refused(self):
        err = self.refused("pages", "update", "42", "--body", "<p>new</p>", "--message", "edit")
        self.assertEqual(err.code, "draft")
        self.assertEqual(self.atlassian.puts(), [])

    # --publish publishes a draft, as the person asked

    def test_draft_full_update_with_publish_publishes(self):
        made = self.run_helper("pages", "update", "42", "--title", "Final plan", "--body", "<p>final</p>", "--version", "1", "--publish")
        [put] = self.atlassian.puts()
        self.assertEqual(put["body"]["status"], "current")
        self.assertEqual(put["body"]["version"], {"number": 2})
        self.assertEqual(put["body"]["title"], "Final plan")
        self.assertEqual(put["body"]["body"], {"representation": "storage", "value": "<p>final</p>"})
        self.assertEqual(made["status"], "current")

    def test_draft_title_only_with_publish_keeps_its_body(self):
        self.run_helper("pages", "update", "42", "--title", "Final plan", "--publish")
        [put] = self.atlassian.puts()
        self.assertEqual(put["body"]["status"], "current")
        self.assertEqual(put["body"]["body"]["value"], "<p>draft words</p>")
        self.assertEqual(put["body"]["version"], {"number": 2})

    def test_draft_body_only_with_publish_keeps_its_title(self):
        self.run_helper("pages", "update", "42", "--text", "final words", "--publish")
        [put] = self.atlassian.puts()
        self.assertEqual(put["body"]["status"], "current")
        self.assertEqual(put["body"]["title"], "Draft plan")
        self.assertEqual(put["body"]["body"]["value"], "<p>final words</p>")

    # a published page's ordinary edits

    def test_published_title_only(self):
        made = self.run_helper("pages", "update", "50", "--title", "Runbook v2")
        [put] = self.atlassian.puts()
        self.assertEqual(put["path"], f"{PAGES}/50")
        self.assertEqual(put["body"], {"id": "50", "status": "current", "title": "Runbook v2",
                                       "body": {"representation": "storage", "value": "<p>published words</p>"}, "version": {"number": 5}})
        self.assertEqual(made["version"], 5)

    def test_published_body_only(self):
        self.run_helper("pages", "update", "50", "--text", "line one\nline two\n\nnext <para>")
        [put] = self.atlassian.puts()
        self.assertEqual(put["body"]["title"], "Runbook")
        self.assertEqual(put["body"]["body"]["value"], "<p>line one<br/>line two</p><p>next &lt;para&gt;</p>")
        self.assertEqual(put["body"]["status"], "current")

    def test_published_full_update_reads_the_status_and_sends_the_given_version(self):
        self.run_helper("pages", "update", "50", "--title", "Runbook v2", "--body", "<p>new</p>", "--version", "4", "--message", "Fixed the dates")
        self.assertEqual(len(self.atlassian.page_reads("50")), 1)
        [put] = self.atlassian.puts()
        self.assertEqual(put["body"]["status"], "current")
        self.assertEqual(put["body"]["version"], {"number": 5, "message": "Fixed the dates"})

    def test_publish_on_a_published_page_is_an_ordinary_edit(self):
        self.run_helper("pages", "update", "50", "--title", "Runbook v2", "--publish")
        [put] = self.atlassian.puts()
        self.assertEqual(put["body"]["status"], "current")
        self.assertEqual(put["body"]["version"], {"number": 5})

    # the version an update names is the one the agent read

    def test_stale_version_is_a_conflict(self):
        # read at 3; the page is at 4 now: the update names 3, Confluence refuses it
        err = self.refused("pages", "update", "50", "--title", "Mine", "--body", "<p>mine</p>", "--version", "3")
        self.assertEqual((err.code, err.status), ("conflict", 409))
        [put] = self.atlassian.puts()
        self.assertEqual(put["body"]["version"], {"number": 4})
        self.assertEqual(self.atlassian.pages["50"]["title"], "Runbook")

    def test_stale_version_on_a_partial_update_is_a_conflict(self):
        err = self.refused("pages", "update", "50", "--text", "mine", "--version", "2")
        self.assertEqual(err.code, "conflict")
        self.assertEqual(self.atlassian.pages["50"]["body"], "<p>published words</p>")

    def test_version_before_one_is_refused_before_any_call(self):
        err = self.refused("pages", "update", "50", "--title", "x", "--version", "0")
        self.assertEqual(err.code, "bad_version")
        self.assertEqual(self.atlassian.requests, [])

    # nothing else crosses a status line

    def test_a_trashed_page_is_not_restored_by_an_update(self):
        for publish in ([], ["--publish"]):
            err = self.refused("pages", "update", "60", "--title", "Back", *publish)
            self.assertEqual(err.code, "not_current")
        self.assertEqual(self.atlassian.puts(), [])

    def test_create_sends_the_status_asked_for(self):
        self.run_helper("pages", "create", "--space", "ENG", "--title", "New", "--text", "x", "--draft")
        self.run_helper("pages", "create", "--space", "ENG", "--title", "New", "--text", "x")
        statuses = [r["body"]["status"] for r in self.atlassian.requests if r["method"] == "POST"]
        self.assertEqual(statuses, ["draft", "current"])


if __name__ == "__main__":
    unittest.main()
