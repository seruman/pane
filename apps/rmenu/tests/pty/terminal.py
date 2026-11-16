"""Private PTY terminal protocol checks. No display or image files.

Adapted from Rune 1855d968, .auto/fake_terminal.py (blob
f4b3473ba0bb6bcfb3b5d76f959df18d25991703). See LICENSE.rune.
"""

import base64
from collections import Counter, deque
import ctypes
import errno
import fcntl
import hashlib
import json
import mmap
import os
import pty
import re
import select
import signal
import struct
import termios
import time
import zlib


CSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]")
LIBC = ctypes.CDLL(None, use_errno=True)
LIBC.shm_open.argtypes = [ctypes.c_char_p, ctypes.c_int, ctypes.c_uint]
LIBC.shm_open.restype = ctypes.c_int
LIBC.shm_unlink.argtypes = [ctypes.c_char_p]
LIBC.shm_unlink.restype = ctypes.c_int
MAX_PAYLOAD = 64 << 20


def shared_bytes(name, size):
    fd = LIBC.shm_open(name, os.O_RDONLY, 0)
    if fd < 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), os.fsdecode(name))
    try:
        assert 0 < size <= MAX_PAYLOAD, size
        assert os.fstat(fd).st_size >= size, "short shared memory object"
        with mmap.mmap(fd, size, access=mmap.ACCESS_READ) as region:
            return region[:]
    finally:
        os.close(fd)
        LIBC.shm_unlink(name)


