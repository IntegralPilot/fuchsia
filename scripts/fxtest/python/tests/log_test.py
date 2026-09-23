# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import contextlib
import dataclasses
import fcntl
import gzip
import io
import json
import os
import tempfile
import unittest
import unittest.mock as mock

import event
import log


class TestLogOutput(unittest.IsolatedAsyncioTestCase):
    async def _write_test_logs(self) -> io.StringIO:
        """Write out test logs

        Returns:
            io.StringIO: A buffer containing test logs.
        """
        recorder = event.EventRecorder()
        output = io.StringIO()
        log_task = asyncio.create_task(log.writer(recorder, output))
        recorder.emit_init()
        id = recorder.emit_build_start(["//test"])
        recorder.emit_end(id=id)
        recorder.emit_info_message("Done testing")
        recorder.emit_end()

        await log_task

        return output

    async def test_logs_json(self) -> None:
        """Test that logs are properly serialized to JSON."""
        output = await self._write_test_logs()

        events: list[event.Event] = [
            event.Event.from_dict(json.loads(line))  # type:ignore
            for line in output.getvalue().splitlines()
        ]

        self.assertEqual(len(events), 5)
        payloads = [e.payload for e in events if e.payload is not None]
        self.assertEqual(len(payloads), 3)
        self.assertIsNotNone(payloads[0].start_timestamp)
        self.assertEqual(payloads[1].build_targets, ["//test"])
        self.assertEqual(
            payloads[2].user_message,
            event.Message("Done testing", event.MessageLevel.INFO),
        )

    async def test_pretty_print(self) -> None:
        output = await self._write_test_logs()
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            log.pretty_print(log.LogSource.from_stream(output))
        self.assertEqual(stdout.getvalue(), "0 tests were run\n")

    async def test_pretty_print_without_root_event(self) -> None:
        """Test pretty_print when root event 0 is missing."""
        output = io.StringIO()
        recorder = event.EventRecorder()
        log_task = asyncio.create_task(log.writer(recorder, output))
        suite_id = recorder.emit_test_suite_started("my_suite", False)
        recorder.emit_test_suite_ended(
            suite_id, event.TestSuiteStatus.PASSED, None
        )
        recorder.emit_end()
        await log_task

        output.seek(0)
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            log.pretty_print(log.LogSource.from_stream(output))
        self.assertIn("1 tests were run", stdout.getvalue())

    async def test_read_log_with_filter(self) -> None:
        """Test that read_log respects an event_filter."""
        output = await self._write_test_logs()
        output.seek(0)
        source = log.LogSource.from_stream(output)
        elements = list(
            source.read_log(
                event_filter=lambda d: isinstance(d.get("payload"), dict)
                and "build_targets" in d["payload"]
            )
        )
        self.assertEqual(len(elements), 1)
        log_event = elements[0].log_event
        self.assertIsNotNone(log_event)
        assert log_event is not None
        payload = log_event.payload
        self.assertIsNotNone(payload)
        assert payload is not None
        self.assertEqual(payload.build_targets, ["//test"])

    async def test_read_log_with_line_filter(self) -> None:
        """Test that read_log respects a line_filter."""
        output = await self._write_test_logs()
        output.seek(0)
        source = log.LogSource.from_stream(output)
        elements = list(
            source.read_log(
                line_filter=lambda l: '"build_targets"' in l,
            )
        )
        self.assertEqual(len(elements), 1)
        log_event = elements[0].log_event
        self.assertIsNotNone(log_event)
        assert log_event is not None
        payload = log_event.payload
        self.assertIsNotNone(payload)
        assert payload is not None
        self.assertEqual(payload.build_targets, ["//test"])


