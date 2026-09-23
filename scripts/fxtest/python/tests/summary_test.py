# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import tempfile
import unittest

from summary import BuildResult
from summary import parse_suggestions_from_output
from summary import RunSummary
from summary import Suggestion
from summary import TestResult


class TestSummary(unittest.TestCase):
    def test_run_summary_passed(self) -> None:
        summary = RunSummary()
        summary.build = BuildResult(status="PASSED")
        summary.add_test(
            TestResult(
                name="host_x64/test1",
                type="host",
                outcome="PASSED",
                log_path="/tmp/logs/test1.stdout.log",
                stdout_log_path="/tmp/logs/test1.stdout.log",
                stderr_log_path="/tmp/logs/test1.stderr.log",
                duration_seconds=1.5,
            )
        )
        summary.finalize()

        self.assertEqual(summary.outcome, "PASSED")
        self.assertEqual(summary.total_passed, 1)
        self.assertEqual(summary.total_failed, 0)
        self.assertEqual(summary.total_skipped, 0)

        data = summary.to_dict()
        self.assertEqual(data["outcome"], "PASSED")
        self.assertEqual(len(data["tests"]), 1)
        self.assertEqual(data["tests"][0]["name"], "host_x64/test1")
        self.assertEqual(data["tests"][0]["outcome"], "PASSED")
        self.assertEqual(
            data["tests"][0]["log_path"], "/tmp/logs/test1.stdout.log"
        )
        self.assertEqual(
            data["tests"][0]["stdout_log_path"], "/tmp/logs/test1.stdout.log"
        )
        self.assertEqual(
            data["tests"][0]["stderr_log_path"], "/tmp/logs/test1.stderr.log"
        )

    def test_run_summary_failed_test(self) -> None:
        summary = RunSummary()
        summary.build = BuildResult(status="PASSED")
        summary.add_test(
            TestResult(
                name="host_x64/test1",
                type="host",
                outcome="PASSED",
                log_path="/tmp/logs/test1.log",
            )
        )
        summary.add_test(
            TestResult(
                name="fuchsia-pkg://fuchsia.com/test2#meta/test2.cm",
                type="device",
                outcome="FAILED",
                log_path="/tmp/logs/test2.log",
                message="Assertion failed",
            )
        )
        summary.finalize()

        self.assertEqual(summary.outcome, "FAILED")
        self.assertEqual(summary.total_passed, 1)
        self.assertEqual(summary.total_failed, 1)
        self.assertEqual(summary.total_skipped, 0)

    def test_run_summary_build_failed(self) -> None:
        summary = RunSummary()
        summary.build = BuildResult(
            status="FAILED",
            log_path="/tmp/logs/build.log",
            error="Compiler error in file.cc",
        )
        summary.finalize()

        self.assertEqual(summary.outcome, "BUILD_FAILED")
        self.assertEqual(summary.total_passed, 0)
        self.assertEqual(summary.total_failed, 0)

        data = summary.to_dict()
        self.assertEqual(data["outcome"], "BUILD_FAILED")
        self.assertIsNotNone(data.get("build"))
        self.assertEqual(data["build"]["status"], "FAILED")
        self.assertEqual(data["build"]["log_path"], "/tmp/logs/build.log")

    def test_write_to_file(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            target_path = os.path.join(td, "sub", "summary.json")
            summary = RunSummary()
            summary.add_test(
                TestResult(
                    name="host_x64/test1",
                    type="host",
                    outcome="PASSED",
                )
            )
            summary.finalize()
            summary.write_to_file(target_path)

            self.assertTrue(os.path.exists(target_path))
            with open(target_path, "r", encoding="utf-8") as f:
                loaded = json.load(f)
            self.assertEqual(loaded["outcome"], "PASSED")
            self.assertEqual(
                loaded["summary_path"], os.path.abspath(target_path)
            )

    def test_run_summary_with_structured_suggestions(self) -> None:
        summary = RunSummary()
        summary.suggestions = [
            Suggestion(
                name="summary_test",
                similarity=0.8889,
                build_includes="host_x64/obj/scripts/fxtest/python/summary_test.sh",
            ),
            Suggestion(
                name="system-updater-tests",
                similarity=0.7698,
                add_test_command="fx add-test //src/sys/pkg/bin/system-updater:system-updater-tests",
            ),
        ]
        summary.finalize(error="No tests found matching criteria")

        self.assertEqual(summary.outcome, "ERROR")
        data = summary.to_dict()
        self.assertEqual(data["outcome"], "ERROR")
        self.assertEqual(data["error"], "No tests found matching criteria")
        self.assertIn("suggestions", data)
        self.assertEqual(len(data["suggestions"]), 2)
        self.assertEqual(data["suggestions"][0]["name"], "summary_test")
        self.assertEqual(data["suggestions"][0]["similarity"], 0.8889)
        self.assertEqual(
            data["suggestions"][0]["build_includes"],
            "host_x64/obj/scripts/fxtest/python/summary_test.sh",
        )
        self.assertEqual(
            data["suggestions"][1]["add_test_command"],
            "fx add-test //src/sys/pkg/bin/system-updater:system-updater-tests",
        )

    def test_run_summary_hints(self) -> None:
        summary = RunSummary()
        summary.hints = [
            "To debug with fx debug cli: fx test --agent-debugging-mode host_x64/test1"
        ]
        summary.finalize()

        data = summary.to_dict()
        self.assertIn("hints", data)
        self.assertEqual(len(data["hints"]), 1)
        self.assertEqual(
            data["hints"][0],
            "To debug with fx debug cli: fx test --agent-debugging-mode host_x64/test1",
        )

        md = summary.to_markdown()
        self.assertIn("## Hints", md)
        self.assertIn(
            "- To debug with fx debug cli: fx test --agent-debugging-mode host_x64/test1",
            md,
        )

    def test_parse_suggestions_from_output(self) -> None:
        raw_output = """summary_test (88.89% similar)
Build includes: host_x64/obj/scripts/fxtest/python/summary_test.sh
system-update-committer-tests (77.66% similar)
fx add-test //src/sys/pkg/bin/system-update-committer:system-update-committer-tests
(6 more matches not shown)"""

        suggestions = parse_suggestions_from_output(raw_output)
        self.assertEqual(len(suggestions), 2)
        self.assertEqual(suggestions[0].name, "summary_test")
        self.assertEqual(suggestions[0].similarity, 0.8889)
        self.assertEqual(
            suggestions[0].build_includes,
            "host_x64/obj/scripts/fxtest/python/summary_test.sh",
        )
        self.assertEqual(suggestions[1].name, "system-update-committer-tests")
        self.assertEqual(suggestions[1].similarity, 0.7766)
        self.assertEqual(
            suggestions[1].add_test_command,
            "fx add-test //src/sys/pkg/bin/system-update-committer:system-update-committer-tests",
        )

    def test_parse_suggestions_with_add_host_test(self) -> None:
        raw_output = """my_host_test (85.00% similar)
fx add-host-test //scripts/fxtest:tests
(2 more matches not shown)"""
        suggestions = parse_suggestions_from_output(raw_output)
        self.assertEqual(len(suggestions), 1)
        self.assertEqual(suggestions[0].name, "my_host_test")
        self.assertEqual(suggestions[0].similarity, 0.85)
        self.assertEqual(
            suggestions[0].add_test_command,
            "fx add-host-test //scripts/fxtest:tests",
        )


if __name__ == "__main__":
    unittest.main()
