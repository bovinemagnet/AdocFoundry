"""Tests of the release's version check and its release notes from the changelog.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import importlib.util
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    "release_version", os.path.join(HERE, "release-version.py"))
release_version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_version)

CARGO = """[workspace]
members = ["crates/*"]

[workspace.package]
version = "0.5.0"
edition = "2021"

[workspace.dependencies]
serde = { version = "=1.0.0" }
"""

ANTORA = """name: adocfoundry
version: ~
display_version: '0.5.0'
"""

CHANGELOG = """# Changelog

## Unreleased

### Added

- Something to come.

## 0.5.0 — 2026-11-02

### Added

- New Page (#72).

### Fixed

- A fix.

## 0.4.0 — 2026-10-27

- Older.
"""


def repository(cargo=CARGO, antora=ANTORA, changelog=CHANGELOG):
    root = tempfile.mkdtemp()
    os.makedirs(os.path.join(root, "src", "docs"))
    for path, text in [("Cargo.toml", cargo), (os.path.join("src", "docs", "antora.yml"), antora),
                       ("CHANGELOG.md", changelog)]:
        with open(os.path.join(root, path), "w", encoding="utf-8") as file:
            file.write(text)
    return root


class VersionCheck(unittest.TestCase):
    def test_a_release_whose_files_agree_passes(self):
        self.assertEqual(release_version.problems(repository(), "0.5.0"), [])

    def test_each_file_that_disagrees_is_named(self):
        root = repository(
            cargo=CARGO.replace('version = "0.5.0"', 'version = "0.4.0"'),
            antora=ANTORA.replace("'0.5.0'", "'0.4.0'"),
            changelog=CHANGELOG.replace("## 0.5.0 — 2026-11-02", "## Unreleased again"),
        )
        found = release_version.problems(root, "0.5.0")
        self.assertEqual(len(found), 3, found)
        self.assertTrue(any("Cargo.toml" in problem and "0.4.0" in problem for problem in found))
        self.assertTrue(any("antora.yml" in problem for problem in found))
        self.assertTrue(any("CHANGELOG.md" in problem for problem in found))

    def test_only_the_workspace_package_version_counts(self):
        # A dependency's version, or another table's, is not the release's.
        cargo = CARGO.replace('[workspace.package]\nversion = "0.5.0"', '[workspace.package]')
        cargo += '\n[package.metadata]\nversion = "0.5.0"\n'
        found = release_version.problems(repository(cargo=cargo), "0.5.0")
        self.assertEqual(len(found), 1, found)

    def test_a_version_that_only_starts_the_same_does_not_match(self):
        self.assertEqual(len(release_version.problems(repository(), "0.5")), 3)


class ReleaseNotes(unittest.TestCase):
    def test_the_versions_section_is_taken_without_its_heading(self):
        self.assertEqual(
            release_version.notes(CHANGELOG, "0.5.0"),
            "### Added\n\n- New Page (#72).\n\n### Fixed\n\n- A fix.\n",
        )

    def test_the_last_section_runs_to_the_end(self):
        self.assertEqual(release_version.notes(CHANGELOG, "0.4.0"), "- Older.\n")

    def test_a_missing_section_is_none(self):
        self.assertIsNone(release_version.notes(CHANGELOG, "9.9.9"))


if __name__ == "__main__":
    unittest.main()
