#!/usr/bin/env python3
"""Renames workspace members: each crate or app's folder (`git mv`), its package name
(`old-name`) and its Rust name (`old_name`) wherever they appear in the workspace's tracked text
files. Names are matched whole, so renaming `foo` leaves `foo-bar` alone.

    scripts/rename.py OLD NEW [OLD NEW ...]
    scripts/rename.py vimkeys vim-keys mdview md-view

Run from the workspace root, then `cargo build` to refresh Cargo.lock.
"""

import os
import re
import subprocess
import sys

NAME = r"[A-Za-z0-9_-]"


def tracked_text_files():
    out = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True).stdout
    for path in out.decode().split("\0"):
        if not path or path == "Cargo.lock" or not os.path.isfile(path):
            continue
        try:
            with open(path, encoding="utf-8") as f:
                yield path, f.read()
        except UnicodeDecodeError:
            continue


def rename(old, new):
    folders = [f"{d}/{old}" for d in ("crates", "apps") if os.path.isdir(f"{d}/{old}")]
    if not folders:
        sys.exit(f"rename: no crates/{old} or apps/{old}")
    for folder in folders:
        subprocess.run(["git", "mv", folder, folder.rsplit("/", 1)[0] + "/" + new], check=True)
    forms = {old: new, old.replace("-", "_"): new.replace("-", "_")}
    patterns = [(re.compile(rf"(?<!{NAME}){re.escape(a)}(?!{NAME})"), b) for a, b in forms.items()]
    for path, text in tracked_text_files():
        changed = text
        for pattern, b in patterns:
            changed = pattern.sub(b, changed)
        if changed != text:
            with open(path, "w", encoding="utf-8") as f:
                f.write(changed)


def main():
    pairs = sys.argv[1:]
    if not pairs or len(pairs) % 2:
        sys.exit(__doc__)
    for old, new in zip(pairs[::2], pairs[1::2]):
        rename(old, new)


if __name__ == "__main__":
    main()
