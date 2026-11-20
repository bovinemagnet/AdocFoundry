"""Checks of the macOS bundle's Info.plist template."""

import plistlib
import unittest
from pathlib import Path

PLIST = Path(__file__).parent / "macos" / "Info.plist"


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


if __name__ == "__main__":
    unittest.main()
