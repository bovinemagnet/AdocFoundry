"""Tests of packaging/ci-result.py, which tells the release whether CI already passed on main.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import importlib.util
import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("ci_result", os.path.join(HERE, "ci-result.py"))
ci_result = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci_result)


def run(status="completed", conclusion="success", event="push", branch="main"):
    return {"status": status, "conclusion": conclusion, "event": event, "head_branch": branch}


class Verdict(unittest.TestCase):
    def test_a_passed_push_to_main_is_reused(self):
        self.assertEqual(ci_result.verdict([run()]), "passed")
        # A failed attempt beside it does not matter once one has passed.
        self.assertEqual(ci_result.verdict([run(conclusion="failure"), run()]), "passed")

    def test_a_run_still_going_is_waited_for(self):
        for status in ["queued", "in_progress", "waiting", "pending", "requested"]:
            self.assertEqual(ci_result.verdict([run(status=status, conclusion=None)]), "wait")

    def test_otherwise_ci_runs_in_the_release(self):
        # No run: a commit changing only documentation, which CI's paths filters skip.
        self.assertEqual(ci_result.verdict([]), "run")
        for conclusion in ["failure", "cancelled", "timed_out", "skipped"]:
            self.assertEqual(ci_result.verdict([run(conclusion=conclusion)]), "run", conclusion)

    def test_only_pushes_to_main_count(self):
        # A pull request's run tested its merge with main, not this commit; another branch's
        # run is no part of the release's history.
        self.assertEqual(ci_result.verdict([run(event="pull_request")]), "run")
        self.assertEqual(ci_result.verdict([run(branch="topic")]), "run")
        self.assertEqual(
            ci_result.verdict([run(event="pull_request", status="in_progress", conclusion=None)]),
            "run",
        )


class Main(unittest.TestCase):
    def outcome(self, answers):
        """What main writes to GITHUB_OUTPUT, given the runs listed on each call, and its sleeps."""
        asked, slept = [], []

        def fetch(repository, sha):
            asked.append((repository, sha))
            return answers[min(len(asked), len(answers)) - 1]

        with tempfile.TemporaryDirectory() as folder:
            output = os.path.join(folder, "output")
            os.environ["GITHUB_OUTPUT"] = output
            try:
                with redirect_stdout(io.StringIO()):
                    status = ci_result.main(
                        ["ci-result.py", "owner/repo", "abc123"], fetch=fetch, sleep=slept.append
                    )
            finally:
                del os.environ["GITHUB_OUTPUT"]
            with open(output, encoding="utf-8") as file:
                written = file.read()
        self.assertEqual(status, 0)
        self.assertTrue(all(call == ("owner/repo", "abc123") for call in asked))
        return written, len(asked), slept

    def test_it_waits_for_mains_run_and_then_reuses_it(self):
        going = [run(status="in_progress", conclusion=None)]
        written, calls, slept = self.outcome([going, going, [run()]])
        self.assertEqual(written, "passed=true\n")
        self.assertEqual(calls, 3)
        self.assertEqual(slept, [ci_result.POLL_SECONDS] * 2)

    def test_without_a_passed_run_ci_runs_in_the_release(self):
        written, calls, slept = self.outcome([[run(conclusion="failure")]])
        self.assertEqual(written, "passed=false\n")
        self.assertEqual((calls, slept), (1, []))


if __name__ == "__main__":
    unittest.main()
