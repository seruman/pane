#!/usr/bin/env python3
"""Run rmenu against private child PTYs using only the Python standard library."""

import argparse
import base64
import json
import mmap
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid

# -I excludes the script directory from sys.path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from terminal import LIBC, Terminal


ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target" / "check" / "debug" / "rmenu"


class PickerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="rmenu-pty-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def terminal(self, args=(), **options):
        terminal = Terminal([str(BINARY), *args], self.root, **options)
        self.addCleanup(terminal.close)
        return terminal

    def ready(self, terminal):
        terminal.wait(lambda: bool(terminal.frames), "no rendered frame")
        terminal.settle()

    def test_outcomes(self):
        item = {"id": "beta-id", "title": "Beta title", "value": "beta\nvalue", "detail": "details"}
        cases = [
            ("line", b"alpha\nbeta\n", [], b"\r", 0, b"alpha\n"),
            ("nul", b"alpha\0beta\0", ["--input", "nul", "--print0"], b"\r", 0, b"alpha\0"),
            ("json", (json.dumps(item) + "\n").encode(), ["--input", "jsonl", "--json"], b"\r", 0, None),
            ("cancelled", b"alpha\n", [], b"\x1b", 130, b""),
            ("malformed", b"{bad json}\n", ["--input", "jsonl"], b"\x1b", 2, b""),
        ]
        for label, data, args, key, expected_code, expected_output in cases:
            with self.subTest(case=label), self.terminal(args, data=data) as terminal:
                self.ready(terminal)
                terminal.send(key)
                code, output, error = terminal.finish()
                self.assertEqual(code, expected_code, error)
                if label == "json":
                    context = json.loads(output)
                    self.assertEqual(context["action"], "accept")
                    self.assertEqual(context["query"], "")
                    self.assertEqual(context["items"], [item])
                    self.assertEqual(context["path"], [{"items": [item], "query": ""}])
                else:
                    self.assertEqual(output, expected_output)
                self.assertEqual(bool(error), label == "malformed", error)

    def test_caller_environment(self):
        config = self.root / "config.json"
        config.write_text(json.dumps({"sources": [{"id": "env", "label": "Environment", "command": [
            sys.executable, "-I", "-c", "import os; print('<' + os.environ.get('RUNE_TERMINAL', 'unset') + '>')"
        ]}]}))
        for value in (None, "0", "1"):
            with self.subTest(RUNE_TERMINAL=value), self.terminal(
                ["--config", str(config)], env={"RUNE_TERMINAL": value}
            ) as terminal:
                self.ready(terminal)
                terminal.send(b"\r")
                code, output, error = terminal.finish()
                self.assertEqual((code, output, error), (0, f"<{value or 'unset'}>\n".encode(), b""))

    def test_requires_controlling_tty(self):
        result = subprocess.run([str(BINARY)], stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                env=dict(os.environ, RUNE_TERMINAL="0"),
                                start_new_session=True, timeout=5)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"tty", result.stderr.lower())

    def test_typing_and_delayed_shared_memory(self):
        for transport, size in (("shm", (640, 384)), ("inline", (240, 128))):
            with self.subTest(transport=transport, size=size), self.terminal(
                data=b"alpha\nbeta\ngamma\n", transport=transport, size=size, delay=0.05
            ) as terminal:
                self.ready(terminal)
                self.assertEqual(terminal.frames[0][2:], size)
                before = len(terminal.frames)
                terminal.send(b"beta")
                terminal.wait(lambda: len(terminal.frames) > before, "typing did not redraw")
                terminal.settle()
                self.assertGreater(len({frame[1] for frame in terminal.frames}), 1)
                if transport == "shm":
                    self.assertGreater(terminal.counts["shm-f"], 0, "no delayed shared-memory edits exercised")
                else:
                    self.assertTrue(any(kind.startswith("inline-") for kind in terminal.counts))
                    self.assertFalse(any(kind.startswith("shm-") for kind in terminal.counts))
                print(f"\n{transport} {size}, 50 ms delayed reads, payloads {dict(terminal.counts)}", flush=True)
                terminal.send(b"\r")
                self.assertEqual(terminal.finish(), (0, b"beta\n", b""))

    def test_quit_during_load(self):
        marker = self.root / "source.json"
        config = self.root / "config.json"
        source = Path(__file__).with_name("source.py")
        config.write_text(json.dumps({"sources": [{"id": "load", "label": "Loading", "command": [
            sys.executable, "-I", str(source), str(marker)
        ]}]}))
        for how in ("escape", "ctrl-c", "TERM", "HUP", "INT"):
            marker.unlink(missing_ok=True)
            with self.subTest(quit=how), self.terminal(
                ["--config", str(config)], source_marker=marker
            ) as terminal:
                terminal.wait(marker.exists, "source did not start")
                child = json.loads(marker.read_text())
                self.assertEqual(child["pid"], child["group"])
                terminal.owned_groups.add(child["group"])
                terminal.wait(lambda: bool(terminal.frames), "no frame during load")
                if how == "escape":
                    terminal.send(b"\x1b")
                elif how == "ctrl-c":
                    terminal.send(b"\x1b[99;5u")
                else:
                    os.kill(terminal.pid, getattr(signal, "SIG" + how))
                code, output, error = terminal.finish()
                self.assertEqual((code, output, error), (130, b"", b""))
                with self.assertRaises(ProcessLookupError, msg=f"source survived {how}"):
                    os.kill(child["pid"], 0)
                print(f"\n{how}, exit 130, source reaped", flush=True)

    def test_fragmented_terminal_commands(self):
        with Terminal(["/bin/sleep", "30"], self.root) as terminal:
            replies = []
            terminal.send = replies.append
            commands = (b"\x1b[?1016$p\x1b]10;?\x07\x1b[?996n\x1b[c"
                        b"\x1b_Ga=T,i=1,s=1,v=1,f=24,m=1;cm\x1b\\"
                        b"\x1b_Gm=0;di\x1b\\")
            for byte in commands:
                terminal.buffer += bytes([byte])
                terminal.parse()
            self.assertEqual(replies, [b"\x1b[?1016;1$y",
                                       b"\x1b]10;rgb:e6e6/eded/f3f3\x1b\\",
                                       b"\x1b[?997;1n", b"\x1b[?62;22c"])
            self.assertEqual(terminal.counts["inline-T"], 1)
            self.assertEqual(terminal.buffer, b"")
            self.assertIsNone(terminal.chunked)

    def test_cleanup_after_failure(self):
        terminal = Terminal(["/bin/sleep", "30"], self.root)
        pid, fd = terminal.pid, terminal.fd
        with self.assertRaisesRegex(RuntimeError, "injected failure"):
            with terminal:
                raise RuntimeError("injected failure")
        with self.assertRaises(ChildProcessError):
            os.waitpid(pid, os.WNOHANG)
        with self.assertRaises(OSError):
            os.fstat(fd)
        terminal.close()

    def test_delayed_reader_rejects_early_unlink(self):
        name = ("/rmenu-pty-" + uuid.uuid4().hex[:16]).encode()
        fd = LIBC.shm_open(name, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
        self.assertGreaterEqual(fd, 0)
        try:
            os.ftruncate(fd, 3)
            with mmap.mmap(fd, 3, access=mmap.ACCESS_WRITE) as region:
                region[:] = b"rgb"
            with Terminal(["/bin/sleep", "30"], self.root, delay=0.05) as terminal:
                terminal.graphics({"a": "f", "i": "1", "t": "s", "S": "3",
                                   "s": "1", "v": "1", "f": "24"}, base64.b64encode(name))
                self.assertEqual(len(terminal.pending), 1)
                self.assertEqual(len(terminal.frames), 0)
                LIBC.shm_unlink(name)
                time.sleep(0.06)
                with self.assertRaises(FileNotFoundError):
                    terminal.consume_due()
        finally:
            os.close(fd)
            LIBC.shm_unlink(name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=os.environ.get("RMENU_BIN", BINARY))
    args, unittest_args = parser.parse_known_args()
    BINARY = args.binary.expanduser().resolve()
    if not BINARY.is_file() or not os.access(BINARY, os.X_OK):
        parser.error(f"build rmenu first or pass --binary, not executable: {BINARY}")
    unittest.main(argv=[sys.argv[0], *unittest_args], verbosity=2)