class TestPreviousStats(unittest.IsolatedAsyncioTestCase):
    async def _write_test_logs(self) -> io.StringIO:
        """Write out test logs with specific hierarchy to test filtering."""
        output = io.StringIO()

        # Mock time.monotonic to control duration and avoid sleeping
        with mock.patch("event.time.monotonic") as mock_time:
            mock_time.return_value = 1000.0
            recorder = event.EventRecorder()
            log_task = asyncio.create_task(log.writer(recorder, output))
            recorder.emit_init()

            id_a = recorder.emit_event_group("Group A")
            id_b = recorder.emit_program_start("prog_b", [], parent=id_a)
            recorder.emit_program_termination(id_b, 0)
            id_c = recorder.emit_program_start(
                "prog_c", ["dldist"], parent=id_a
            )
            recorder.emit_program_termination(id_c, 0)
            recorder.emit_end(id=id_a)

            id_d = recorder.emit_build_start(["//build_target_123"])
            mock_time.return_value += 3.0
            id_e = recorder.emit_program_start("prog_e", [], parent=id_d)
            mock_time.return_value += 1.0
            recorder.emit_program_termination(id_e, 0)
            recorder.emit_end(id=id_d)

            id_f = recorder.emit_test_group(1)
            id_g = recorder.emit_test_suite_started(
                "suite_g", False, parent=id_f
            )
            id_h = recorder.emit_program_start("prog_h", [], parent=id_g)
            mock_time.return_value += 2.0
            recorder.emit_program_termination(id_h, 0)
            recorder.emit_test_suite_ended(
                id_g, event.TestSuiteStatus.PASSED, None
            )

            id_i = recorder.emit_test_suite_started(
                "suite_i", False, parent=id_f
            )
            id_j = recorder.emit_program_start("prog_j", [], parent=id_i)
            mock_time.return_value += 3.0
            recorder.emit_program_termination(id_j, 0)
            recorder.emit_test_suite_ended(
                id_i, event.TestSuiteStatus.PASSED, None
            )
            recorder.emit_end(id=id_f)

            id_k = recorder.emit_start_file_parsing("file", "path")
            recorder.emit_end(id=id_k)

            recorder.emit_end()
            await log_task

        output.seek(0)
        return output

    async def test_compute_stats(self) -> None:
        output = await self._write_test_logs()
        stats = log.compute_stats(log.LogSource.from_stream(output))

        top_n_list = stats.top_n
        self.assertEqual(len(top_n_list), 3)

        self.assertEqual(
            top_n_list[0].category, event.EventStatCategory.BUILDING
        )
        self.assertIn("Building 1 targets", top_n_list[0].label)
        self.assertEqual(
            top_n_list[2].category, event.EventStatCategory.TESTING
        )
        self.assertEqual(top_n_list[1].label, "Running TestSuite suite_i")
        self.assertEqual(
            top_n_list[1].category, event.EventStatCategory.TESTING
        )
        self.assertEqual(top_n_list[2].label, "Running TestSuite suite_g")

        summary = stats.summary
        self.assertIn(event.EventStatCategory.BUILDING, summary)
        self.assertEqual(summary[event.EventStatCategory.BUILDING].count, 1)
        self.assertEqual(summary[event.EventStatCategory.BUILDING].mean, 4.0)
        self.assertIn(event.EventStatCategory.TESTING, summary)
        self.assertEqual(summary[event.EventStatCategory.TESTING].count, 2)
        self.assertIn(event.EventStatCategory.SEARCHING, summary)
        self.assertEqual(summary[event.EventStatCategory.SEARCHING].count, 1)
        self.assertIn(event.EventStatCategory.OTHERS, summary)
        self.assertEqual(summary[event.EventStatCategory.OTHERS].count, 1)
        self.assertIn(event.EventStatCategory.PARSING, summary)
        self.assertEqual(summary[event.EventStatCategory.PARSING].count, 1)

    async def test_to_analytics_dict(self) -> None:
        output = await self._write_test_logs()
        stats = log.compute_stats(log.LogSource.from_stream(output))
        analytics = stats.to_analytics_dict()

        # Wall time in microseconds
        self.assertGreater(analytics["t"], 0)
        self.assertEqual(analytics["a"], 0)
        self.assertIsNone(analytics["ab"])

        # Top operations
        self.assertEqual(len(analytics["top"]), 3)
        self.assertEqual(analytics["top"][0]["c"], "B")
        self.assertIn("Building 1 targets", analytics["top"][0]["l"])
        self.assertGreater(analytics["top"][0]["d"], 0)
        self.assertEqual(analytics["top"][1]["c"], "T")
        self.assertEqual(analytics["top"][1]["l"], "suite_i")
        self.assertGreater(analytics["top"][1]["d"], 0)
        self.assertEqual(analytics["top"][2]["c"], "T")
        self.assertEqual(analytics["top"][2]["l"], "suite_g")
        self.assertGreater(analytics["top"][2]["d"], 0)

        # Summary categories
        self.assertIn("B", analytics["sum"])
        self.assertIn("T", analytics["sum"])
        self.assertIn("S", analytics["sum"])
        self.assertIn("P", analytics["sum"])
        self.assertIn("O", analytics["sum"])
        self.assertEqual(analytics["sum"]["T"]["c"], 2)
        self.assertGreater(analytics["sum"]["T"]["s"], 0)
        self.assertGreater(analytics["sum"]["T"]["m"], 0)

    def test_execution_stats_fields_accounted_for_in_analytics(self) -> None:
        """Verify that all ExecutionStats fields are accounted for in analytics."""
        field_to_analytics_key = {
            "top_n": "top",
            "summary": "sum",
            "was_aborted": "a",
            "total_elapsed_micros": "t",
            "aborted_span": "ab",
            "selection": "sel",
        }
        excluded_fields: set[str] = set()

        all_dataclass_fields = {
            f.name for f in dataclasses.fields(log.ExecutionStats)
        }
        accounted_fields = set(field_to_analytics_key.keys()) | excluded_fields

        self.assertEqual(
            all_dataclass_fields,
            accounted_fields,
            "ExecutionStats fields changed! Please update to_analytics_dict() "
            "and field_to_analytics_key (or add to excluded_fields).",
        )

        stats = log.ExecutionStats(
            top_n=[],
            summary={},
            was_aborted=True,
            total_elapsed_micros=100,
            aborted_span=log.AbortedSpan("test", 100, "T"),
        )
        analytics_dict = stats.to_analytics_dict()
        self.assertEqual(
            set(analytics_dict.keys()),
            set(field_to_analytics_key.values()),
            "to_analytics_dict() output keys do not match field_to_analytics_key.",
        )

    def test_category_code_map_covers_all_categories(self) -> None:
        """Verify that _CATEGORY_CODE_MAP covers all non-ignored EventStatCategory members."""
        expected_categories = set(event.EventStatCategory) - {
            event.EventStatCategory.IGNORE
        }
        self.assertEqual(
            set(log._CATEGORY_CODE_MAP.keys()),
            expected_categories,
            "New category added to EventStatCategory! Please map it in _CATEGORY_CODE_MAP.",
        )
        self.assertEqual(
            len(set(log._CATEGORY_CODE_MAP.values())),
            len(log._CATEGORY_CODE_MAP),
            "Duplicate category code found in _CATEGORY_CODE_MAP.",
        )

    async def test_compute_stats_interrupted(self) -> None:
        output = io.StringIO()
        with mock.patch("event.time.monotonic") as mock_time:
            mock_time.return_value = 2000.0
            recorder = event.EventRecorder()
            log_task = asyncio.create_task(log.writer(recorder, output))
            recorder.emit_init()

            # Completed test
            id_f = recorder.emit_test_group(1)
            id_completed = recorder.emit_test_suite_started(
                "fuchsia-pkg://fuchsia.com/quick-test#meta/quick-test.cm",
                False,
                parent=id_f,
            )
            mock_time.return_value += 2.0
            recorder.emit_test_suite_ended(
                id_completed, event.TestSuiteStatus.PASSED, None
            )

            # In-flight test that gets interrupted
            recorder.emit_test_suite_started(
                "fuchsia-pkg://fuchsia.com/slow-test#meta/slow-test.cm",
                False,
                parent=id_f,
            )
            mock_time.return_value += 5.0
            # Simulating abort: suite never emits test_suite_ended, global run ends abruptly
            recorder.emit_end()
            await log_task

        output.seek(0)
        stats = log.compute_stats(log.LogSource.from_stream(output))
        self.assertTrue(stats.was_aborted)
        self.assertIsNotNone(stats.aborted_span)
        assert stats.aborted_span is not None
        self.assertEqual(
            stats.aborted_span.label,
            "Running TestSuite fuchsia-pkg://fuchsia.com/slow-test#meta/slow-test.cm",
        )
        self.assertEqual(stats.aborted_span.category, "T")
        self.assertEqual(stats.aborted_span.elapsed_micros, 5_000_000)

        # Completed operations in summary/top, interrupted test is NOT in top/summary
        self.assertEqual(
            stats.summary[event.EventStatCategory.TESTING].count, 1
        )
        self.assertEqual(len(stats.top_n), 1)
        self.assertEqual(
            stats.top_n[0].label,
            "Running TestSuite fuchsia-pkg://fuchsia.com/quick-test#meta/quick-test.cm",
        )

        analytics = stats.to_analytics_dict()
        self.assertEqual(analytics["a"], 1)
        self.assertIsNotNone(analytics["ab"])
        assert analytics["ab"] is not None
        self.assertEqual(analytics["ab"]["l"], "slow-test#meta/slow-test.cm")
        self.assertEqual(analytics["ab"]["c"], "T")
        self.assertEqual(analytics["ab"]["d"], 5_000_000)

    def test_format_selection_string(self) -> None:
        """Test canonical selection string formatting."""
        # None flags
        self.assertEqual(log.format_selection_string(None), "")

        # Empty selection
        self.assertEqual(log.format_selection_string({}), "")
        self.assertEqual(log.format_selection_string({"selection": []}), "")

        # Single test name
        self.assertEqual(
            log.format_selection_string({"selection": ["archivist"]}),
            "archivist",
        )

        # Conjunction (--and)
        self.assertEqual(
            log.format_selection_string(
                {
                    "selection": [
                        "--package",
                        "foo",
                        "--and",
                        "--component",
                        "bar",
                    ]
                }
            ),
            "--package foo --and --component bar",
        )

        # Disjunction (multiple match groups)
        self.assertEqual(
            log.format_selection_string(
                {"selection": ["--package", "foo", "--component", "bar"]}
            ),
            "--package foo --component bar",
        )

        # Run affected tests alone
        self.assertEqual(
            log.format_selection_string({"run_affected_tests": True}),
            "--run-affected-tests",
        )

        # Run affected tests combined
        self.assertEqual(
            log.format_selection_string(
                {"run_affected_tests": True, "selection": ["my_test"]}
            ),
            "my_test --run-affected-tests",
        )

        # Fallback on syntax errors in selection
        self.assertEqual(
            log.format_selection_string({"selection": ["--package"]}),
            "--package",
        )

        # Length capping at 1000 chars
        long_names = ["very_long_test_name_" + str(i) for i in range(100)]
        formatted = log.format_selection_string({"selection": long_names})
        self.assertLessEqual(len(formatted), 1000)

    async def test_compute_stats_with_selection(self) -> None:
        """Verify that compute_stats extracts selection from parse_flags event."""
        output = io.StringIO()
        recorder = event.EventRecorder()
        log_task = asyncio.create_task(log.writer(recorder, output))
        recorder.emit_init()
        recorder.emit_parse_flags(
            {
                "selection": [
                    "--package",
                    "my_pkg",
                    "--and",
                    "--component",
                    "my_comp",
                ],
                "run_affected_tests": True,
            }
        )
        recorder.emit_end()
        await log_task

        output.seek(0)
        stats = log.compute_stats(log.LogSource.from_stream(output))
        self.assertEqual(
            stats.selection,
            "--package my_pkg --and --component my_comp --run-affected-tests",
        )
        analytics = stats.to_analytics_dict()
        self.assertEqual(
            analytics["sel"],
            "--package my_pkg --and --component my_comp --run-affected-tests",
        )


