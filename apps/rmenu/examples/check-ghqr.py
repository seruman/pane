#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class GhqrTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ghqr-check-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.examples = self.root / "example ' with spaces"
        shutil.copytree(Path(__file__).resolve().parent, self.examples)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.caller = self.root / "caller"
        self.caller.mkdir()
        self.selected = self.root / 'selected " repo\nname'
        self.selected.mkdir()
        self.log = self.root / "calls.jsonl"
        self.env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}")
        for key in ("TERM_PROGRAM", "REX_SESSION", "REX_BLOCK"):
            self.env.pop(key, None)
        self.env.update(
            CHECK_LOG=str(self.log),
            CHECK_ROOT=str(self.root),
            CHECK_RESULT=json.dumps({"action": "accept", "source": "repos",
                                     "items": [{"value": str(self.selected)}]}),
            RMENU_BIN=str(self.bin / "menu"),
        )
        for dependency in ("ghq", "git-memo"):
            self.script(dependency, "#!/bin/sh\nexit 0\n")
        self.script("menu", f"#!{shutil.which('python3')}\n" + '''import json, os, sys
from pathlib import Path
args = sys.argv[1:]
config = Path(args[args.index("--config") + 1])
with open(os.environ["CHECK_LOG"], "a") as log:
    log.write(json.dumps({"menu": args, "config": json.loads(config.read_text()),
                          "cwd": os.getcwd(), "dir": str(config.parent)}) + "\\n")
print(os.environ["CHECK_RESULT"])
print("picker diagnostic", file=sys.stderr)
sys.exit(int(os.environ.get("CHECK_STATUS", "0")))
''')
        self.script("rex", f"#!{shutil.which('python3')}\n" + '''import json, os, subprocess, sys
args = sys.argv[1:]
with open(os.environ["CHECK_LOG"], "a") as log:
    log.write(json.dumps({"rex": args}) + "\\n")
if args == ["session", "inspect", "fake-session", "--json"]:
    print(json.dumps({"windows": [{"window_id": "fake-window", "layers": [
        {"blocks": [{"block_id": "fake-block"}]}]}]}))
elif args[:4] == ["api", "call", "-s", "fake-session"]:
    request = json.loads(args[5])
    if args[4] == "session.new_layer":
        options = request["layout"]["block"]["options"]
        subprocess.run(options["command"], cwd=options["cwd"], timeout=5, check=False)
        print(json.dumps({"layer_id": "fake-layer", "block_ids": ["fake-picker"]}))
    elif args[4] not in ("session.close_layer", "session.focus_block"):
        sys.exit(91)
elif args != ["wait", "fake-picker"]:
    sys.exit(92)
''')

    def script(self, name, text):
        path = self.bin / name
        path.write_text(text)
        path.chmod(0o755)

    def run_picker(self, rex=False, query="a ' query with $symbols"):
        if rex:
            self.env.update(TERM_PROGRAM="rex", REX_SESSION="fake-session", REX_BLOCK="fake-block")
        result = subprocess.run(
            ["fish", "--no-config", "-c", '''
source $argv[1]
function _git_dir_action
    printf '%s\\0' $argv >$CHECK_ROOT/action
end
cd -- $argv[2]
ghqr "$argv[3]"
set -l code $status
printf '%s\\0%s\\0' $code "$PWD" >$CHECK_ROOT/result
exit $code
''', str(self.examples / "ghqr.fish"), str(self.caller), query],
            cwd=self.root, env=self.env, capture_output=True, text=True, timeout=10,
        )
        self.calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.menu = next(call for call in self.calls if "menu" in call)
        self.assertFalse(Path(self.menu["dir"]).exists(), "picker temporary directory leaked")
        self.assertIn("picker diagnostic", result.stderr)
        self.assertIn("--query=" + query, self.menu["menu"])
        self.assertEqual(self.menu["cwd"], str(self.caller))
        for source in self.menu["config"]["sources"]:
            command = source["command"]
            self.assertTrue(Path(command[0]).is_absolute(), source)
            self.assertTrue(Path(command[0]).is_file(), source)
        return result

    def test_terminal_accept_changes_to_selected_directory(self):
        result = self.run_picker()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "result").read_bytes(),
                         f"0\0{self.selected}\0".encode())
        self.assertFalse(any("rex" in call for call in self.calls))

    def test_layer_preserves_query_and_closes(self):
        self.env["CHECK_RESULT"] = json.dumps({
            "action": "split-right", "source": "repos", "items": [{"value": str(self.selected)}]})
        result = self.run_picker(rex=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "action").read_bytes(), f"right\0{self.selected}\0".encode())
        rex_calls = [call["rex"] for call in self.calls if "rex" in call]
        request = json.loads(rex_calls[1][-1])
        self.assertEqual(request["window_id"], "fake-window")
        self.assertEqual(rex_calls[-2][4], "session.close_layer")
        self.assertEqual(rex_calls[-1][4], "session.focus_block")

    def test_layer_preserves_failure_and_cancellation_status(self):
        for status in (2, 130):
            with self.subTest(status=status):
                self.env["CHECK_STATUS"] = str(status)
                result = self.run_picker(rex=True)
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertFalse((self.root / "action").exists())
                self.log.unlink()

    def test_memo_split_keeps_repository_label(self):
        self.env["CHECK_RESULT"] = json.dumps({
            "action": "split-below", "source": "memos", "items": [{"value": str(self.selected)}],
            "path": [{"source": "repos", "items": [{"value": str(self.caller)}]}]})
        result = self.run_picker(rex=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "action").read_bytes(),
                         f"below\0{self.selected}\0{self.caller}\0".encode())

    def test_memo_without_repository_parent_is_refused(self):
        self.env["CHECK_RESULT"] = json.dumps({
            "action": "project-tab", "source": "memos", "items": [{"value": str(self.selected)}],
            "path": [{"source": "worktrees", "items": [{"value": str(self.caller)}]}]})
        result = self.run_picker(rex=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertFalse((self.root / "action").exists())


if __name__ == "__main__":
    unittest.main()
