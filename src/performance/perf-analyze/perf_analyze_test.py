# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the perf-analyze tool."""

import importlib.resources
import io
import json
import unittest
from contextlib import redirect_stderr, redirect_stdout
from typing import Sequence

from perf_analyze import main
from plugins import AnalyzePlugin, PluginArgumentError, SectionResult


class MockAnalyzePlugin(AnalyzePlugin):
    """Mock implementation of AnalyzePlugin for unit testing success paths."""

    name: str = "mock-analyze"
    description: str = "Mock analyze plugin"

    def analyze(
        self,
        remaining_args: Sequence[str],
        trace_path: str,
        cache: bool = True,
    ) -> Sequence[SectionResult]:
        """Mock analysis execution."""
        if "--help" in remaining_args:
            print("mock-analyze help")
            return []
        return [
            SectionResult(
                name="mock_result", results=[{"analyze_result": "ok"}]
            )
        ]


class MockErrorPlugin(AnalyzePlugin):
    """Mock implementation of AnalyzePlugin for unit testing error paths."""

    name: str = "mock-error"
    description: str = "Mock error plugin"

    def analyze(
        self,
        remaining_args: Sequence[str],
        trace_path: str,
        cache: bool = True,
    ) -> Sequence[SectionResult]:
        """Mock analysis execution raising errors."""
        if "--invalid" in remaining_args:
            raise PluginArgumentError("Invalid argument")
        raise RuntimeError("Some other error")


