"""Tests that a release is published only once CI has passed on the tagged commit, and only
when the tag matches the version the files give.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import os
import re
import unittest

WORKFLOWS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".github", "workflows")


def read(name):
    with open(os.path.join(WORKFLOWS, name), encoding="utf-8") as workflow:
        return workflow.read()


def jobs(text):
    """Each job's lines, by name, read from the indentation under `jobs:`."""
    found = {}
    body = text.split("\njobs:\n", 1)[1]
    name = None
    for line in body.splitlines():
        heading = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if heading:
            name = heading.group(1)
            found[name] = []
        elif name and (line.startswith("    ") or not line.strip()):
            found[name].append(line)
        elif line.strip() and not line.startswith(" "):
            break
    return found


def needs(lines):
    for line in lines:
        match = re.match(r"^    needs:\s*\[(.*)\]\s*$", line)
        if match:
            return [need.strip() for need in match.group(1).split(",")]
        match = re.match(r"^    needs:\s*(\S+)\s*$", line)
        if match:
            return [match.group(1)]
    return []


class ReleaseWaitsForCi(unittest.TestCase):
    def test_the_release_runs_ci_on_the_tagged_commit(self):
        release = jobs(read("release.yml"))
        callers = [
            name
            for name, lines in release.items()
            if any(re.match(r"^    uses:\s*\./\.github/workflows/ci\.yml\s*$", line) for line in lines)
        ]
        self.assertEqual(len(callers), 1, "one job of release.yml runs ci.yml")
        self.assertIn(callers[0], needs(release["publish"]), "publish waits for CI")
        self.assertIn("smoke", needs(release["publish"]), "publish still waits for the smoke tests")

    def test_tag_check_precedes_build(self):
        release = jobs(read("release.yml"))
        checks = [
            name
            for name, lines in release.items()
            if any("packaging/release-version.py check" in line for line in lines)
        ]
        self.assertEqual(checks, ["version"], "one job checks the tag against the files")
        check = "\n".join(release["version"])
        self.assertIn('"${GITHUB_REF_NAME#v}"', check, "the tag's version is the one checked")
        self.assertIn("startsWith(github.ref, 'refs/tags/')", check, "only a tag is checked")
        # Nothing is built, tested or published until the tag has been checked.
        for name in ["ci", "macos", "windows", "linux"]:
            self.assertIn("version", needs(release[name]), f"{name} waits for the version check")

    def test_the_release_is_published_with_its_changelog_section(self):
        publish = "\n".join(jobs(read("release.yml"))["publish"])
        self.assertIn('packaging/release-version.py notes "${GITHUB_REF_NAME#v}"', publish)
        self.assertRegex(publish, r"body_path:\s*\S+")

    def test_the_smoke_test_expects_the_tags_version(self):
        smoke = "\n".join(jobs(read("release.yml"))["smoke"])
        self.assertIn("--expect-version", smoke)

    def test_ci_can_be_called_and_does_not_also_run_on_tags(self):
        ci = read("ci.yml")
        triggers = ci.split("\njobs:\n", 1)[0]
        self.assertRegex(triggers, r"(?m)^  workflow_call:", "release.yml can call ci.yml")
        # A tag runs CI once, through the release.
        self.assertNotRegex(triggers, r"(?m)^\s+tags:")


if __name__ == "__main__":
    unittest.main()