class TestExclusiveLogFile(unittest.TestCase):
    def test_format_fallback_path(self) -> None:
        self.assertEqual(
            log.format_fallback_path("test.log.json.gz", 1234),
            "test.1234.log.json.gz",
        )
        self.assertEqual(
            log.format_fallback_path("foo.json.gz", 5678),
            "foo.5678.json.gz",
        )
        self.assertEqual(
            log.format_fallback_path("bar.gz", 9999),
            "bar.9999.gz",
        )
        # For arbitrary custom paths without known log extensions, safely
        # append the PID as a suffix rather than guessing stem boundaries.
        self.assertEqual(
            log.format_fallback_path("custom_log", 42),
            "custom_log.42",
        )

    def test_open_exclusive_log_file_single_and_overwrite(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            path = os.path.join(td, "test.log.json.gz")

            # First open: creates and writes cleanly
            stream1, actual_path1 = log.open_exclusive_log_file(path)
            self.assertEqual(actual_path1, path)
            stream1.write("run 1 content\n")
            stream1.close()

            # Second open: since stream1 was closed, lock is released and file is overwritten
            stream2, actual_path2 = log.open_exclusive_log_file(path)
            self.assertEqual(actual_path2, path)
            stream2.write("run 2 content\n")
            stream2.close()

            with gzip.open(path, "rt") as f:
                content = f.read()
            self.assertEqual(content, "run 2 content\n")

    def test_open_exclusive_log_file_contention_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            path = os.path.join(td, "test.log.json.gz")

            # Simulate an active writer holding an exclusive lock
            fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)

                # Attempting to open the same path falls back to PID suffix
                stream, actual_path = log.open_exclusive_log_file(path)
                expected_fallback = log.format_fallback_path(path, os.getpid())
                self.assertEqual(actual_path, expected_fallback)
                self.assertTrue(os.path.exists(expected_fallback))

                stream.write("fallback content\n")
                stream.close()

                with gzip.open(expected_fallback, "rt") as f:
                    self.assertEqual(f.read(), "fallback content\n")
            finally:
                os.close(fd)
