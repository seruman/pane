# Regenerate the Rune icons offline

Run from the repository root with Python 3.9 or newer. No packages, downloads, or
Rust build are required.

```sh
python3 scripts/generate_icons.py
python3 scripts/generate_icons.py --check
python3 -I -m unittest discover -s scripts -p 'test_generate_icons.py'
```

`--check` fails if `crates/pane-ui/src/icons.rs` is missing or differs from the
sources. It does not write files. Edit the SVG sources rather than the Rust file.
The generator sorts filenames and emits their lowercase kebab-case names as
`Icon.name` and uppercase snake-case names as public Rust statics.

## The SVGs preserve the existing pane snapshot

These 48 SVGs come from the icon bodies in `crates/pane-ui/src/icons.rs` at pane
commit `b7571d7a30bdc175800b9a36461083fd2a4fec76`. They are normalized snapshots,
not original upstream SVG files. The repository contained neither the original
SVGs nor the generator. The original header attributed the icons to
[Lucide](https://lucide.dev) under the ISC licence. The exact upstream revision
and original filenames are unknown. See [Lucide's licence](https://lucide.dev/license).

Extraction kept every path command, shape attribute, element order, public
static name, icon name, and 24-unit view size. Shared stroke and fill attributes
moved to the SVG root. Four palette circles had duplicate `fill` attributes.
Extraction kept the first value, `currentColor`, to match Rune's existing
first-attribute lookup, and discarded the later `none`. The circles stay filled.

The snapshot test pins a SHA-256 digest of all original statics, names, view
sizes, and ordered shapes with sorted attributes, using those first-attribute
semantics. This checks geometry and presentation without relying on attribute
order or a copy of the generator's output. An intentional icon change must also
update that baseline after review.

## Sources use flat, normalized shapes

The supported format is an SVG namespace root with `viewBox="0 0 24 24"` and
flat `path`, `circle`, `rect`, or `line` children. Existing polyline and polygon
geometry was already expressed as paths in the Rust snapshot. The generator
does not convert new polylines or polygons. It rejects unsupported elements,
root attributes, and nested shapes instead of silently dropping them.

Root `stroke`, `stroke-width`, `stroke-linecap`, `stroke-linejoin`, and `fill`
attributes propagate to each shape only when that shape does not set its own
value. The generator parses each source and each emitted body as XML, so
duplicate attributes fail generation before it writes the Rust file.
