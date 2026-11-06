#!/usr/bin/env python3
"""The size of each file a release ships, against the design's budget.

usage: packaging/binary-sizes.py <budget-MB> <file>...

Writes a Markdown table of the files' sizes to standard output and, in a GitHub Actions job, to
the job's summary ($GITHUB_STEP_SUMMARY). A file over the budget gets a warning annotation; the
budget is soft, so the exit status is 0 whenever every file could be read.
"""

import os
import sys

MB = 1024 * 1024


def table(sizes, budget_mb):
    """The Markdown table for `sizes`, (name, bytes) pairs, against `budget_mb`."""
    lines = [
        f"### Sizes (budget {budget_mb} MB)",
        "",
        "| File | MB | |",
        "|---|---:|---|",
    ]
    for name, size in sizes:
        over = "over budget" if size > budget_mb * MB else ""
        lines.append(f"| `{name}` | {size / MB:.1f} | {over} |")
    return "\n".join(lines) + "\n"


def warnings(sizes, budget_mb):
    """A workflow warning command for each file over `budget_mb`."""
    return [
        f"::warning title=Size budget::{name} is {size / MB:.1f} MB, over the {budget_mb} MB budget"
        for name, size in sizes
        if size > budget_mb * MB
    ]


def main(argv):
    if len(argv) < 3:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    budget_mb = float(argv[1])
    if budget_mb.is_integer():
        budget_mb = int(budget_mb)
    sizes = [(path, os.path.getsize(path)) for path in argv[2:]]
    text = table(sizes, budget_mb)
    sys.stdout.write(text)
    for warning in warnings(sizes, budget_mb):
        print(warning)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as file:
            file.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
