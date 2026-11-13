#!/usr/bin/env python3
"""The GitLab skill's helper (gitlab_api.py) takes a whole answer or none.

An answer that ends before its stated size (Content-Length), a chunked
answer cut short, a reset connection and an answer over the helper's limit
are refused, whether the answer is read into memory or saved to a file; a
saved download replaces its file only once it is whole, and a failed one
leaves the file as it was and nothing of its own behind.

Each answer is a real http.client.HTTPResponse, read from bytes in memory
or, through the helper's own urllib opener, from a socket on 127.0.0.1: no
other network. Standard library only. Run:

    python3 -I crates/templates/tests/gitlab_api_test.py
"""

import contextlib
import http.client
import importlib.util
import io
import json
import os
import socket
import stat
import struct
import tempfile
import threading
import unittest
import urllib.request
from pathlib import Path
from unittest import mock

HELPER = Path(__file__).resolve().parents[3] / "templates/skills/skills/productivity/gitlab-finite/scripts/gitlab_api.py"
BODY = b"hello, world"
OLD = b"the file as it was"
RESET = ConnectionResetError(54, "Connection reset by peer")


def load_helper():
    spec = importlib.util.spec_from_file_location("gitlab_api", HELPER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gitlab_api = load_helper()


class Wire(io.RawIOBase):
    """A socket's bytes, a few at a time: `data`, then its end, or `fault`
    raised where the end would be."""

    def __init__(self, data, fault, step=5):
        self.data = data
        self.fault = fault
        self.step = step
        self.at = 0

    def readable(self):
        return True

    def readinto(self, b):
        if self.at >= len(self.data):
            if self.fault is not None:
                raise self.fault
            return 0
        n = min(len(b), self.step, len(self.data) - self.at)
        b[:n] = self.data[self.at:self.at + n]
        self.at += n
        return n


class Socket:
    def __init__(self, wire):
        self.wire = wire

    def makefile(self, *_args, **_kwargs):
        return io.BufferedReader(self.wire)


def answer(body, headers, fault=None):
    """GitLab's answer: a 200 with `headers`, then `body`, then `fault`."""
    head = "HTTP/1.1 200 OK\r\n" + "".join(f"{k}: {v}\r\n" for k, v in headers) + "\r\n"
    response = http.client.HTTPResponse(Socket(Wire(head.encode("ascii") + body, fault)), method="GET")
    response.begin()
    return response


def sized(body, length=None, fault=None):
    """An answer stating its size (Content-Length: `length`, `len(body)` if
    none) and sending `body`."""
    return answer(body, [("Content-Length", len(body) if length is None else length)], fault)


def chunked(*chunks, end=True, cut=0, fault=None):
    """A chunked answer (no Content-Length): `chunks`, its last (empty)
    chunk if `end`, less its last `cut` bytes."""
    body = b"".join(b"%x\r\n%s\r\n" % (len(c), c) for c in chunks) + (b"0\r\n\r\n" if end else b"")
    return answer(body[:len(body) - cut], [("Transfer-Encoding", "chunked")], fault)


class Opener:
    """The helper's opener, answering every request with one answer."""

    def __init__(self, response):
        self.response = response
        self.urls = []

    def open(self, request, timeout=None):
        self.urls.append(request.full_url)
        return self.response


class Helper(unittest.TestCase):
    def setUp(self):
        self.folder = Path(self.use(tempfile.TemporaryDirectory()))
        self.use(mock.patch.dict(os.environ, {"GITLAB_OAUTH_ACCESS_TOKEN": "placeholder"}))

    def use(self, cm):
        # unittest's enterContext is Python 3.11's; the helper runs on older ones too
        value = cm.__enter__()
        self.addCleanup(cm.__exit__, None, None, None)
        return value

    def serve(self, response):
        opener = Opener(response)
        self.use(mock.patch.object(gitlab_api, "OPENER", opener))
        return opener

    def refused(self, code, *args, **kwargs):
        with self.assertRaises(gitlab_api.GitLabError) as caught:
            gitlab_api.call(*args, **kwargs)
        self.assertEqual(caught.exception.code, code, str(caught.exception))
        return caught.exception

    def run_command(self, *argv):
        args = gitlab_api.parser().parse_args(list(argv))
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            args.func(args)
        return json.loads(printed.getvalue())


class InMemory(Helper):
    """`call` reading an answer into memory: JSON, or bytes (`raw`)."""

    def test_a_whole_answer_is_read(self):
        self.serve(sized(BODY))
        self.assertEqual(gitlab_api.call("GET", "/api/v4/x", raw=True), BODY)

    def test_a_whole_json_answer_is_parsed(self):
        self.serve(sized(b'{"id": 1, "username": "ada"}'))
        self.assertEqual(gitlab_api.call("GET", "/api/v4/user"), {"id": 1, "username": "ada"})

    def test_an_empty_answer_is_an_empty_object(self):
        self.serve(sized(b""))
        self.assertEqual(gitlab_api.call("DELETE", "/api/v4/x"), {})

    def test_an_answer_cut_short_is_refused(self):
        self.serve(sized(BODY[:7], length=len(BODY)))
        self.refused("incomplete", "GET", "/api/v4/x", raw=True)

    def test_a_json_answer_cut_short_is_refused(self):
        whole = b'{"id": 1, "username": "ada"}'
        self.serve(sized(whole[:10], length=len(whole)))
        self.refused("incomplete", "GET", "/api/v4/user")

    def test_an_empty_answer_that_states_a_size_is_refused(self):
        self.serve(sized(b"", length=len(BODY)))
        self.refused("incomplete", "GET", "/api/v4/x", raw=True)

    def test_a_whole_chunked_answer_is_read(self):
        self.serve(chunked(b"hello, ", b"world"))
        self.assertEqual(gitlab_api.call("GET", "/api/v4/x", raw=True), BODY)

    def test_a_chunked_answer_cut_inside_a_chunk_is_refused(self):
        self.serve(chunked(b"hello, ", b"world", end=False, cut=5))
        self.refused("incomplete", "GET", "/api/v4/x", raw=True)

    def test_a_chunked_answer_without_its_last_chunk_is_refused(self):
        self.serve(chunked(b"hello, ", b"world", end=False))
        self.refused("incomplete", "GET", "/api/v4/x", raw=True)

    def test_a_reset_is_unreachable(self):
        self.serve(sized(BODY[:7], length=len(BODY), fault=RESET))
        self.refused("unreachable", "GET", "/api/v4/x", raw=True)

    def test_a_chunked_reset_is_unreachable(self):
        self.serve(chunked(b"hello, ", b"world", end=False, fault=RESET))
        self.refused("unreachable", "GET", "/api/v4/x", raw=True)

    def test_an_answer_stating_a_size_over_the_limit_is_refused(self):
        self.use(mock.patch.object(gitlab_api, "MAX_READ_BYTES", 8))
        self.serve(sized(BODY))
        self.refused("too_large", "GET", "/api/v4/x", raw=True)

    def test_a_chunked_answer_over_the_limit_is_refused(self):
        self.use(mock.patch.object(gitlab_api, "MAX_READ_BYTES", 8))
        self.serve(chunked(b"hello, ", b"world"))
        self.refused("too_large", "GET", "/api/v4/x", raw=True)

    def test_an_answer_at_the_limit_is_read(self):
        self.use(mock.patch.object(gitlab_api, "MAX_READ_BYTES", len(BODY)))
        self.serve(chunked(b"hello, ", b"world"))
        self.assertEqual(gitlab_api.call("GET", "/api/v4/x", raw=True), BODY)

    def test_a_file_cut_short_is_not_printed(self):
        self.serve(sized(b"print('hi')"[:6], length=11))
        with self.assertRaises(gitlab_api.GitLabError) as caught:
            self.run_command("files", "get", "group/project", "app.py")
        self.assertEqual(caught.exception.code, "incomplete")

    def test_a_whole_file_is_printed(self):
        opener = self.serve(sized(b"print('hi')\n"))
        printed = self.run_command("files", "get", "group/project", "app.py", "--ref", "main")
        self.assertEqual(printed["content"], "print('hi')\n")
        self.assertEqual(opener.urls, ["https://gitlab.com/api/v4/projects/group%2Fproject/repository/files/app%2Epy/raw?ref=main"])

    def test_a_job_log_cut_short_is_not_printed(self):
        self.serve(chunked(b"step 1\n", b"step 2\n", end=False))
        with self.assertRaises(gitlab_api.GitLabError) as caught:
            self.run_command("jobs", "log", "group/project", "987654")
        self.assertEqual(caught.exception.code, "incomplete")

    def test_a_whole_job_log_is_printed(self):
        self.serve(chunked(b"\x1b[32mstep 1\x1b[0m\n", b"step 2\n"))
        printed = self.run_command("jobs", "log", "group/project", "987654")
        self.assertEqual((printed["lines"], printed["log"]), (2, "step 1\nstep 2"))


class Saved(Helper):
    """`call(..., out=...)`: an answer written to a file."""

    def setUp(self):
        super().setUp()
        self.out = self.folder / "project.tar.gz"

    def existing(self, mode=0o640):
        self.out.write_bytes(OLD)
        self.out.chmod(mode)

    def assert_left_as_it_was(self):
        self.assertEqual(self.out.read_bytes(), OLD)
        self.assert_nothing_else_left()

    def assert_nothing_else_left(self):
        left = sorted(p.name for p in self.folder.iterdir())
        self.assertEqual(left, [self.out.name] if self.out.exists() else [])

    def save(self, response):
        self.serve(response)
        return gitlab_api.call("GET", "/api/v4/x", out=str(self.out))

    def refused_saving(self, code, response):
        self.serve(response)
        return self.refused(code, "GET", "/api/v4/x", out=str(self.out))

    def test_a_download_is_saved(self):
        self.assertEqual(self.save(sized(BODY)), {"saved": str(self.out), "bytes": len(BODY)})
        self.assertEqual(self.out.read_bytes(), BODY)
        self.assert_nothing_else_left()

    def test_a_new_file_is_made_as_open_makes_one(self):
        old_mask = os.umask(0o027)
        self.addCleanup(os.umask, old_mask)
        self.save(sized(BODY))
        self.assertEqual(stat.S_IMODE(self.out.stat().st_mode), 0o640)

    def test_a_download_replaces_an_existing_file_and_keeps_its_mode(self):
        self.existing(mode=0o755)
        self.assertEqual(self.save(sized(BODY))["bytes"], len(BODY))
        self.assertEqual(self.out.read_bytes(), BODY)
        self.assertEqual(stat.S_IMODE(self.out.stat().st_mode), 0o755)
        self.assert_nothing_else_left()

    def test_an_empty_download_is_saved(self):
        self.existing()
        self.assertEqual(self.save(sized(b""))["bytes"], 0)
        self.assertEqual(self.out.read_bytes(), b"")

    def test_a_whole_chunked_download_is_saved(self):
        self.existing()
        self.assertEqual(self.save(chunked(b"hello, ", b"world"))["bytes"], len(BODY))
        self.assertEqual(self.out.read_bytes(), BODY)
        self.assert_nothing_else_left()

    def test_a_download_cut_short_keeps_the_existing_file(self):
        self.existing()
        self.refused_saving("incomplete", sized(BODY[:7], length=len(BODY)))
        self.assert_left_as_it_was()

    def test_a_download_cut_short_leaves_no_file(self):
        self.refused_saving("incomplete", sized(BODY[:7], length=len(BODY)))
        self.assertFalse(self.out.exists())
        self.assert_nothing_else_left()

    def test_a_chunked_download_cut_short_keeps_the_existing_file(self):
        self.existing()
        self.refused_saving("incomplete", chunked(b"hello, ", b"world", end=False, cut=3))
        self.assert_left_as_it_was()

    def test_a_chunked_download_without_its_last_chunk_keeps_the_existing_file(self):
        self.existing()
        self.refused_saving("incomplete", chunked(b"hello, ", b"world", end=False))
        self.assert_left_as_it_was()

    def test_a_reset_keeps_the_existing_file(self):
        self.existing()
        self.refused_saving("unreachable", sized(BODY[:7], length=len(BODY), fault=RESET))
        self.assert_left_as_it_was()

    def test_a_reset_leaves_no_file(self):
        self.refused_saving("unreachable", chunked(b"hello, ", end=False, fault=RESET))
        self.assertFalse(self.out.exists())
        self.assert_nothing_else_left()

    def test_a_download_stating_a_size_over_the_limit_keeps_the_existing_file(self):
        self.use(mock.patch.object(gitlab_api, "MAX_SAVE_BYTES", 8))
        self.existing()
        self.refused_saving("too_large", sized(BODY))
        self.assert_left_as_it_was()

    def test_a_chunked_download_over_the_limit_keeps_the_existing_file(self):
        self.use(mock.patch.object(gitlab_api, "MAX_SAVE_BYTES", 8))
        self.existing()
        self.refused_saving("too_large", chunked(b"hello, ", b"world"))
        self.assert_left_as_it_was()

    def test_a_download_at_the_limit_is_saved(self):
        self.use(mock.patch.object(gitlab_api, "MAX_SAVE_BYTES", len(BODY)))
        self.assertEqual(self.save(chunked(b"hello, ", b"world"))["bytes"], len(BODY))
        self.assertEqual(self.out.read_bytes(), BODY)

    def test_an_interrupted_download_keeps_the_existing_file(self):
        self.existing()
        self.serve(sized(BODY[:7], length=len(BODY), fault=KeyboardInterrupt()))
        with self.assertRaises(KeyboardInterrupt):
            gitlab_api.call("GET", "/api/v4/x", out=str(self.out))
        self.assert_left_as_it_was()

    def test_a_file_that_cannot_be_replaced_is_left_as_it_was(self):
        self.existing()
        self.use(mock.patch.object(gitlab_api.os, "replace", side_effect=OSError(28, "No space left on device")))
        self.refused_saving("unwritable", sized(BODY))
        self.assert_left_as_it_was()

    def test_a_folder_that_does_not_exist_is_unwritable(self):
        self.out = self.folder / "missing" / "project.tar.gz"
        self.refused_saving("unwritable", sized(BODY))
        self.assertEqual(list(self.folder.iterdir()), [])

    def test_a_folder_is_not_replaced(self):
        self.out.mkdir()
        (self.out / "kept").write_bytes(OLD)
        self.refused_saving("unwritable", sized(BODY))
        self.assertEqual((self.out / "kept").read_bytes(), OLD)
        self.assertEqual([p.name for p in self.folder.iterdir()], [self.out.name])

    def test_an_archive_is_saved(self):
        opener = self.serve(sized(BODY))
        printed = self.run_command("files", "archive", "group/project", "--ref", "main", "--out", str(self.out))
        self.assertEqual(printed, {"saved": str(self.out), "bytes": len(BODY)})
        self.assertEqual(self.out.read_bytes(), BODY)
        self.assertEqual(opener.urls, ["https://gitlab.com/api/v4/projects/group%2Fproject/repository/archive.tar.gz?sha=main"])

    def test_an_archive_cut_short_keeps_the_existing_file(self):
        self.existing()
        self.serve(sized(BODY[:7], length=len(BODY)))
        with self.assertRaises(gitlab_api.GitLabError) as caught:
            self.run_command("files", "archive", "group/project", "--out", str(self.out))
        self.assertEqual(caught.exception.code, "incomplete")
        self.assert_left_as_it_was()

    def test_a_saved_job_log_cut_short_keeps_the_existing_file(self):
        self.existing()
        self.serve(chunked(b"step 1\n", b"step 2\n", end=False))
        with self.assertRaises(gitlab_api.GitLabError) as caught:
            self.run_command("jobs", "log", "group/project", "987654", "--out", str(self.out))
        self.assertEqual(caught.exception.code, "incomplete")
        self.assert_left_as_it_was()

    def test_a_saved_file_cut_short_keeps_the_existing_file(self):
        self.existing()
        self.serve(sized(BODY[:7], length=len(BODY), fault=RESET))
        with self.assertRaises(gitlab_api.GitLabError) as caught:
            self.run_command("files", "get", "group/project", "app.py", "--out", str(self.out))
        self.assertEqual(caught.exception.code, "unreachable")
        self.assert_left_as_it_was()


class Server(threading.Thread):
    """One answer over TCP on 127.0.0.1: `reply` once the request has
    arrived, then the connection closed, or reset (`reset`)."""

    def __init__(self, reply, reset=False):
        super().__init__(daemon=True)
        self.listener = socket.create_server(("127.0.0.1", 0))
        self.reply = reply
        self.reset = reset
        self.url = "http://127.0.0.1:%d" % self.listener.getsockname()[1]

    def run(self):
        with self.listener:
            conn, _ = self.listener.accept()
            with conn:
                request = b""
                while b"\r\n\r\n" not in request:
                    piece = conn.recv(65536)
                    if not piece:
                        return
                    request += piece
                conn.sendall(self.reply)
                if self.reset:
                    # closed with no linger: the peer gets a reset (RST), not an end
                    conn.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))


