#!/usr/bin/env python3
"""Run with python3 -I -m unittest discover -s scripts -p 'test_generate_icons.py'."""

import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

SCRIPT = Path(__file__).with_name("generate_icons.py")
spec = importlib.util.spec_from_file_location("generate_icons", SCRIPT)
icons = importlib.util.module_from_spec(spec)
spec.loader.exec_module(icons)


class GenerateIconsTests(unittest.TestCase):
    def test_snapshot_preserves_public_names_geometry_and_presentation(self):
        records = []
        pattern = (
            r'pub static (\w+): Icon = Icon \{\n    name: "([^"]+)",\n'
            r'    view: ([\d.]+),\n    body: r#"(.*?)"#'
        )
        for symbol, name, view, body in re.findall(pattern, icons.generate()):
            root = ET.fromstring(f"<svg>{body}</svg>")
            records.append((symbol, name, view, [
                (shape.tag, sorted(shape.attrib.items())) for shape in root
            ]))
        self.assertEqual(len(records), 48)
        digest = hashlib.sha256(
            json.dumps(sorted(records), separators=(",", ":")).encode()
        ).hexdigest()
        self.assertEqual(
            digest, "c1fb6a3067f110b111c6231210c42ce91732915083c65051c7f42f47b8a2f21c",
            "The snapshot uses the original Rune bodies with first-attribute semantics",
        )

    def test_palette_circles_stay_filled(self):
        root = ET.fromstring(f'<svg>{icons.icon_body(icons.SOURCES / "palette.svg")}</svg>')
        self.assertEqual([circle.get("fill") for circle in root.findall("circle")],
                         ["currentColor"] * 4)
        self.assertEqual(root.find("path").get("fill"), "none")

    def test_child_presentation_wins_and_missing_attributes_inherit(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "override.svg"
            source.write_text('''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"
                fill="none" stroke="currentColor" stroke-width="2"
                stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none"
                    stroke-width="3" stroke-linecap="butt" stroke-linejoin="bevel"/>
                <path d="M0 0L1 1"/>
            </svg>''')
            root = ET.fromstring(f"<svg>{icons.icon_body(source)}</svg>")
            self.assertEqual(
                {key: root[0].get(key) for key in icons.PRESENTATION},
                {"stroke": "none", "stroke-width": "3", "stroke-linecap": "butt",
                 "stroke-linejoin": "bevel", "fill": "currentColor"},
            )
            self.assertEqual(
                {key: root[1].get(key) for key in icons.PRESENTATION},
                {"stroke": "currentColor", "stroke-width": "2", "stroke-linecap": "round",
                 "stroke-linejoin": "round", "fill": "none"},
            )

    def test_cli_is_deterministic_and_check_detects_drift_without_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            shutil.copy(SCRIPT, root / "scripts/generate_icons.py")
            shutil.copytree(icons.SOURCES, root / "assets/icons")
            output = root / "crates/pane-ui/src/icons.rs"
            output.parent.mkdir(parents=True)

            def run(*args):
                return subprocess.run(
                    [sys.executable, "-I", str(root / "scripts/generate_icons.py"), *args],
                    cwd=root.parent, capture_output=True, text=True, check=False,
                )

            self.assertEqual(run("--check").returncode, 1)
            self.assertFalse(output.exists())
            self.assertEqual(run().returncode, 0)
            first = output.read_bytes()
            self.assertEqual(run().returncode, 0)
            self.assertEqual(output.read_bytes(), first)
            self.assertEqual(run("--check").returncode, 0)
            output.write_bytes(first + b"\n")
            self.assertEqual(run("--check").returncode, 1)
            self.assertEqual(output.read_bytes(), first + b"\n")
            self.assertEqual(run().returncode, 0)
            source = root / "assets/icons/check.svg"
            source.write_text(source.read_text().replace('stroke-width="2"', 'stroke-width="3"'))
            self.assertEqual(run("--check").returncode, 1)
            self.assertEqual(output.read_bytes(), first)
            self.assertEqual(run().returncode, 0)
            self.assertNotEqual(output.read_bytes(), first)
            self.assertEqual(run("--check").returncode, 0)
            before_invalid_source = output.read_bytes()
            source.write_text(source.read_text().replace('fill="none"', 'fill="none" fill="none"'))
            self.assertEqual(run().returncode, 1)
            self.assertEqual(output.read_bytes(), before_invalid_source)

    def test_checked_in_output_matches_sources(self):
        self.assertEqual(icons.OUTPUT.read_bytes(), icons.generate().encode("utf-8"))

    def test_unsupported_svg_is_rejected_instead_of_silently_changed(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "unsupported.svg"
            for contents in (
                '<g><path d="M0 0L1 1"/></g>',
                '<polygon points="0,0 1,1 2,0"/>',
            ):
                with self.subTest(contents=contents):
                    source.write_text(
                        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
                        + contents + '</svg>'
                    )
                    with self.assertRaises(ValueError):
                        icons.icon_body(source)


if __name__ == "__main__":
    unittest.main()
