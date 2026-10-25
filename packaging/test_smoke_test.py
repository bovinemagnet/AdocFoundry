"""Tests of the smoke test's signature checks, which otherwise run only on a signed release.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import importlib.util
import os
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


if __name__ == "__main__":
    unittest.main()