class OverTcp(Helper):
    """The helper's own opener (urllib, no redirects) reading from a real
    socket that ends, or is reset, part way through an answer."""

    def setUp(self):
        super().setUp()
        self.out = self.folder / "job.log"
        # the helper's opener, without any proxy this machine's environment names
        self.use(mock.patch.object(gitlab_api, "OPENER", urllib.request.build_opener(urllib.request.ProxyHandler({}), gitlab_api.NoRedirect)))

    def listen(self, reply, reset=False):
        server = Server(reply, reset)
        self.use(mock.patch.object(gitlab_api, "GITLAB", server.url))
        server.start()
        self.addCleanup(server.join, 5)

    def stating(self, length, body):
        self.listen(b"HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s" % (length, body))

    def test_a_whole_answer_is_read(self):
        self.stating(len(BODY), BODY)
        self.assertEqual(gitlab_api.call("GET", "/api/v4/x", raw=True), BODY)

    def test_an_answer_cut_short_is_refused(self):
        self.stating(len(BODY), BODY[:7])
        self.refused("incomplete", "GET", "/api/v4/x", raw=True)

    def test_a_whole_download_replaces_the_existing_file(self):
        self.out.write_bytes(OLD)
        self.stating(len(BODY), BODY)
        self.assertEqual(gitlab_api.call("GET", "/api/v4/x", out=str(self.out)), {"saved": str(self.out), "bytes": len(BODY)})
        self.assertEqual(self.out.read_bytes(), BODY)
        self.assertEqual([p.name for p in self.folder.iterdir()], [self.out.name])

    def test_a_download_cut_short_keeps_the_existing_file(self):
        self.out.write_bytes(OLD)
        self.stating(len(BODY), BODY[:7])
        self.refused("incomplete", "GET", "/api/v4/x", out=str(self.out))
        self.assertEqual(self.out.read_bytes(), OLD)
        self.assertEqual([p.name for p in self.folder.iterdir()], [self.out.name])

    def test_a_reset_keeps_the_existing_file(self):
        self.out.write_bytes(OLD)
        self.listen(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\nhello, \r\n", reset=True)
        self.refused("unreachable", "GET", "/api/v4/x", out=str(self.out))
        self.assertEqual(self.out.read_bytes(), OLD)
        self.assertEqual([p.name for p in self.folder.iterdir()], [self.out.name])


if __name__ == "__main__":
    unittest.main()
