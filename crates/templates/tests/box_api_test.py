"""The Box skill's helper (templates/skills/skills/productivity/box-finite/
scripts/box_api.py): a download is saved only once all of its bytes are
here, and one that breaks off leaves the destination as it was.

Each answer is a real http.client.HTTPResponse over an in-memory socket, so
a short body reads as CPython reads one: `read(n)` returns short or empty at
a premature end and raises nothing. No network.

    python3 -I crates/templates/tests/box_api_test.py
"""

import builtins
import contextlib
import errno
import http.client
import importlib.util
import io
import json
import os
import pathlib
import shutil
import sys
import tempfile
import unittest
from unittest import mock

# the helper ships in the skills tree: leave no __pycache__ there
sys.dont_write_bytecode = True
HELPER = pathlib.Path(__file__).resolve().parents[3] / "templates/skills/skills/productivity/box-finite/scripts/box_api.py"
spec = importlib.util.spec_from_file_location("box_api", HELPER)
box = importlib.util.module_from_spec(spec)
spec.loader.exec_module(box)

BODY = b"0123456789"
ORIGINAL = b"the file that was already there"
DOWNLOAD_URL = "https://dl.boxcloud.com/d/1/abc/download"


class Connection(io.RawIOBase):
    """A connection's bytes: `data`, then `then` raised (or its end)."""

    def __init__(self, data, then=None):
        self.data = data
        self.at = 0
        self.then = then

    def readable(self):
        return True

    def readinto(self, b):
        if self.at >= len(self.data):
            if self.then is not None:
                raise self.then
            return 0
        n = min(len(b), len(self.data) - self.at)
        b[:n] = self.data[self.at:self.at + n]
        self.at += n
        return n


class Socket:
    def __init__(self, connection):
        self.connection = connection

    def makefile(self, mode):
        return io.BufferedReader(self.connection)


def answer(body, headers, then=None):
    """A 200 with `headers` and `body`, its connection then ending (or
    raising `then`)."""
    head = "HTTP/1.1 200 OK\r\n" + "".join(f"{k}: {v}\r\n" for k, v in headers.items()) + "\r\n"
    response = http.client.HTTPResponse(Socket(Connection(head.encode() + body, then)), method="GET")
    response.begin()
    return response


def whole():
    return answer(BODY, {"Content-Length": len(BODY)})


def short():
    """Ten bytes promised, three sent, then the connection's end."""
    return answer(BODY[:3], {"Content-Length": len(BODY)})


def reset():
    """Ten bytes promised, six sent, then a reset connection."""
    return answer(BODY[:6], {"Content-Length": len(BODY)}, ConnectionResetError(errno.ECONNRESET, "Connection reset by peer"))


def short_chunked():
    """A chunked body cut off inside its only chunk."""
    return answer(b"a\r\n01234", {"Transfer-Encoding": "chunked"})


def interrupted():
    return answer(BODY[:6], {"Content-Length": len(BODY)}, KeyboardInterrupt())


def unframed(body):
    """A body with no length, ending when its connection closes."""
    return answer(body, {"Connection": "close"})


class Downloads:
    """box_api.DOWNLOADS: every request answered by `response`, after
    `meanwhile` runs (a file appearing at the destination, say)."""

    def __init__(self, response, meanwhile=None):
        self.response = response
        self.meanwhile = meanwhile
        self.opened = 0

    def open(self, request, timeout=None):
        assert request.full_url == DOWNLOAD_URL, request.full_url
        assert not request.has_header("Authorization"), "a download's bytes are fetched with no token"
        self.opened += 1
        if self.meanwhile:
            self.meanwhile()
        return self.response


class FullDisk:
    """A file whose second write fails as a full disk's does."""

    def __init__(self, f):
        self.f = f
        self.writes = 0

    def write(self, b):
        self.writes += 1
        if self.writes > 1:
            raise OSError(errno.ENOSPC, "No space left on device")
        return self.f.write(b)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.f.close()

    def __getattr__(self, name):
        return getattr(self.f, name)


def full_disk_open(path, mode="r", *args, **kwargs):
    f = builtins.open(path, mode, *args, **kwargs)
    return FullDisk(f) if "r" not in mode else f


