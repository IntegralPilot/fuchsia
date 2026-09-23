# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the cpu diagnostic analysis plugin."""

import unittest
from typing import Any
from unittest.mock import MagicMock, patch

from cpu import (
    CpuPlugin,
    _analyze_core_utilization_from_average,
    _analyze_core_utilization_from_counters,
)
from plugins import PluginArgumentError


class CpuPluginTest(unittest.TestCase):
    """Tests for the cpu analysis plugin using mock trace processor queries."""

    def setUp(self) -> None:
        self.all_tables = {
            "trace_bounds",
            "thread_state",
            "thread",
            "process",
            "slice",
            "thread_track",
            "counter",
            "process_counter_track",
        }

    def test_analyze_success_flow(self) -> None:
        """Tests that all 6 diagnostic sections are executed in prioritized order with expected outputs."""
        plugin = CpuPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = self.all_tables

        def run_query_mock(sql: str) -> list[dict[str, Any]]:
            if "wakeup_count" in sql:
                return [
                    {
                        "thread_name": "idle",
                        "tid": 0,
                        "process_name": "idle",
                        "pid": 0,
                        "wakeup_count": 5000,
                        "avg_duration_ms": 0.85,
                    },
                    {
                        "thread_name": "worker_thread",
                        "tid": 101,
                        "process_name": "app.cm",
                        "pid": 42,
                        "wakeup_count": 1200,
                        "avg_duration_ms": 0.05,
                    },
                ]
            elif "Processing Rate" in sql or "effective_load_pct" in sql:
                return [
                    {
                        "cpu": 0,
                        "running_ms": 150.0,
                        "running_pct": 1.5,
                        "avg_rate_pct": 52.4,
                        "min_rate_pct": 40.0,
                        "max_rate_pct": 100.0,
                        "effective_load_pct": 0.79,
                    }
                ]
            elif "total_cpu_ms" in sql and "thread_state" in sql:
                return [
                    {
                        "thread_name": "heavy_worker",
                        "tid": 201,
                        "process_name": "service.cm",
                        "pid": 50,
                        "total_cpu_ms": 320.5,
                    }
                ]
            elif "slice_count" in sql:
                return [
                    {
                        "thread_name": "async_executor",
                        "tid": 301,
                        "slice_count": 450,
                        "total_duration_ms": 42.1,
                    }
                ]
            elif "binder transaction" in sql:
                return [
                    {
                        "thread_name": "binder:401",
                        "tid": 401,
                        "process_name": "android.service",
                        "pid": 80,
                        "transaction_count": 85,
                        "total_duration_ms": 15.4,
                        "max_duration_ms": 2.1,
                    }
                ]
            elif "wake_lease" in sql or "suspend" in sql:
                return [
                    {
                        "event_name": "acquire_wake_lease:power",
                        "thread_name": "sag_thread",
                        "process_name": "system-activity-governor.cm",
                        "occurrences": 12,
                        "total_duration_ms": 55.0,
                        "max_duration_ms": 10.0,
                    }
                ]
            return []

        mock_tp.run_query.side_effect = run_query_mock

        with patch("cpu.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze(["--limit", "10"], "dummy_trace")

        self.assertEqual(len(results), 6)

        # 1. Restless Sleepers (Wakeups)
        self.assertEqual(results[0].name, "Restless Sleepers (Wakeup Counts)")
        res0 = results[0].results
        assert res0 is not None
        self.assertEqual(len(res0), 2)
        self.assertEqual(res0[0]["thread_name"], "idle")
        self.assertEqual(res0[1]["wakeup_count"], 1200)

        # 2. Per-Core Utilization & Processing Rate
        self.assertEqual(
            results[1].name, "Per-Core Utilization & Processing Rate"
        )
        res1 = results[1].results
        assert res1 is not None
        self.assertEqual(len(res1), 1)
        self.assertEqual(res1[0]["cpu"], 0)
        self.assertEqual(res1[0]["avg_rate_pct"], 52.4)

        # 3. Top CPU Consumers
        self.assertEqual(results[2].name, "Top CPU Consumers (Usual Suspects)")
        res2 = results[2].results
        assert res2 is not None
        self.assertEqual(len(res2), 1)
        self.assertEqual(res2[0]["total_cpu_ms"], 320.5)

        # 4. Fuchsia Async Executor Overhead
        self.assertEqual(results[3].name, "Fuchsia Async Executor Overhead")
        res3 = results[3].results
        assert res3 is not None
        self.assertEqual(len(res3), 1)
        self.assertEqual(res3[0]["slice_count"], 450)

        # 5. Binder IPC Breakdown
        self.assertEqual(results[4].name, "Binder IPC Breakdown")
        res4 = results[4].results
        assert res4 is not None
        self.assertEqual(len(res4), 1)
        self.assertEqual(res4[0]["transaction_count"], 85)

        # 6. Suspend and Wake Lease Tracking
        self.assertEqual(results[5].name, "Suspend and Wake Lease Tracking")
        res5 = results[5].results
        assert res5 is not None
        self.assertEqual(len(res5), 1)
        self.assertEqual(res5[0]["event_name"], "acquire_wake_lease:power")

    def test_analyze_empty_results(self) -> None:
        """Tests that empty query results format as empty lists across all 6 sections."""
        plugin = CpuPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = self.all_tables
        mock_tp.run_query.return_value = []

        with patch("cpu.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze([], "dummy_trace")

        self.assertEqual(len(results), 6)
        for section in results:
            self.assertTrue(section.name)
            self.assertEqual(section.results, [])

    def test_analyze_missing_schema(self) -> None:
        """Tests that missing schema tables are handled gracefully with section error messages."""
        plugin = CpuPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = set()
        mock_tp.run_query.return_value = []

        with patch("cpu.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze([], "dummy_trace")

        self.assertEqual(len(results), 6)
        for section in results:
            assert section.error is not None
            self.assertIn("Required schema tables/views missing", section.error)

    def test_analyze_without_frequency_counters(self) -> None:
        """Tests that Query 2 falls back to core utilization when frequency counters are absent."""
        plugin = CpuPlugin()
        mock_tp = MagicMock()
        # All required tables except counter and process_counter_track
        tables_no_counters = {
            "trace_bounds",
            "thread_state",
            "thread",
            "process",
            "slice",
            "thread_track",
        }
        mock_tp.get_tables.return_value = tables_no_counters

        def run_query_mock(sql: str) -> list[dict[str, Any]]:
            if "running_pct" in sql:
                self.assertNotIn("process_counter_track", sql)
                return [
                    {
                        "cpu": 0,
                        "running_ms": 100.0,
                        "running_pct": 2.0,
                        "avg_rate_pct": None,
                        "min_rate_pct": None,
                        "max_rate_pct": None,
                        "effective_load_pct": 2.0,
                    }
                ]
            return []

        mock_tp.run_query.side_effect = run_query_mock

        with patch("cpu.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze([], "dummy_trace")

        self.assertEqual(len(results), 6)
        core_util_section = results[1]
        self.assertEqual(
            core_util_section.name, "Per-Core Utilization & Processing Rate"
        )
        core_results = core_util_section.results
        assert core_results is not None
        self.assertEqual(len(core_results), 1)
        self.assertIsNone(core_results[0]["avg_rate_pct"])

    def test_analyze_query_failure(self) -> None:
        """Tests that runtime SQL query failures return formatted error dicts without crashing."""
        plugin = CpuPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = self.all_tables
        mock_tp.run_query.side_effect = RuntimeError(
            "database disk image is malformed"
        )

        with patch("cpu.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze([], "dummy_trace")

        self.assertEqual(len(results), 6)
        for section in results:
            assert section.error is not None
            self.assertIn(
                "Query execution failed: database disk image is malformed",
                section.error,
            )

    def test_analyze_core_utilization_variants_directly(self) -> None:
        """Tests calling from_counters and from_average variants directly."""
        mock_tp = MagicMock()
        mock_tp.run_query.return_value = [
            {
                "cpu": 0,
                "running_ms": 100.0,
                "running_pct": 2.0,
                "avg_rate_pct": 50.0,
                "min_rate_pct": 40.0,
                "max_rate_pct": 60.0,
                "effective_load_pct": 1.0,
            }
        ]
        res_counters = _analyze_core_utilization_from_counters(
            mock_tp, self.all_tables
        )
        self.assertEqual(
            res_counters.name, "Per-Core Utilization & Processing Rate"
        )
        assert res_counters.results is not None
        self.assertEqual(len(res_counters.results), 1)

        res_average = _analyze_core_utilization_from_average(
            mock_tp, self.all_tables
        )
        self.assertEqual(
            res_average.name, "Per-Core Utilization & Processing Rate"
        )
        assert res_average.results is not None
        self.assertEqual(len(res_average.results), 1)

    def test_analyze_with_counter_tables_but_no_power_tracks(self) -> None:
        """Tests that from_average is used when counter tables exist but no kernel:power tracks exist."""
        plugin = CpuPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = self.all_tables

        def run_query_mock(sql: str) -> list[dict[str, Any]]:
            if "LIMIT 1" in sql and "Processing Rate:CPU:%" in sql:
                # Probe query finds no kernel:power tracks
                return []
            if "running_pct" in sql:
                self.assertNotIn("process_counter_track", sql)
                return [
                    {
                        "cpu": 0,
                        "running_ms": 100.0,
                        "running_pct": 2.0,
                        "avg_rate_pct": None,
                        "min_rate_pct": None,
                        "max_rate_pct": None,
                        "effective_load_pct": 2.0,
                    }
                ]
            return []

        mock_tp.run_query.side_effect = run_query_mock

        with patch("cpu.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze([], "dummy_trace")

        self.assertEqual(len(results), 6)
        core_util_section = results[1]
        self.assertEqual(
            core_util_section.name, "Per-Core Utilization & Processing Rate"
        )
        core_results = core_util_section.results
        assert core_results is not None
        self.assertEqual(len(core_results), 1)
        self.assertIsNone(core_results[0]["avg_rate_pct"])

    def test_analyze_invalid_limit(self) -> None:
        """Tests that invalid, non-positive, or non-integer limits raise PluginArgumentError."""
        plugin = CpuPlugin()
        for invalid_limit in ["0", "-5"]:
            with self.subTest(limit=invalid_limit):
                with self.assertRaisesRegex(
                    PluginArgumentError, "Limit must be a positive integer"
                ):
                    plugin.analyze(["--limit", invalid_limit], "dummy_trace")

        with self.assertRaisesRegex(PluginArgumentError, "invalid int value"):
            plugin.analyze(["--limit", "abc"], "dummy_trace")


if __name__ == "__main__":
    unittest.main()
