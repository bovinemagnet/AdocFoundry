"""Fills fuzz/corpus/<target>/ with seeds from the repository's fixtures.

Run from anywhere with: python3 fuzz/seed.py

The pages under tests/fixtures seed the parsers and the model; the targets of their xrefs,
includes and images seed the resource id parser; the formulas of tests/fixtures/stem seed the
STEM check; and the pages' lines seed the string literal writer. Each seed is named by its
hash, so running the script again adds nothing twice. The corpus folder is not committed.
"""

import hashlib
import os
import pathlib
import re

FUZZ = pathlib.Path(__file__).resolve().parent
FIXTURES = FUZZ.parent / "tests" / "fixtures"
CORPUS = FUZZ / "corpus"

# The first byte of a math_is_safe input chooses what the rest is.
TYPST, ASCIIMATH, LATEX = b"\x00", b"\x01", b"\x02"

TARGET = re.compile(r"(?:xref:|include::|image::?)([^\[\s]+)\[")
FORMULA = re.compile(r"(stem|asciimath|latexmath):\[([^\]]*)\]")


def write(target, data):
    folder = CORPUS / target
    folder.mkdir(parents=True, exist_ok=True)
    (folder / hashlib.sha1(data).hexdigest()).write_bytes(data)


def formulas(name):
    """The formulas of a stem fixture and the Typst they convert to, skipping comments."""
    path = FIXTURES / "stem" / name
    for line in path.read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            yield line.split("\t", 1)


def main():
    pages = sorted(FIXTURES.rglob("*.adoc"))
    for page in pages:
        data = page.read_bytes()
        for target in ("parse_source", "parse_dom", "model_roundtrip"):
            write(target, data)
        text = data.decode("utf-8", errors="replace")
        for match in TARGET.finditer(text):
            write("antora_resource_id", match.group(1).encode())
        for match in FORMULA.finditer(text):
            notation = LATEX if match.group(1) == "latexmath" else ASCIIMATH
            write("math_is_safe", notation + match.group(2).encode())
        for line in text.splitlines():
            if line.strip():
                write("pdf_literal", line.encode())
    for name, notation in (("asciimath.txt", ASCIIMATH), ("latex.txt", LATEX)):
        for formula, *typst in formulas(name):
            write("math_is_safe", notation + formula.encode())
            for converted in typst:
                write("math_is_safe", TYPST + converted.encode())
    for target in sorted(os.listdir(CORPUS)):
        print(f"{target}: {len(os.listdir(CORPUS / target))} seeds")


if __name__ == "__main__":
    main()
