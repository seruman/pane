#!/usr/bin/env python3
"""Run with python3 -I -m unittest discover -s scripts -p 'test_pane_inspect.py'."""

import base64
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name("pane-inspect.py")
spec = importlib.util.spec_from_file_location("pane_inspect", SCRIPT)
inspect = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inspect)

# A one-pixel PNG; validation does not depend on any image library.
PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a"
    "1ioAAAAASUVORK5CYII="
)


def screenshot():
    return {
        "ok": True,
        "result": {
            "image": {
                "format": "png", "encoding": "base64",
                "data": base64.b64encode(PNG).decode("ascii"),
            },
            "capture": {"width": 1, "height": 1, "extra": "preserved"},
            "other": [1, 2],
        },
        "request_id": "example",
    }


class ExchangeTests(unittest.TestCase):
    def exchange(self, chunks, limit=inspect.MAX_RESPONSE):
        client = mock.MagicMock()
        client.__enter__.return_value = client
        pending = list(chunks)

        def recv(size):
            if not pending:
                return b""
            chunk = pending.pop(0)
            if len(chunk) > size:
                pending.insert(0, chunk[size:])
                chunk = chunk[:size]
            return chunk

        client.recv.side_effect = recv
        with mock.patch.object(inspect.socket, "socket", return_value=client), \
                mock.patch.object(inspect, "MAX_RESPONSE", limit):
            result = inspect.exchange("/private/inspect", b'{"op":"ping"}\n')
        return result, client

    def test_fragmented_response_and_only_first_line(self):
        result, client = self.exchange([
            b'{"ok":', b'true,"result":{"text":"', b'\xc3', b'\xa9"}}\nignored\n',
        ])
        self.assertEqual(result, {"ok": True, "result": {"text": "\u00e9"}})
        client.connect.assert_called_once_with("/private/inspect/control.sock")
        client.sendall.assert_called_once_with(b'{"op":"ping"}\n')
        client.__exit__.assert_called_once()
        self.assertTrue(all(0 < call.args[0] <= inspect.TIMEOUT
                            for call in client.settimeout.call_args_list))

    def test_response_limit_includes_newline(self):
        payload = b'{"ok":true,"result":null}\n'
        result, _ = self.exchange([payload], limit=len(payload))
        self.assertTrue(result["ok"])
        with self.assertRaisesRegex(ValueError, "exceeds"):
            self.exchange([payload], limit=len(payload) - 1)
        with self.assertRaisesRegex(ValueError, "exceeds"):
            self.exchange([b"x" * 33], limit=32)

    def test_trailing_bytes_do_not_count_against_first_response(self):
        result, _ = self.exchange([b"{}\n" + b"x" * 100], limit=3)
        self.assertEqual(result, {})

    def test_disconnect_before_newline(self):
        for chunks in ([], [b'{"ok":true}']):
            with self.subTest(chunks=chunks), self.assertRaisesRegex(ValueError, "newline"):
                self.exchange(chunks)

    def test_invalid_response_json_utf8_and_constants(self):
        for payload in (b"nope\n", b"\xff\n", b"NaN\n", b"Infinity\n", b"-Infinity\n"):
            with self.subTest(payload=payload), self.assertRaises(ValueError):
                self.exchange([payload])

    def test_whole_request_deadline_not_reset_by_chunks(self):
        # Start, connect, send, first recv, second recv: deadline expires even
        # though the first chunk arrived successfully.
        with mock.patch.object(inspect, "TIMEOUT", 8), \
                mock.patch.object(inspect.time, "monotonic", side_effect=[0, 1, 2, 7, 9]):
            with self.assertRaisesRegex(TimeoutError, "deadline"):
                self.exchange([b'{"ok":', b'true}\n'])

    def test_deadline_checked_after_complete_response(self):
        with mock.patch.object(inspect, "TIMEOUT", 8), \
                mock.patch.object(inspect.time, "monotonic", side_effect=[0, 1, 2, 3, 9]):
            with self.assertRaisesRegex(TimeoutError, "deadline"):
                self.exchange([b"{}\n"])


