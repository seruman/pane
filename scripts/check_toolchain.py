#!/usr/bin/env python3
"""Check the Rust channel against the local Rune dependency."""
import sys
import tomllib
from pathlib import Path


def check(root: Path) -> None:
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    rune = manifest["workspace"]["dependencies"]["rune"]
    dependency = (root / rune["path"]).resolve()
    upstream = next(path / "rust-toolchain.toml" for path in dependency.parents
                    if (path / "rust-toolchain.toml").is_file())
    local = root / "rust-toolchain.toml"
    channel = lambda path: tomllib.loads(path.read_text())["toolchain"]["channel"]
    if channel(local) != channel(upstream):
        raise SystemExit(f"Rust channel mismatch: {local}={channel(local)}, {upstream}={channel(upstream)}")


if __name__ == "__main__":
    check(Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent)