class PerfAnalyzeTest(unittest.TestCase):
    """Tests for the perf-analyze command line tool."""

    def test_query_sql_text(self) -> None:
        """Tests executing a simple SQL query in default text format."""
        source = importlib.resources.files("test_data").joinpath(
            "sample_fxt.fxt"
        )
        with importlib.resources.as_file(source) as trace_path:
            stdout = io.StringIO()
            with redirect_stdout(stdout):
                exit_code = main(
                    [
                        "query",
                        "--trace",
                        str(trace_path),
                        "--sql",
                        "select count(*) as cnt from slice",
                    ]
                )
            self.assertEqual(exit_code, 0)
            output = stdout.getvalue().strip()
            self.assertEqual(output, "cnt\n520")

    def test_query_sql_error(self) -> None:
        """Tests that query errors are formatted correctly."""
        source = importlib.resources.files("test_data").joinpath(
            "sample_fxt.fxt"
        )
        with importlib.resources.as_file(source) as trace_path:
            stdout = io.StringIO()
            stderr = io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                exit_code = main(
                    [
                        "--format",
                        "text",
                        "query",
                        "--trace",
                        str(trace_path),
                        "--sql",
                        "select invalid-sql from slice",
                    ]
                )
            self.assertNotEqual(exit_code, 0)
            output = stdout.getvalue().strip()
            self.assertTrue(
                output.startswith("Error: Traceback"), f"Output was: {output!r}"
            )
            self.assertIn("no such column: invalid", output)

    def test_batch_query(self) -> None:
        """Tests executing a batch of queries from a file."""
        trace_source = importlib.resources.files("test_data").joinpath(
            "sample_fxt.fxt"
        )
        queries_source = importlib.resources.files("test_data").joinpath(
            "sample_queries.json"
        )

        with importlib.resources.as_file(
            trace_source
        ) as trace_path, importlib.resources.as_file(
            queries_source
        ) as queries_path:
            stdout = io.StringIO()
            with redirect_stdout(stdout):
                exit_code = main(
                    [
                        "--format",
                        "json",
                        "query",
                        "--trace",
                        str(trace_path),
                        "--batch",
                        f"@{queries_path}",
                    ]
                )
            self.assertEqual(exit_code, 0)
            results = json.loads(stdout.getvalue())

            expected = [
                {"name": "slice_count", "results": [{"cnt": 520}]},
                {"name": "process_count", "results": [{"cnt": 2}]},
            ]
            self.assertEqual(results, expected)

    def test_query_help(self) -> None:
        """Tests that query --help displays query help."""
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            exit_code = main(["query", "--help"])
        self.assertEqual(exit_code, 0)
        output = stdout.getvalue()
        self.assertIn("--trace", output)
        self.assertIn("--sql", output)
        self.assertIn("--batch", output)

    def test_query_empty_results_text(self) -> None:
        """Tests that queries with no results format as 'No results.'"""
        source = importlib.resources.files("test_data").joinpath(
            "sample_fxt.fxt"
        )
        with importlib.resources.as_file(source) as trace_path:
            stdout = io.StringIO()
            with redirect_stdout(stdout):
                exit_code = main(
                    [
                        "query",
                        "--trace",
                        str(trace_path),
                        "--sql",
                        "select * from slice where 1=0",
                    ]
                )
            self.assertEqual(exit_code, 0)
            output = stdout.getvalue().strip()
            self.assertEqual(output, "No results.")

    def test_list_plugins(self) -> None:
        """Tests that analyze --list-plugins lists registered plugins."""
        plugins = [MockAnalyzePlugin()]
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            exit_code = main(["analyze", "--list-plugins"], plugins=plugins)
        self.assertEqual(exit_code, 0)
        output = stdout.getvalue().strip()
        self.assertIn("mock-analyze\tMock analyze plugin", output)

    def test_analyze_plugin_routing(self) -> None:
        """Tests routing and execution of an analyze plugin."""
        plugins = [MockAnalyzePlugin()]
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            exit_code = main(
                [
                    "--format",
                    "json",
                    "analyze",
                    "--trace",
                    "dummy_trace",
                    "--plugin",
                    "mock-analyze",
                ],
                plugins=plugins,
            )
        self.assertEqual(exit_code, 0)
        results = json.loads(stdout.getvalue())
        self.assertEqual(
            results,
            [{"name": "mock_result", "results": [{"analyze_result": "ok"}]}],
        )

    def test_analyze_plugin_argument_error(self) -> None:
        """Tests that PluginArgumentError from plugin returns exit code 2."""
        plugins = [MockErrorPlugin()]
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            exit_code = main(
                [
                    "analyze",
                    "--trace",
                    "dummy_trace",
                    "--plugin",
                    "mock-error",
                    "--invalid",
                ],
                plugins=plugins,
            )
        self.assertEqual(exit_code, 2)

    def test_analyze_plugin_generic_error(self) -> None:
        """Tests that generic Exception from plugin returns exit code 1."""
        plugins = [MockErrorPlugin()]
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            exit_code = main(
                [
                    "analyze",
                    "--trace",
                    "dummy_trace",
                    "--plugin",
                    "mock-error",
                ],
                plugins=plugins,
            )
        self.assertEqual(exit_code, 1)

    def test_query_unrecognized_arguments(self) -> None:
        """Tests that query with unknown flags returns exit code 2."""
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            exit_code = main(
                [
                    "query",
                    "--trace",
                    "dummy_trace",
                    "--sql",
                    "select 1",
                    "--unknown-flag",
                ]
            )
        self.assertEqual(exit_code, 2)
        self.assertIn(
            "unrecognized arguments: --unknown-flag", stderr.getvalue()
        )

    def test_analyze_missing_required_arguments(self) -> None:
        """Tests that analyze without --trace and --plugin returns exit code 2."""
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            exit_code = main(["analyze"])
        self.assertEqual(exit_code, 2)
        self.assertIn(
            "the following arguments are required: --trace, --plugin",
            stderr.getvalue(),
        )

    def test_analyze_plugin_help(self) -> None:
        """Tests that analyze --plugin <name> --help dispatches help to plugin."""
        plugins = [MockAnalyzePlugin()]
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            exit_code = main(
                ["analyze", "--plugin", "mock-analyze", "--help"],
                plugins=plugins,
            )
        self.assertEqual(exit_code, 0)
        self.assertIn("mock-analyze help", stdout.getvalue())

    def test_analyze_unknown_plugin(self) -> None:
        """Tests that analyze --plugin unknown returns exit code 2."""
        plugins = [MockAnalyzePlugin()]
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            exit_code = main(
                [
                    "analyze",
                    "--trace",
                    "dummy_trace",
                    "--plugin",
                    "non-existent-plugin",
                ],
                plugins=plugins,
            )
        self.assertEqual(exit_code, 2)
        self.assertIn(
            "Plugin 'non-existent-plugin' not found", stderr.getvalue()
        )


if __name__ == "__main__":
    unittest.main()
