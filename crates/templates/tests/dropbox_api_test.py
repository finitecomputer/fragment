"""The Dropbox skill's helper: a body cut short is never taken for the whole.

Each answer is a real http.client.HTTPResponse parsed from bytes, so the
helper reads it as it would read Dropbox's. HTTPResponse.read(n) answers b""
at an early end of the connection instead of raising, which is what these
cases pin down. No network: the helper's opener is replaced.

Run: python3 -I crates/templates/tests/dropbox_api_test.py
"""

import contextlib
import http.client
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest import mock

HELPER = Path(__file__).resolve().parents[3] / "templates/skills/skills/productivity/dropbox-finite/scripts/dropbox_api.py"
FILE = b"0123456789"
COMPLETE_BEFORE = b"the complete earlier copy"


def load_helper():
    spec = importlib.util.spec_from_file_location("dropbox_api_under_test", HELPER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Wire:
    """A socket stand-in: its makefile is the raw answer, which may end early
    or fail after `fail_after` bytes."""

    def __init__(self, raw, fail_after=None):
        self.raw = raw
        self.fail_after = fail_after

    def makefile(self, _mode):
        if self.fail_after is None:
            return io.BytesIO(self.raw)
        return io.BufferedReader(FailingStream(self.raw, self.fail_after))


class FailingStream(io.RawIOBase):
    """Bytes until `fail_after` of the body is gone, then a timeout."""

    def __init__(self, raw, fail_after):
        self.data = raw
        head = raw.index(b"\r\n\r\n") + 4
        self.limit = head + fail_after
        self.at = 0

    def readable(self):
        return True

    def readinto(self, buffer):
        if self.at >= self.limit:
            raise TimeoutError("timed out")
        n = min(len(buffer), self.limit - self.at)
        buffer[:n] = self.data[self.at:self.at + n]
        self.at += n
        return n


def answer(body, length=None, result=None, chunked=False, fail_after=None, status=b"200 OK"):
    """An answer (200 unless `status`) carrying `body`, with the headers given."""
    lines = [b"HTTP/1.1 " + status, b"Connection: close"]
    if length is not None:
        lines.append(b"Content-Length: " + str(length).encode())
    if result is not None:
        lines.append(b"Dropbox-API-Result: " + json.dumps(result).encode())
    if chunked:
        lines.append(b"Transfer-Encoding: chunked")
    response = http.client.HTTPResponse(Wire(b"\r\n".join(lines) + b"\r\n\r\n" + body, fail_after), method="POST")
    response.begin()
    return response


def chunks(*parts, end=True):
    framed = b"".join(b"%x\r\n%s\r\n" % (len(p), p) for p in parts)
    return framed + (b"0\r\n\r\n" if end else b"")


def metadata(size):
    return {".tag": "file", "name": "plan.pdf", "path_display": "/Work/plan.pdf", "id": "id:a4ayc_80_OEAAAAAAAAAXw", "size": size, "rev": "a1c10ce0dd78"}


class Opener:
    """The helper's opener, answering every request with `response`."""

    def __init__(self, response):
        self.response = response
        self.requests = []

    def open(self, request, timeout):
        self.requests.append(request)
        if self.response.status >= 400:
            # what urllib raises for a refusal: the answer, its body unread
            raise urllib.error.HTTPError(request.full_url, self.response.status, self.response.reason, self.response.headers, self.response)
        return self.response


class HelperCase(unittest.TestCase):
    def setUp(self):
        self.helper = load_helper()
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.dest = os.path.join(self.dir.name, "plan.pdf")
        env = mock.patch.dict(os.environ, {"DROPBOX_OAUTH_ACCESS_TOKEN": "placeholder"})
        env.start()
        self.addCleanup(env.stop)

    def run_helper(self, response, *argv):
        """The helper's exit status (0 where it returned), stdout and the
        JSON it printed on stderr, answering with `response`."""
        self.helper.OPENER = Opener(response)
        stdout, stderr = io.StringIO(), io.StringIO()
        status = 0
        with mock.patch.object(sys, "argv", ["dropbox_api.py", *argv]), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                self.helper.main()
            except SystemExit as exc:
                status = exc.code
        error = json.loads(stderr.getvalue()) if stderr.getvalue() else None
        return status, stdout.getvalue(), error

    def download(self, response, *flags):
        return self.run_helper(response, "download", "/Work/plan.pdf", "--to", self.dest, *flags)

    def keep_earlier_copy(self):
        Path(self.dest).write_bytes(COMPLETE_BEFORE)

    def assert_refused_cut_short(self, outcome):
        status, stdout, error = outcome
        self.assertEqual(status, 1)
        self.assertEqual(stdout, "")
        self.assertEqual(error["error"], "incomplete_answer")
        self.assertTrue(error["hint"])

    def assert_no_part_left(self):
        """Nothing in the folder but the destination (where there is one)."""
        self.assertEqual(set(os.listdir(self.dir.name)) - {"plan.pdf"}, set())


class Download(HelperCase):
    def test_a_complete_download_is_saved(self):
        status, stdout, error = self.download(answer(FILE, length=10, result=metadata(10)))
        self.assertEqual((status, error), (0, None))
        self.assertEqual(json.loads(stdout)["bytes"], 10)
        self.assertEqual(Path(self.dest).read_bytes(), FILE)
        self.assert_no_part_left()

    def test_an_empty_file_is_saved_empty(self):
        status, stdout, error = self.download(answer(b"", length=0, result=metadata(0)))
        self.assertEqual((status, error), (0, None))
        self.assertEqual(json.loads(stdout)["bytes"], 0)
        self.assertEqual(Path(self.dest).read_bytes(), b"")
        self.assert_no_part_left()

    def test_a_complete_chunked_download_is_saved(self):
        status, _stdout, error = self.download(answer(chunks(b"01234", b"56789"), result=metadata(10), chunked=True))
        self.assertEqual((status, error), (0, None))
        self.assertEqual(Path(self.dest).read_bytes(), FILE)

    def test_overwrite_replaces_with_a_complete_download(self):
        self.keep_earlier_copy()
        status, _stdout, error = self.download(answer(FILE, length=10, result=metadata(10)), "--overwrite")
        self.assertEqual((status, error), (0, None))
        self.assertEqual(Path(self.dest).read_bytes(), FILE)
        self.assert_no_part_left()

    def test_an_early_end_saves_nothing(self):
        self.assert_refused_cut_short(self.download(answer(b"abc", length=10, result=metadata(10))))
        self.assertFalse(os.path.exists(self.dest))
        self.assert_no_part_left()

    def test_an_early_end_with_overwrite_keeps_the_earlier_file(self):
        self.keep_earlier_copy()
        self.assert_refused_cut_short(self.download(answer(b"abc", length=10, result=metadata(10)), "--overwrite"))
        self.assertEqual(Path(self.dest).read_bytes(), COMPLETE_BEFORE)
        self.assert_no_part_left()

    def test_an_early_end_against_content_length_alone_is_refused(self):
        self.keep_earlier_copy()
        self.assert_refused_cut_short(self.download(answer(b"abc", length=10), "--overwrite"))
        self.assertEqual(Path(self.dest).read_bytes(), COMPLETE_BEFORE)

    def test_an_early_end_against_dropbox_size_alone_is_refused(self):
        # no Content-Length: the body runs until the connection closes
        self.keep_earlier_copy()
        self.assert_refused_cut_short(self.download(answer(b"abc", result=metadata(10)), "--overwrite"))
        self.assertEqual(Path(self.dest).read_bytes(), COMPLETE_BEFORE)
        self.assert_no_part_left()

    def test_more_than_dropbox_size_is_refused(self):
        self.assert_refused_cut_short(self.download(answer(chunks(FILE, b"!"), result=metadata(10), chunked=True)))
        self.assertFalse(os.path.exists(self.dest))
        self.assert_no_part_left()

    def test_chunks_that_stop_early_are_refused(self):
        self.keep_earlier_copy()
        self.assert_refused_cut_short(self.download(answer(chunks(b"abc", end=False), result=metadata(10), chunked=True), "--overwrite"))
        self.assertEqual(Path(self.dest).read_bytes(), COMPLETE_BEFORE)
        self.assert_no_part_left()

    def test_a_timeout_mid_body_keeps_the_earlier_file(self):
        self.keep_earlier_copy()
        self.assert_refused_cut_short(self.download(answer(FILE, length=10, result=metadata(10), fail_after=3), "--overwrite"))
        self.assertEqual(Path(self.dest).read_bytes(), COMPLETE_BEFORE)
        self.assert_no_part_left()

    def test_an_existing_file_needs_overwrite(self):
        self.keep_earlier_copy()
        status, stdout, error = self.download(answer(FILE, length=10, result=metadata(10)))
        self.assertEqual((status, stdout, error["error"]), (1, "", "exists"))
        self.assertEqual(self.helper.OPENER.requests, [])
        self.assertEqual(Path(self.dest).read_bytes(), COMPLETE_BEFORE)


class Answers(HelperCase):
    """Calls whose answer is read into memory: an RPC call's, and an upload's."""

    BODY = json.dumps(metadata(10)).encode()

    def upload(self, response):
        local = os.path.join(self.dir.name, "notes.txt")
        Path(local).write_bytes(FILE)
        return self.run_helper(response, "upload", local, "--to", "/Work/notes.txt")

    def test_a_complete_rpc_answer_is_printed(self):
        status, stdout, error = self.run_helper(answer(self.BODY, length=len(self.BODY)), "get", "/Work/plan.pdf")
        self.assertEqual((status, error), (0, None))
        self.assertEqual(json.loads(stdout)["size"], 10)

    def test_an_rpc_answer_cut_short_is_refused(self):
        self.assert_refused_cut_short(self.run_helper(answer(self.BODY[:12], length=len(self.BODY)), "get", "/Work/plan.pdf"))

    def test_rpc_chunks_that_stop_early_are_refused(self):
        self.assert_refused_cut_short(self.run_helper(answer(chunks(self.BODY[:12], end=False), chunked=True), "get", "/Work/plan.pdf"))

    def test_a_complete_upload_answer_is_printed(self):
        status, stdout, error = self.upload(answer(self.BODY, length=len(self.BODY)))
        self.assertEqual((status, error), (0, None))
        self.assertEqual(json.loads(stdout)["name"], "plan.pdf")

    def test_an_upload_answer_cut_short_is_refused(self):
        self.assert_refused_cut_short(self.upload(answer(self.BODY[:12], length=len(self.BODY))))

    NOT_FOUND = json.dumps({"error_summary": "path/not_found/..", "error": {".tag": "path", "path": {".tag": "not_found"}}}).encode()

    def test_a_refusal_names_dropbox_error(self):
        status, stdout, error = self.run_helper(answer(self.NOT_FOUND, length=len(self.NOT_FOUND), status=b"409 Conflict"), "get", "/Work/plan.pdf")
        self.assertEqual((status, stdout), (1, ""))
        self.assertEqual((error["error"], error["status"]), ("path/not_found", 409))

    def test_a_refusal_cut_short_keeps_its_status(self):
        outcome = self.run_helper(answer(self.NOT_FOUND[:12], length=len(self.NOT_FOUND), status=b"409 Conflict"), "get", "/Work/plan.pdf")
        self.assert_refused_cut_short(outcome)
        self.assertEqual(outcome[2]["status"], 409)


if __name__ == "__main__":
    unittest.main()
