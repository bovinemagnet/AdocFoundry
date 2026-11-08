"""Tests that a release is published only once CI has passed on the tagged commit, and only
when the tag matches the version the files give.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import os
import re
import tomllib
import unittest

WORKFLOWS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".github", "workflows")
NEXTEST = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".config", "nextest.toml")


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


def workflows():
    return sorted(name for name in os.listdir(WORKFLOWS) if name.endswith((".yml", ".yaml")))


class WorkflowsAreReproducible(unittest.TestCase):
    """A run uses the same actions and tools next month as today, and a hung job is stopped."""

    def test_every_job_has_a_time_limit(self):
        for name in workflows():
            for job, lines in jobs(read(name)).items():
                # A job that calls another workflow takes its jobs' limits.
                if any(re.match(r"^    uses:", line) for line in lines):
                    continue
                self.assertTrue(
                    any(re.match(r"^    timeout-minutes:\s*\d+\s*$", line) for line in lines),
                    f"{name}: job {job} has no timeout-minutes",
                )

    def test_actions_are_pinned_to_a_commit(self):
        for name in workflows():
            for number, line in enumerate(read(name).splitlines(), 1):
                match = re.match(r"^\s*(?:-\s+)?uses:\s*(\S+)(.*)$", line)
                if not match or match.group(1).startswith("./"):
                    continue
                self.assertRegex(
                    match.group(1),
                    r"^[\w.-]+/[\w./-]+@[0-9a-f]{40}$",
                    f"{name}:{number}: {match.group(1)} is not pinned to a commit",
                )
                self.assertRegex(
                    match.group(2),
                    r"#\s*v\d+(\.\d+)*",
                    f"{name}:{number}: the pinned commit has no version comment",
                )

    def test_tools_are_installed_at_pinned_versions(self):
        for name in workflows():
            text = read(name)
            # `--locked` alone pins a tool's dependencies, not the tool; with `--version` both
            # are fixed, and Cargo checks every crate against the registry's checksums. An
            # installer action falls back to unchecked downloads for tools it has no hash for.
            self.assertNotIn("taiki-e/install-action", text, f"{name}: install tools with cargo")
            for command in re.findall(r"cargo install\b[^\n]*", text):
                self.assertRegex(
                    command,
                    r"^cargo install [\w-]+ --version \d+\.\d+\.\d+ --locked$",
                    f"{name}: {command}",
                )

    def test_checkouts_keep_no_credentials(self):
        # Nothing pushes with git; the token stays out of .git/config for every later step.
        for name in workflows():
            lines = read(name).splitlines()
            for number, line in enumerate(lines):
                if re.match(r"^\s*-\s+uses:\s*actions/checkout@", line):
                    step = []
                    for following in lines[number + 1 :]:
                        if re.match(r"^\s*-\s", following) or not following.startswith(" " * 8):
                            break
                        step.append(following)
                    self.assertTrue(
                        any(re.match(r"^\s+persist-credentials:\s*false\s*$", s) for s in step),
                        f"{name}:{number + 1}: checkout keeps its credentials",
                    )

    def test_downloads_are_versioned_and_checked(self):
        for name in workflows():
            text = read(name)
            self.assertNotIn("/download/continuous/", text, f"{name} downloads a moving build")
        linux = "\n".join(jobs(read("release.yml"))["linux"])
        self.assertIn("sha256sum -c", linux, "linuxdeploy is checked before it runs")

    def test_a_superseded_pull_request_run_is_cancelled(self):
        ci = read("ci.yml").split("\njobs:\n", 1)[0]
        self.assertRegex(ci, r"(?m)^concurrency:")
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", ci)

    def test_a_hung_test_on_windows_is_stopped_and_named(self):
        # Windows runs have hung in a different test each time (#391). libtest waits for the
        # job's time limit and names no test; nextest stops one that outlives its own limit.
        test = "\n".join(jobs(read("ci.yml"))["test"])
        self.assertRegex(
            test,
            r"if: runner\.os == 'Windows'\n\s+run: cargo nextest run --workspace "
            r"--exclude adocfoundry --profile ci\n",
        )
        self.assertRegex(
            test,
            r"if: runner\.os != 'Windows'\n\s+run: cargo test --workspace "
            r"--exclude adocfoundry --no-fail-fast\n",
        )
        with open(NEXTEST, "rb") as config:
            profile = tomllib.load(config)["profile"]["ci"]
        timeout = profile["slow-timeout"]
        limit = int(timeout["period"].removesuffix("s")) * timeout["terminate-after"]
        # Long enough for the slowest test seen on Windows (about three minutes), and far
        # short of the job's two hours.
        self.assertGreaterEqual(limit, 6 * 60)
        self.assertLessEqual(limit, 15 * 60)
        self.assertIs(profile["fail-fast"], False, "every failure is reported")


class FuzzWorkflow(unittest.TestCase):
    """fuzz.yml beyond what WorkflowsAreReproducible checks of every workflow."""

    def test_it_reads_only(self):
        fuzz = read("fuzz.yml")
        self.assertRegex(fuzz.split("\njobs:\n", 1)[0], r"(?m)^permissions:\n  contents: read$")

    def test_fuzzing_runs_only_on_schedule_or_by_hand(self):
        fuzz = jobs(read("fuzz.yml"))
        run = "\n".join(fuzz["fuzz"])
        self.assertIn("-max_total_time=300", run)
        self.assertIn("github.event_name == 'schedule'", run)
        self.assertIn("if: failure()", run, "crashes are uploaded")
        build = "\n".join(fuzz["build"])
        self.assertIn("cargo +nightly fuzz build", build)
        self.assertNotIn("fuzz run", build)


if __name__ == "__main__":
    unittest.main()