class Download(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="box-api-test-")
        self.addCleanup(shutil.rmtree, self.root)
        self.fresh()
        # Box's size for the file, asked only of a body with no length
        self.stated = None
        for name, value in (("CHUNK_BYTES", 4), ("download_url", self.download_url), ("call", self.call)):
            patcher = mock.patch.object(box, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def fresh(self, existing=None):
        """A new, empty folder; `existing` at the destination, if named."""
        self.dir = tempfile.mkdtemp(dir=self.root)
        self.target = os.path.join(self.dir, "plan.pdf")
        if existing is not None:
            pathlib.Path(self.target).write_bytes(existing)

    def download_url(self, file):
        assert file == "456"
        return DOWNLOAD_URL

    def call(self, method, path, body=None, query=None):
        assert (method, path, query) == ("GET", "/2.0/files/456", {"fields": "size"}), (method, path, query)
        assert self.stated is not None, "Box's size is asked only of a body with no length"
        return {"size": self.stated}

    def download(self, response, overwrite=False, meanwhile=None):
        """`files download 456 --out <target>`, Box's bytes answered by
        `response`: what it printed."""
        argv = ["files", "download", "456", "--out", self.target] + (["--overwrite"] if overwrite else [])
        args = box.parser().parse_args(argv)
        self.printed = io.StringIO()
        self.downloads = Downloads(response, meanwhile)
        with mock.patch.object(box, "DOWNLOADS", self.downloads), contextlib.redirect_stdout(self.printed):
            args.func(args)
        return json.loads(self.printed.getvalue())

    def listing(self):
        return sorted(os.listdir(self.dir))

    def content(self):
        return pathlib.Path(self.target).read_bytes()

    def assert_saved(self, printed):
        self.assertEqual(printed, {"status": "downloaded", "id": "456", "path": self.target, "bytes": len(BODY)})
        self.assertEqual(self.content(), BODY)
        self.assertEqual(self.listing(), ["plan.pdf"], "nothing staged is left behind")

    def assert_nothing_changes(self, respond, raised=box.BoxError, code="download_failed", patch_open=False):
        """A download answered by `respond()` fails and changes nothing: no
        file where there was none, and an existing one, though it may be
        overwritten, as it was, with nothing staged left beside it."""
        for existing in (None, ORIGINAL):
            with self.subTest(existing=existing):
                self.fresh(existing)
                opener = mock.patch.object(box, "open", full_disk_open, create=True) if patch_open else contextlib.nullcontext()
                with opener, self.assertRaises(raised) as caught:
                    self.download(respond(), overwrite=existing is not None)
                self.assertNotIn("downloaded", self.printed.getvalue())
                if existing is None:
                    self.assertEqual(self.listing(), [])
                else:
                    self.assertEqual(self.listing(), ["plan.pdf"], "the existing file is kept")
                    self.assertEqual(self.content(), ORIGINAL)
                if raised is box.BoxError:
                    self.assertEqual(caught.exception.code, code, str(caught.exception))

    def test_a_whole_download_is_saved(self):
        self.assert_saved(self.download(whole()))

    def test_a_whole_download_replaces_a_file_with_overwrite(self):
        self.fresh(ORIGINAL)
        self.assert_saved(self.download(whole(), overwrite=True))

    def test_an_existing_file_is_refused_without_overwrite_before_any_download(self):
        self.fresh(ORIGINAL)
        with self.assertRaises(box.BoxError) as caught:
            self.download(whole())
        self.assertEqual(caught.exception.code, "exists")
        self.assertEqual(self.downloads.opened, 0)
        self.assertEqual(self.content(), ORIGINAL)
        self.assertEqual(self.listing(), ["plan.pdf"])

    def test_a_body_short_of_its_content_length_saves_nothing(self):
        self.assert_nothing_changes(short)

    def test_a_reset_connection_saves_nothing(self):
        self.assert_nothing_changes(reset)

    def test_a_short_chunked_body_saves_nothing(self):
        self.assert_nothing_changes(short_chunked)

    def test_a_failed_write_saves_nothing(self):
        self.assert_nothing_changes(whole, code="bad_out", patch_open=True)

    def test_an_interrupted_download_saves_nothing(self):
        self.assert_nothing_changes(interrupted, raised=KeyboardInterrupt)

    def test_a_file_that_appears_during_the_download_is_kept_without_overwrite(self):
        appear = lambda: pathlib.Path(self.target).write_bytes(ORIGINAL)
        with self.assertRaises(box.BoxError) as caught:
            self.download(whole(), meanwhile=appear)
        self.assertEqual(caught.exception.code, "exists")
        self.assertEqual(self.content(), ORIGINAL)
        self.assertEqual(self.listing(), ["plan.pdf"], "nothing staged is left behind")

    def test_a_file_that_appears_during_the_download_is_replaced_with_overwrite(self):
        appear = lambda: pathlib.Path(self.target).write_bytes(ORIGINAL)
        self.assert_saved(self.download(whole(), overwrite=True, meanwhile=appear))

    def test_a_body_with_no_length_is_saved_when_it_is_boxs_size(self):
        self.stated = len(BODY)
        self.assert_saved(self.download(unframed(BODY)))

    def test_a_body_with_no_length_short_of_boxs_size_saves_nothing(self):
        self.stated = len(BODY)
        self.assert_nothing_changes(lambda: unframed(BODY[:3]))


class ApiAnswers(unittest.TestCase):
    """The API's answers (`call`, an upload's) are read whole with `read()`,
    which, unlike `read(n)`, raises on a body short of its Content-Length."""

    def test_an_answer_short_of_its_content_length_is_never_read_as_whole(self):
        body = json.dumps({"id": "1", "name": "Ada"}).encode()
        response = answer(body[:5], {"Content-Length": len(body), "Content-Type": "application/json"})
        with mock.patch.object(box, "send", lambda *a, **k: response), mock.patch.dict(os.environ, {"BOX_OAUTH_ACCESS_TOKEN": "placeholder"}):
            with self.assertRaises(http.client.IncompleteRead):
                box.call("GET", "/2.0/users/me")


if __name__ == "__main__":
    unittest.main()