class Terminal:
    def __init__(self, argv, root, *, data=b"", env=None, size=(640, 384),
                 transport="shm", delay=0.0, source_marker=None):
        self.root = root
        self.transport = transport
        self.delay = delay
        self.buffer = b""
        self.chunked = None
        self.pending = deque()
        self.frames = []
        self.counts = Counter()
        self.owned_groups = set()
        self.source_marker = source_marker
        self.code = None
        self.eof = False
        self.fd = None
        self.pid = None
        (root / "input").write_bytes(data)
        child_env = dict(os.environ)
        for key in list(child_env):
            if key.startswith(("REX_", "RUNE_TERMINAL", "GPUI_TERMINAL")) or key == "TERM_PROGRAM":
                child_env.pop(key, None)
        child_env.update(TERM="xterm-256color", RUNE_PERF_WARN="0")
        for key, value in (env or {}).items():
            if value is None:
                child_env.pop(key, None)
            else:
                child_env[key] = value
        width, height = size
        pid, fd = pty.fork()
        if pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ,
                            struct.pack("HHHH", max(1, height // 16), max(1, width // 8), width, height))
                # macOS detaches /dev/tty when the last slave descriptor closes.
                slave = os.dup(0)
                os.set_inheritable(slave, True)
                for number, name, flags in ((0, "input", os.O_RDONLY),
                                            (1, "output", os.O_WRONLY | os.O_CREAT | os.O_TRUNC),
                                            (2, "error", os.O_WRONLY | os.O_CREAT | os.O_TRUNC)):
                    opened = os.open(root / name, flags, 0o600)
                    os.dup2(opened, number)
                    if opened != number:
                        os.close(opened)
                os.execvpe(argv[0], argv, child_env)
            except BaseException:
                os._exit(127)
        self.pid, self.fd = pid, fd
        os.set_blocking(fd, False)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def send(self, data):
        deadline = time.monotonic() + 2
        while data:
            remaining = deadline - time.monotonic()
            assert remaining > 0, "PTY input timed out"
            if not select.select([], [self.fd], [], remaining)[1]:
                raise AssertionError("PTY input timed out")
            try:
                data = data[os.write(self.fd, data):]
            except BlockingIOError:
                continue

    def poll(self):
        if self.code is None and self.pid is not None:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.code = os.waitstatus_to_exitcode(status)
                self.pid = None
        return self.code

    def pump(self, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.consume_due()
            self.poll()
            if self.eof:
                time.sleep(min(0.01, max(0, deadline - time.monotonic())))
                continue
            wait = min(0.01, max(0, deadline - time.monotonic()))
            if not select.select([self.fd], [], [], wait)[0]:
                continue
            try:
                chunk = os.read(self.fd, 1 << 20)
            except OSError as error:
                if error.errno == errno.EIO:
                    chunk = b""
                elif error.errno == errno.EAGAIN:
                    continue
                else:
                    raise
            if not chunk:
                self.eof = True
                continue
            self.buffer += chunk
            assert len(self.buffer) <= MAX_PAYLOAD, "unterminated terminal command"
            self.parse()
        self.consume_due()

    def wait(self, predicate, message, timeout=10):
        deadline = time.monotonic() + timeout
        while not predicate():
            assert time.monotonic() < deadline, message
            assert self.poll() is None, (message, self.code, self.stderr())
            self.pump(0.02)

    def settle(self, quiet=0.2, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump(0.02)
            assert self.poll() is None, (self.code, self.stderr())
            if self.frames and not self.pending and time.monotonic() - self.frames[-1][0] >= quiet:
                return
        raise AssertionError("rendering did not settle")

    def finish(self, timeout=5):
        deadline = time.monotonic() + timeout
        while self.poll() is None:
            assert time.monotonic() < deadline, "picker did not exit"
            self.pump(0.02)
        return self.code, (self.root / "output").read_bytes(), self.stderr()

    def stderr(self):
        path = self.root / "error"
        return path.read_bytes() if path.exists() else b""

    def parse(self):
        while self.buffer:
            start = self.buffer.find(b"\x1b")
            if start < 0:
                self.buffer = b""
                return
            self.buffer = self.buffer[start:]
            if len(self.buffer) < 2 or self.buffer == b"\x1b_":
                return
            if self.buffer.startswith((b"\x1b_G", b"\x1b]")):
                end = self.buffer.find(b"\x1b\\")
                length = 2
                if self.buffer.startswith(b"\x1b]"):
                    bell = self.buffer.find(b"\x07")
                    if bell >= 0 and (end < 0 or bell < end):
                        end, length = bell, 1
                if end < 0:
                    return
                command, self.buffer = self.buffer[:end], self.buffer[end + length:]
                if command.startswith(b"\x1b_G"):
                    header, _, data = command[3:].partition(b";")
                    keys = dict(kv.split("=", 1) for kv in header.decode().split(",") if "=" in kv)
                    self.graphics(keys, data)
                else:
                    self.color(command[2:])
            elif self.buffer.startswith(b"\x1b["):
                match = CSI.match(self.buffer)
                if not match:
                    return
                command = match.group()
                self.buffer = self.buffer[match.end():]
                reply = {b"\x1b[?1016$p": b"\x1b[?1016;1$y",
                         b"\x1b[?996n": b"\x1b[?997;1n",
                         b"\x1b[c": b"\x1b[?62;22c"}.get(command)
                if reply:
                    self.send(reply)
            else:
                self.buffer = self.buffer[2:]

    def color(self, command):
        if not command.endswith(b";?"):
            return
        code = command[:-2]
        color = {b"10": "e6edf3", b"11": "0d1117", b"12": "2f81f7"}.get(code)
        if code.startswith(b"4;"):
            colors = "484f58 ff7b72 3fb950 d29922 58a6ff bc8cff 39c5cf b1bac4 6e7681 ffa198 56d364 e3b341 79c0ff d2a8ff 56d4dd ffffff".split()
            color = colors[int(code[2:])]
        if color:
            rgb = "/".join(color[i:i + 2] * 2 for i in (0, 2, 4))
            self.send(b"\x1b]" + code + b";rgb:" + rgb.encode() + b"\x1b\\")

    def graphics(self, keys, data):
        if self.chunked is not None:
            first, payload = self.chunked
            payload.extend(data)
            assert len(payload) <= MAX_PAYLOAD
            if keys.get("m") != "1":
                self.chunked = None
                self.inline(first, payload)
            return
        action, image = keys.get("a", "t"), keys.get("i")
        if action == "q":
            result = "ENOENT:no shared memory" if keys.get("t") == "s" and self.transport != "shm" else "OK"
            reply = f"\x1b_Gi={image};{result}\x1b\\".encode()
            # A fence cannot acknowledge pixels that the delayed reader has not consumed.
            if self.pending:
                self.pending.append((self.pending[-1][0], "reply", reply))
            else:
                self.send(reply)
        elif action == "f" and image == str(0x47500002):
            self.send(f"\x1b_Gi={image};OK\x1b\\".encode())
        elif action in ("T", "t", "f") and image == "1":
            if keys.get("t") == "s":
                name = base64.b64decode(data, validate=True)
                assert name.startswith(b"/") and b"\0" not in name, name
                self.pending.append((time.monotonic() + self.delay, "shared", (keys, name)))
            elif keys.get("m") == "1":
                self.chunked = (keys, bytearray(data))
            else:
                self.inline(keys, data)

    def consume_due(self):
        while self.pending and self.pending[0][0] <= time.monotonic():
            _, kind, payload = self.pending.popleft()
            if kind == "reply":
                if self.poll() is None:
                    self.send(payload)
            else:
                keys, name = payload
                raw = shared_bytes(name, int(keys["S"]))
                self.frame(keys, raw, "shm")

    def inline(self, keys, payload):
        raw = base64.b64decode(payload, validate=True)
        if keys.get("o") == "z":
            decoder = zlib.decompressobj()
            raw = decoder.decompress(raw, MAX_PAYLOAD + 1)
            assert decoder.eof and not decoder.unconsumed_tail
        self.frame(keys, raw, "inline")

    def frame(self, keys, raw, transport):
        action = keys.get("a", "t")
        width, height = int(keys["s"]), int(keys["v"])
        pixel_bytes = int(keys.get("f", "32")) // 8
        assert pixel_bytes in (3, 4)
        assert len(raw) == width * height * pixel_bytes, (keys, len(raw))
        self.counts[transport + "-" + action] += 1
        # Retain only a digest and metadata, never an image or a capture artifact.
        self.frames.append((time.monotonic(), hashlib.sha256(raw).hexdigest(), width, height))

    def close(self):
        try:
            if self.pid is not None:
                self.owned_groups.add(self.pid)
            # The test source publishes its group atomically before it starts writing.
            if self.source_marker is not None and self.source_marker.exists():
                source = json.loads(self.source_marker.read_text())
                assert source["pid"] == source["group"] and source["group"] > 1
                self.owned_groups.add(source["group"])
            for group in self.owned_groups:
                try:
                    os.killpg(group, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            deadline = time.monotonic() + 3
            while self.poll() is None and time.monotonic() < deadline:
                time.sleep(0.01)
            if self.pid is not None:
                raise AssertionError(f"child {self.pid} did not exit after SIGKILL")
        finally:
            for _, kind, payload in self.pending:
                if kind == "shared":
                    LIBC.shm_unlink(payload[1])
            if self.fd is not None:
                os.close(self.fd)
                self.fd = None
            self.source_marker = None
            self.owned_groups.clear()
