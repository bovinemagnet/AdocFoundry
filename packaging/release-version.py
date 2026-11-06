#!/usr/bin/env python3
"""The release's version, checked against the files that carry it, and its release notes.

usage: packaging/release-version.py check <version>
       packaging/release-version.py notes <version> <file>

`check` fails, naming each file, unless `Cargo.toml`'s `[workspace.package]` version, the
documentation's `display_version` in `src/docs/antora.yml`, a `## <version> ` heading in
`CHANGELOG.md`, the Flatpak manifest's source `tag: v<version>` and the AppStream metadata's
newest `<release>` all give <version>: a tag that does not match them would ship binaries reporting
another version, which the updater would offer again and again. `cargo xtask bump <version>`
writes them all.

`notes` writes the changelog's section for <version>, without its heading, to <file>.

Run from the repository root.
"""

import os
import re
import sys

FLATPAK = os.path.join("packaging", "flatpak", "io.github.bovinemagnet.AdocFoundry.yml")
METAINFO = os.path.join("packaging", "linux", "io.github.bovinemagnet.AdocFoundry.metainfo.xml")


def read(root, path):
    with open(os.path.join(root, path), encoding="utf-8") as file:
        return file.read()


def workspace_version(cargo):
    """The `version` of `[workspace.package]` in Cargo.toml's text, or None."""
    table = None
    for line in cargo.splitlines():
        heading = re.match(r"^\s*\[([^\]]+)\]\s*$", line)
        if heading:
            table = heading.group(1).strip()
            continue
        match = re.match(r'^\s*version\s*=\s*"([^"]*)"', line)
        if match and table == "workspace.package":
            return match.group(1)
    return None


def problems(root, version):
    """What disagrees with `version`, a line for each file."""
    found = []
    cargo = workspace_version(read(root, "Cargo.toml"))
    if cargo != version:
        found.append(f"Cargo.toml: [workspace.package] version is {cargo}, not {version}")
    antora = os.path.join("src", "docs", "antora.yml")
    match = re.search(r"(?m)^display_version:\s*'?\"?([^'\"\n]*)", read(root, antora))
    shown = match.group(1).strip() if match else None
    if shown != version:
        found.append(f"{antora}: display_version is {shown}, not {version}")
    if not re.search(rf"(?m)^## {re.escape(version)} ", read(root, "CHANGELOG.md")):
        found.append(f"CHANGELOG.md: no `## {version} ` heading")
    match = re.search(r"(?m)^\s+tag:\s*(\S+)\s*$", read(root, FLATPAK))
    tag = match.group(1) if match else None
    if tag != f"v{version}":
        found.append(f"{FLATPAK}: the source's tag is {tag}, not v{version}")
    match = re.search(r'<release\s+version="([^"]*)"', read(root, METAINFO))
    newest = match.group(1) if match else None
    if newest != version:
        found.append(f"{METAINFO}: the newest release is {newest}, not {version}")
    return found


def notes(changelog, version):
    """The section of `changelog` headed `## <version> …`, without its heading, or None."""
    lines = changelog.splitlines(keepends=True)
    for start, line in enumerate(lines):
        if line.startswith(f"## {version} "):
            body = []
            for line in lines[start + 1:]:
                if line.startswith("## "):
                    break
                body.append(line)
            return "".join(body).strip("\n") + "\n"
    return None


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "check":
        found = problems(".", sys.argv[2])
        for problem in found:
            print(f"FAILED: {problem}")
        if found:
            sys.exit(f"the release's files do not give version {sys.argv[2]}; "
                     "run `cargo xtask bump <version>` and commit before tagging")
        print("ok: Cargo.toml, src/docs/antora.yml, CHANGELOG.md, the Flatpak manifest and the "
              f"metainfo give {sys.argv[2]}")
    elif len(sys.argv) == 4 and sys.argv[1] == "notes":
        text = notes(read(".", "CHANGELOG.md"), sys.argv[2])
        if text is None:
            sys.exit(f"CHANGELOG.md has no section for {sys.argv[2]}")
        with open(sys.argv[3], "w", encoding="utf-8") as file:
            file.write(text)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
