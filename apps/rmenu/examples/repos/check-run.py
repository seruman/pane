#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class RepoLauncherTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="repos-run-check-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.examples = self.root / 'examples with "quotes"'
        shutil.copytree(Path(__file__).resolve().parent, self.examples)
        self.caller = self.root / "caller"
        self.caller.mkdir()
        self.menu = self.caller / "menu"
        self.menu.write_text(f"#!{shutil.which('python3')}\n" + '''import json, os, sys
from pathlib import Path
args = sys.argv[1:]
config = Path(args[args.index("--config") + 1]).resolve()
Path(os.environ["CHECK_LOG"]).write_text(json.dumps({
    "args": args, "cwd": os.getcwd(), "config_path": str(config),
    "config": json.loads(config.read_text())}))
print("menu output")
print("menu diagnostic", file=sys.stderr)
sys.exit(int(os.environ.get("CHECK_STATUS", "0")))
''')
        self.menu.chmod(0o755)
        self.log = self.root / "call.json"

    def launch(self, menu, status=0):
        result = subprocess.run(
            [str(self.examples / "run.sh"), "--query", "a ' query"], cwd=self.caller,
            env=dict(os.environ, RMENU_BIN=menu, CHECK_LOG=str(self.log), CHECK_STATUS=str(status)),
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, status, result.stderr)
        self.assertEqual(result.stdout, "menu output\n")
        self.assertEqual(result.stderr, "menu diagnostic\n")
        call = json.loads(self.log.read_text())
        self.assertEqual(call["cwd"], str(self.caller))
        self.assertEqual(call["args"][-3:], ["--json", "--query", "a ' query"])
        self.assertFalse(Path(call["config_path"]).exists(), "temporary config leaked")
        commands = [source["command"] for source in call["config"]["sources"]]
        commands.append(call["config"]["preview"])
        for command in commands:
            self.assertTrue(Path(command[0]).is_absolute(), command)
            self.assertTrue(Path(command[0]).is_file(), command)
        template = json.loads((self.examples / "config.json").read_text())
        self.assertEqual(call["config"]["bindings"], template["bindings"])
        return call

    def test_absolute_commands_preserve_caller_directory(self):
        self.launch(str(self.menu))

    def test_relative_binary_override_uses_caller_directory(self):
        self.launch("./menu")

    def test_failure_preserves_status_and_removes_config(self):
        self.launch(str(self.menu), status=2)


if __name__ == "__main__":
    unittest.main()
