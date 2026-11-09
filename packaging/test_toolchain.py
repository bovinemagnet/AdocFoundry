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


FLATPAK = "packaging/flatpak/io.github.bovinemagnet.AdocFoundry.yml"


class FlatpakUsesThePinnedToolchain(unittest.TestCase):
    """The Flatpak builds with rust-toolchain.toml's release, not the runtime's rust-stable
    extension, which lags it (24.08's had 1.89, and 26.08's 1.98, when 1.99 was pinned)."""

    def channel(self):
        return re.search(r'(?m)^channel\s*=\s*"([^"]+)"', read("rust-toolchain.toml")).group(1)

    def test_the_manifest_builds_with_the_pinned_rust(self):
        manifest = read(FLATPAK)
        self.assertNotIn("org.freedesktop.Sdk.Extension.rust-stable", manifest)
        self.assertNotIn("/usr/lib/sdk/rust-stable", manifest)
        for arch in ["x86_64", "aarch64"]:
            source = re.search(
                r"- type: archive\n\s+only-arches: \[(\w+)\]\n\s+url: (\S+)\n\s+sha256: (\S+)\n",
                manifest[manifest.index(f"only-arches: [{arch}]") - 40 :],
            )
            self.assertIsNotNone(source, f"no Rust archive for {arch}")
            self.assertEqual(source.group(1), arch)
            self.assertEqual(
                source.group(2),
                f"https://static.rust-lang.org/dist/rust-{self.channel()}-{arch}-unknown-linux-gnu.tar.xz",
            )
            self.assertRegex(source.group(3), r"^[0-9a-f]{64}$")
        self.assertIn("append-path: /app/rust/bin", manifest)
        # The compiler builds the app and is left out of it.
        self.assertRegex(manifest, r"(?m)^    cleanup:\n      - /rust$")
        rust = manifest.index("  - name: rust\n")
        # Splitting the debug information out of a prebuilt libLLVM.so left it unloadable
        # ("object file has no loadable segments"), so flatpak-builder leaves the module's files
        # as the Rust project built them.
        module = manifest[rust : manifest.index("  - name: adocfoundry\n")]
        self.assertRegex(module, r"build-options:\n\s+no-debuginfo: true\n")
        self.assertLess(rust, manifest.index("  - name: adocfoundry\n"), "Rust is built first")

    def test_the_release_builds_in_the_manifests_runtime(self):
        runtime = re.search(r"(?m)^runtime-version: '([\d.]+)'$", read(FLATPAK)).group(1)
        release = read(".github/workflows/release.yml")
        job = release[release.index("\n  flatpak:\n") :]
        self.assertIn(f"# freedesktop-{runtime}, pinned to its digest.", job)


if __name__ == "__main__":
    unittest.main()
