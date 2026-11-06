#!/usr/bin/env python3
"""Makes fontawesome-solid-subset.ttf and fontawesome-regular-subset.ttf, the admonition icons.

The sources are Font Awesome Free 5.15.1's solid and regular fonts as prawn-icon 3.0.0 ships
them (data/fonts/fas/fa-solid.ttf and data/fonts/far/fa-regular.ttf), the fonts asciidoctor-pdf
2.3 draws admonition icons with. Each subset keeps the glyphs named below, at the code points
the fonts' legends (fas.yml, far.yml) give them, and nothing else is changed. The names and code
points must match ICONS in crates/adoc-pdf/src/icons.rs.

Run with fontTools 4.55 (pip install fonttools==4.55.3, in a virtual environment):

    python3 fontawesome-subset.py <prawn-icon>/data/fonts
"""

import subprocess
import sys
from pathlib import Path

SOLID = {
    "ban": 0xF05E,
    "bell": 0xF0F3,
    "bolt": 0xF0E7,
    "bug": 0xF188,
    "check": 0xF00C,
    "check-circle": 0xF058,
    "comment": 0xF075,
    "comment-alt": 0xF27A,
    "exclamation": 0xF12A,
    "exclamation-circle": 0xF06A,
    "exclamation-triangle": 0xF071,
    "fire": 0xF06D,
    "fire-alt": 0xF7E4,
    "flag": 0xF024,
    "hand-paper": 0xF256,
    "heart": 0xF004,
    "info": 0xF129,
    "info-circle": 0xF05A,
    "lightbulb": 0xF0EB,
    "question": 0xF128,
    "question-circle": 0xF059,
    "star": 0xF005,
    "sticky-note": 0xF249,
    "stop-circle": 0xF28D,
    "thumbtack": 0xF08D,
    "times-circle": 0xF057,
}

REGULAR = {
    "bell": 0xF0F3,
    "check-circle": 0xF058,
    "comment": 0xF075,
    "comment-alt": 0xF27A,
    "flag": 0xF024,
    "hand-paper": 0xF256,
    "heart": 0xF004,
    "lightbulb": 0xF0EB,
    "question-circle": 0xF059,
    "star": 0xF005,
    "sticky-note": 0xF249,
    "stop-circle": 0xF28D,
    "times-circle": 0xF057,
}


def legend(path: Path) -> dict:
    """Name to code point, from a prawn-icon legend (one `  name: "<glyph>"` per line)."""
    names = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        name, _, value = line.strip().partition(": ")
        value = value.strip('"')
        if len(value) == 1:
            names[name] = ord(value)
    return names


def subset(source: Path, legend_path: Path, glyphs: dict, output: Path) -> None:
    known = legend(legend_path)
    for name, code in glyphs.items():
        if known.get(name) != code:
            sys.exit(f"{name}: {legend_path} gives {known.get(name)!r}, not {code:#x}")
    unicodes = ",".join(f"U+{code:04X}" for code in sorted(set(glyphs.values())))
    subprocess.run(
        [
            sys.executable,
            "-m",
            "fontTools.subset",
            str(source),
            f"--unicodes={unicodes}",
            f"--output-file={output}",
            "--name-IDs=*",
            "--name-languages=*",
            "--notdef-outline",
            "--no-hinting",
            "--layout-features=",
        ],
        check=True,
    )


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    fonts = Path(sys.argv[1])
    here = Path(__file__).resolve().parent
    subset(
        fonts / "fas" / "fa-solid.ttf",
        fonts / "fas" / "fas.yml",
        SOLID,
        here / "fontawesome-solid-subset.ttf",
    )
    subset(
        fonts / "far" / "fa-regular.ttf",
        fonts / "far" / "far.yml",
        REGULAR,
        here / "fontawesome-regular-subset.ttf",
    )


if __name__ == "__main__":
    main()