class CliTests(unittest.TestCase):
    def run_cli(self, response, *options, request='{"op":"screenshot"}'):
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(inspect, "exchange", return_value=response) as exchange, \
                contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status = inspect.main(["/private/inspect", request, *options])
        return status, stdout.getvalue(), stderr.getvalue(), exchange

    def test_no_output_preserves_response_and_image_data(self):
        response = screenshot()
        before = copy.deepcopy(response)
        status, stdout, stderr, exchange = self.run_cli(response)
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(stdout), before)
        self.assertEqual(response, before)
        self.assertEqual(stderr, "")
        exchange.assert_called_once_with("/private/inspect", b'{"op":"screenshot"}\n')

    def test_no_output_does_not_require_screenshot_schema(self):
        for result in (None, [], {"image": {"data": "not base64"}}, "pong"):
            with self.subTest(result=result):
                response = {"ok": True, "result": result}
                status, stdout, stderr, _ = self.run_cli(response)
                self.assertEqual(status, 0)
                self.assertEqual(json.loads(stdout), response)
                self.assertEqual(stderr, "")

    def test_save_png_and_print_metadata_without_mutating_response(self):
        response = screenshot()
        before = copy.deepcopy(response)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "capture.png"
            status, stdout, stderr, _ = self.run_cli(response, "--output", str(output))
            self.assertEqual(status, 0, stderr)
            self.assertEqual(output.read_bytes(), PNG)
            expected = copy.deepcopy(before)
            del expected["result"]["image"]["data"]
            expected["output"] = str(output)
            self.assertEqual(json.loads(stdout), expected)
            self.assertNotIn(before["result"]["image"]["data"], stdout)
            self.assertEqual(response, before)

    def test_existing_file_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "user.png"
            output.write_bytes(b"user content")
            status, stdout, stderr, _ = self.run_cli(screenshot(), "--output", str(output))
            self.assertEqual(status, 1)
            self.assertEqual(stdout, "")
            self.assertIn("pane-inspect:", stderr)
            self.assertEqual(output.read_bytes(), b"user content")

    def test_existing_symlink_is_never_followed(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "missing-target"
            output = Path(directory) / "link.png"
            output.symlink_to(target)
            status, _, _, _ = self.run_cli(screenshot(), "--output", str(output))
            self.assertEqual(status, 1)
            self.assertTrue(output.is_symlink())
            self.assertFalse(target.exists())

    def test_missing_parent_directory_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "missing" / "capture.png"
            status, stdout, stderr, _ = self.run_cli(screenshot(), "--output", str(output))
            self.assertEqual(status, 1)
            self.assertEqual(stdout, "")
            self.assertIn("pane-inspect:", stderr)
            self.assertFalse(output.parent.exists())

    def test_invalid_images_rejected_before_opening_any_file(self):
        invalid_images = [
            None, [], "png", {},
            {"format": "jpeg", "encoding": "base64", "data": ""},
            {"format": "png", "encoding": "hex", "data": ""},
        ]
        for data in (None, 42, [], "", "!!!", "abc", "\u00e9", "AAAA\n", "eA==junk",
                     base64.b64encode(b"not png").decode(),
                     base64.b64encode(b"\x89PNG\r\n\x1a").decode()):
            invalid_images.append({"format": "png", "encoding": "base64", "data": data})
        for image in invalid_images:
            with self.subTest(image=image), mock.patch.object(Path, "open") as open_file:
                status, stdout, stderr, _ = self.run_cli(
                    {"ok": True, "result": {"image": image}}, "--output", "unused.png"
                )
                self.assertEqual(status, 1)
                self.assertEqual(stdout, "")
                self.assertIn("pane-inspect:", stderr)
                open_file.assert_not_called()

    def test_unsupported_output_responses_rejected_before_open(self):
        for result in (None, [], "pong", {}, {"capture": {}}):
            with self.subTest(result=result), mock.patch.object(Path, "open") as open_file:
                status, stdout, stderr, _ = self.run_cli(
                    {"ok": True, "result": result}, "--output", "unused.png"
                )
                self.assertEqual(status, 1)
                self.assertEqual(stdout, "")
                self.assertIn("--output", stderr)
                open_file.assert_not_called()

    def test_decoded_size_limit_and_predecode_bound(self):
        # Same encoded length can represent different decoded sizes (padding),
        # so test both the encoded precheck and the decoded-size check.
        for size in (12, 30):
            response = screenshot()
            response["result"]["image"]["data"] = base64.b64encode(
                b"\x89PNG\r\n\x1a\n" + b"x" * (size - 8)
            ).decode()
            with self.subTest(size=size), mock.patch.object(inspect, "MAX_RESPONSE", 11), \
                    mock.patch.object(Path, "open") as open_file:
                status, _, stderr, _ = self.run_cli(response, "--output", "unused.png")
                self.assertEqual(status, 1)
                self.assertIn("exceeds", stderr)
                open_file.assert_not_called()

    def test_decoded_size_at_limit_is_allowed(self):
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(inspect, "MAX_RESPONSE", len(PNG)):
            output = Path(directory) / "capture.png"
            status, _, stderr, _ = self.run_cli(screenshot(), "--output", str(output))
            self.assertEqual(status, 0, stderr)
            self.assertEqual(output.read_bytes(), PNG)

    def test_server_error_is_printed_and_never_writes_output(self):
        response = {"ok": False, "error": "capture unavailable"}
        for options in ((), ("--output", "unused.png")):
            with self.subTest(options=options), mock.patch.object(Path, "open") as open_file:
                status, stdout, stderr, _ = self.run_cli(response, *options)
                self.assertEqual(status, 1)
                self.assertEqual(json.loads(stdout), response)
                self.assertEqual(stderr, "")
                open_file.assert_not_called()

    def test_invalid_envelopes(self):
        for response in (None, [], {}, {"ok": 1}, {"ok": True},
                         {"ok": False}, {"ok": False, "error": {}}):
            with self.subTest(response=response), mock.patch.object(Path, "open") as open_file:
                status, stdout, stderr, _ = self.run_cli(response, "--output", "unused.png")
                self.assertEqual(status, 1)
                self.assertEqual(stdout, "")
                self.assertIn("pane-inspect:", stderr)
                open_file.assert_not_called()

    def test_request_validation_before_connecting(self):
        for request in ("{", "NaN", "Infinity", '"' + "x" * inspect.MAX_REQUEST + '"'):
            with self.subTest(request=request[:20]):
                status, stdout, stderr, exchange = self.run_cli({}, request=request)
                self.assertEqual(status, 1)
                self.assertEqual(stdout, "")
                self.assertIn("pane-inspect:", stderr)
                exchange.assert_not_called()

    def test_request_limit_includes_newline(self):
        with mock.patch.object(inspect, "MAX_REQUEST", 4):
            status, _, _, exchange = self.run_cli({"ok": True, "result": None}, request='"x"')
            self.assertEqual(status, 0)
            exchange.assert_called_once_with("/private/inspect", b'"x"\n')
            status, _, _, exchange = self.run_cli({}, request='"xx"')
            self.assertEqual(status, 1)
            exchange.assert_not_called()

    def test_transport_errors_return_nonzero_without_traceback(self):
        for error in (OSError("connection refused"), TimeoutError("timed out"),
                      ValueError("invalid JSON"), RecursionError("too deeply nested")):
            with self.subTest(error=error), \
                    mock.patch.object(inspect, "exchange", side_effect=error), \
                    contextlib.redirect_stderr(io.StringIO()) as stderr, \
                    contextlib.redirect_stdout(io.StringIO()) as stdout:
                self.assertEqual(inspect.main(["/private/inspect", "{}"]), 1)
                self.assertEqual(stdout.getvalue(), "")
                self.assertIn("pane-inspect:", stderr.getvalue())
                self.assertNotIn("Traceback", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
