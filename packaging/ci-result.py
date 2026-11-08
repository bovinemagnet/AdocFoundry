#!/usr/bin/env python3
"""Whether CI has passed on a commit pushed to main, so a release of it need not run CI again.

usage: packaging/ci-result.py <owner/repository> <commit>

A release is tagged on a commit of main, whose own CI run is usually still going when the tag is
pushed. This waits while that run is queued or running, then writes `passed=true` to
$GITHUB_OUTPUT if a run of ci.yml for a push to main passed on the commit, else `passed=false`,
and the release runs CI itself. Only pushes to main count: a pull request's run tested its merge
with main, not the commit. The runs are read with `gh api`, which needs GH_TOKEN with
`actions: read`.
"""

import json
import os
import subprocess
import sys
import time

# How long to wait between asking; CI takes most of an hour.
POLL_SECONDS = 60


def verdict(runs):
    """`passed` when a push to main's run passed, `wait` while one is still going, else `run`."""
    mains = [run for run in runs if run["event"] == "push" and run["head_branch"] == "main"]
    if any(run["status"] == "completed" and run["conclusion"] == "success" for run in mains):
        return "passed"
    if any(run["status"] != "completed" for run in mains):
        return "wait"
    return "run"


def fetch(repository, sha):
    """ci.yml's runs for pushes of `sha`, newest first."""
    output = subprocess.run(
        [
            "gh",
            "api",
            f"repos/{repository}/actions/workflows/ci.yml/runs"
            f"?head_sha={sha}&event=push&per_page=100",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(output.stdout)["workflow_runs"]


def main(argv, fetch=fetch, sleep=time.sleep):
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    repository, sha = argv[1], argv[2]
    while True:
        found = verdict(fetch(repository, sha))
        if found != "wait":
            break
        print(f"CI on main for {sha} is still running; asking again in {POLL_SECONDS} s")
        sleep(POLL_SECONDS)
    passed = "true" if found == "passed" else "false"
    print(
        f"CI on main passed for {sha}; the release reuses it"
        if found == "passed"
        else f"No passed CI run on main for {sha}; the release runs CI"
    )
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"passed={passed}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
