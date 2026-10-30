"""Tests of the smoke test's signature checks, which otherwise run only on a signed release, and
of where it finds the dictionaries' licence in each package.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import importlib.util
import os
import shutil
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("smoke_test", os.path.join(HERE, "smoke-test.py"))
smoke_test = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke_test)

APP = os.path.join("Volumes", "AdocFoundry", "AdocFoundry.app", "Contents", "MacOS")


def runner(answers):
    """A command runner answering by the command's first two words, and recording each call."""
    calls = []

    def run(command):
        calls.append(command)
        return answers.get(tuple(command[:2]), (0, ""))
    run.calls = calls
    return run


class SignatureChecks(unittest.TestCase):
    def test_signature_checks_report_unsigned_builds(self):
        # Ad hoc: codesign verifies, but there is no Developer ID or runtime, and Gatekeeper
        # rejects the bundle.
        run = runner({
            ("codesign", "--display"): (0, "Signature=adhoc\nflags=0x2(adhoc)"),
            ("spctl", "--assess"): (3, "rejected"),
        })
        problems = smoke_test.signature_problems(APP, "darwin", run)
        self.assertEqual(len(problems), 2 * len(smoke_test.BINARIES) + 1, problems)
        self.assertIn("rejected", problems[-1])
        self.assertTrue(run.calls[-1][-1].endswith("AdocFoundry.app"), run.calls[-1])

        unsigned = runner({("powershell", "-NoProfile"): (0, "NotSigned\n")})
        problems = smoke_test.signature_problems("bin", "win32", unsigned)
        self.assertEqual(len(problems), len(smoke_test.BINARIES), problems)
        self.assertIn("NotSigned", problems[0])

    def test_a_signed_build_has_no_problems(self):
        run = runner({
            ("codesign", "--display"): (0, "Authority=Developer ID Application: Someone (TEAM)\n"
                                           "flags=0x10000(runtime)"),
        })
        self.assertEqual(smoke_test.signature_problems(APP, "darwin", run), [])
        signed = runner({("powershell", "-NoProfile"): (0, "Valid\r\n")})
        self.assertEqual(smoke_test.signature_problems("bin", "win32", signed), [])

    def test_linux_has_no_signature_check(self):
        self.assertEqual(len(smoke_test.signature_problems("bin", "linux", runner({}))), 1)


class LicenceChecks(unittest.TestCase):
    def package(self, binaries, licence):
        """A temporary package with its binaries' folder at `binaries` and, unless None, the
        licence at `licence`, both relative to its root: the root."""
        root = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, root)
        os.makedirs(os.path.join(root, binaries))
        if licence:
            path = os.path.join(root, licence)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as file:
                file.write("Copyright 2000-2026 by Kevin Atkinson\n")
        return root

    def test_the_licence_is_found_in_each_package(self):
        packages = {
            "Windows zip": ("unpacked", "unpacked/licenses/SCOWL.txt"),
            "macOS app": ("AdocFoundry.app/Contents/MacOS",
                          "AdocFoundry.app/Contents/Resources/licenses/SCOWL.txt"),
            "deb, rpm, AppImage": ("usr/bin", "usr/share/doc/adocfoundry/SCOWL.txt"),
        }
        for package, (binaries, licence) in packages.items():
            root = self.package(binaries, licence)
            path = smoke_test.licence_path(os.path.join(root, binaries))
            self.assertIsNotNone(path, package)
            self.assertTrue(os.path.samefile(path, os.path.join(root, licence)), package)

    def test_a_package_without_the_licence_is_reported(self):
        root = self.package("usr/bin", None)
        self.assertIsNone(smoke_test.licence_path(os.path.join(root, "usr/bin")))

if __name__ == "__main__":
    unittest.main()
