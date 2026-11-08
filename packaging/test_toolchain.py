"""Tests that the toolchain is pinned, and that Cargo.toml's `rust-version` names the same release.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import glob
import os
import re
import tomllib
import unittest

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")


def read(path):
    with open(os.path.join(ROOT, path), encoding="utf-8") as file:
        return file.read()


class ToolchainIsPinned(unittest.TestCase):
    def channel(self):
        match = re.search(r'(?m)^channel\s*=\s*"([^"]+)"', read("rust-toolchain.toml"))
        self.assertIsNotNone(match, "rust-toolchain.toml names a channel")
        return match.group(1)

    def test_the_channel_is_a_release(self):
        # `stable` would build each release with whatever compiler was newest that day.
        self.assertRegex(self.channel(), r"^\d+\.\d+\.\d+$")

    def test_rust_version_is_the_pinned_release(self):
        match = re.search(r'(?m)^rust-version\s*=\s*"([^"]+)"', read("Cargo.toml"))
        self.assertIsNotNone(match, "[workspace.package] sets rust-version")
        major_minor = ".".join(self.channel().split(".")[:2])
        self.assertEqual(match.group(1), major_minor)

    def test_every_crate_takes_the_workspace_rust_version(self):
        manifests = glob.glob(os.path.join(ROOT, "crates", "*", "Cargo.toml"))
        manifests.append(os.path.join(ROOT, "xtask", "Cargo.toml"))
        self.assertTrue(manifests)
        for manifest in manifests:
            with open(manifest, encoding="utf-8") as file:
                self.assertRegex(
                    file.read(),
                    r"(?m)^rust-version\.workspace\s*=\s*true$",
                    os.path.relpath(manifest, ROOT),
                )


class TestBuildsAreLean(unittest.TestCase):
    def test_debug_builds_keep_line_tables_only(self):
        # Writing and linking full debug information costs build time, most of all on CI's
        # Windows runner (#391). Backtraces still name files and lines.
        with open(os.path.join(ROOT, "Cargo.toml"), "rb") as file:
            profiles = tomllib.load(file)["profile"]
        self.assertEqual(profiles.get("dev", {}).get("debug"), "line-tables-only")
        # Tests build with the dev profile, so nothing may set it back for them.
        self.assertNotIn("debug", profiles.get("test", {}))


if __name__ == "__main__":
    unittest.main()
