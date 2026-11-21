"""Checks of the packages' manifests: the macOS bundle's Info.plist template and the Flatpak's."""

import plistlib
import unittest
from pathlib import Path

PLIST = Path(__file__).parent / "macos" / "Info.plist"
FLATPAK = Path(__file__).parent / "flatpak" / "io.github.bovinemagnet.AdocFoundry.yml"


class Bundle(unittest.TestCase):
    def setUp(self):
        text = PLIST.read_text().replace("@VERSION@", "0.0.0")
        self.plist = plistlib.loads(text.encode())

    def test_the_bundle_explains_local_network_use(self):
        """Sharing a workspace and joining a session reach the local network, which macOS asks
        about first: the question says why, and only when."""
        reason = self.plist.get("NSLocalNetworkUsageDescription", "")
        self.assertIn("share a workspace", reason)
        self.assertIn("join a session", reason)


class Flatpak(unittest.TestCase):
    def finish_args(self):
        args, inside = [], False
        for line in FLATPAK.read_text().splitlines():
            if line.startswith("finish-args:"):
                inside = True
            elif inside and line.startswith("  - "):
                args.append(line[4:].strip())
            elif inside and line and not line.startswith(" "):
                break
        return args

    def test_the_flatpak_may_reach_the_network_for_collaboration(self):
        """Sharing a workspace listens and joining a session connects, so the app needs the
        network; nothing listens or connects until the person chooses to."""
        self.assertIn("--share=network", self.finish_args())


if __name__ == "__main__":
    unittest.main()
