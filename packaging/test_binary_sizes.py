"""Tests of packaging/binary-sizes.py, the release jobs' size report.

Run with: python3 -m unittest discover -s packaging -p 'test_*.py'
"""

import importlib.util
import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("binary_sizes", os.path.join(HERE, "binary-sizes.py"))
sizes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sizes)

MB = 1024 * 1024


class SizeReport(unittest.TestCase):
    def test_a_file_over_the_budget_is_flagged_and_warned_of(self):
        report = [("adocfoundry", 41 * MB), ("adoc-ls", 39 * MB)]
        table = sizes.table(report, 40)
        self.assertIn("| `adocfoundry` | 41.0 | over budget |", table)
        self.assertIn("| `adoc-ls` | 39.0 |  |", table)
        self.assertEqual(
            sizes.warnings(report, 40),
            ["::warning title=Size budget::adocfoundry is 41.0 MB, over the 40 MB budget"],
        )

    def test_the_summary_gets_the_table_and_the_exit_status_stays_zero(self):
        with tempfile.TemporaryDirectory() as folder:
            binary = os.path.join(folder, "adocfoundry")
            with open(binary, "wb") as file:
                file.truncate(2 * MB)
            summary = os.path.join(folder, "summary.md")
            os.environ["GITHUB_STEP_SUMMARY"] = summary
            try:
                with redirect_stdout(io.StringIO()) as out:
                    status = sizes.main(["binary-sizes.py", "1", binary])
            finally:
                del os.environ["GITHUB_STEP_SUMMARY"]
            self.assertEqual(status, 0, "the budget is soft")
            self.assertIn("::warning", out.getvalue())
            with open(summary, encoding="utf-8") as file:
                self.assertIn(f"| `{binary}` | 2.0 | over budget |", file.read())


if __name__ == "__main__":
    unittest.main()
