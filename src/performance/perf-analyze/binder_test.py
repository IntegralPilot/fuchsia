# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the Starnix binder analysis plugin."""

import unittest
from typing import Any
from unittest.mock import MagicMock, patch

from binder import (
    CATEGORY_NOT_FOUND_NOTE,
    BinderPlugin,
    _has_starnix_binder_category,
)
from plugins import PluginArgumentError


class BinderPluginTest(unittest.TestCase):
    """Tests for the BinderPlugin and its analysis functions."""

    def test_analyze_empty_trace(self) -> None:
        """Tests analyzing a trace with no binder activity."""
        plugin = BinderPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = {
            "thread_state",
            "slice",
            "thread_track",
            "thread",
            "process",
            "process_track",
            "flow",
            "args",
            "trace_bounds",
            "counter",
            "process_counter_track",
        }
        mock_tp.run_query.return_value = [{"1": 1}]

        def run_query_side_effect(sql: str) -> list[dict[str, Any]]:
            if "category = 'starnix:binder'" in sql and "SELECT 1" in sql:
                return [{"1": 1}]
            return []

        mock_tp.run_query.side_effect = run_query_side_effect

        with patch("binder.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze(["--threshold-ms", "10.0"], "dummy_trace")

        self.assertEqual(len(results), 5)
        self.assertEqual(results[0].name, "Missed Wakeups (Wakeup Latencies)")
        self.assertEqual(results[0].results, [])
        self.assertEqual(
            results[1].name, "Binder Delays (Transaction Queue Latencies)"
        )
        self.assertEqual(results[1].results, [])
        self.assertEqual(
            results[2].name, "Spawn Looper Events (Late-Spawned Wakers)"
        )
        self.assertEqual(results[2].results, [])
        self.assertEqual(
            results[3].name,
            "No Available Threads Events (Thread Pool Exhaustion)",
        )
        self.assertEqual(results[3].results, [])
        self.assertEqual(
            results[4].name, "Binder Process Queue Depth (Counter Summary)"
        )
        self.assertEqual(results[4].results, [])

    def test_analyze_normal_waker_delay(self) -> None:
        """Tests identifying wakeups and binder delays without spawning loopers."""
        plugin = BinderPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = {
            "thread_state",
            "slice",
            "thread_track",
            "thread",
            "process",
            "process_track",
            "flow",
            "args",
            "trace_bounds",
            "counter",
            "process_counter_track",
        }

        def run_query_mock(sql: str) -> list[dict[str, Any]]:
            if "category = 'starnix:binder'" in sql and "SELECT 1" in sql:
                return [{"1": 1}]
            if "thread_state" in sql:
                return [
                    {
                        "wakeup_ts": 1000,
                        "scheduling_delay_ns": 15000000,
                        "thread_name": "binder:100",
                        "tid": 100,
                        "process_name": "target_proc",
                        "pid": 50,
                        "waker_thread_name": "waker_proc",
                        "waker_tid": 200,
                    }
                ]
            elif "flow" in sql and "SpawnLooper" not in sql:
                return [
                    {
                        "flow_id": 1,
                        "send_ts": 2000,
                        "recv_ts": 17000,
                        "queue_latency_ns": 15000000,
                        "sender_thread": "sender_thread",
                        "sender_tid": 200,
                        "receiver_thread": "binder:100",
                        "receiver_tid": 100,
                        "status": "Completed",
                        "cmd": "Transaction { ... }",
                    }
                ]
            return []

        mock_tp.run_query.side_effect = run_query_mock

        with patch("binder.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze(["--threshold-ms", "10.0"], "dummy_trace")

        self.assertEqual(len(results), 5)
        r0 = results[0].results
        r1 = results[1].results
        r2 = results[2].results
        r3 = results[3].results
        r4 = results[4].results
        assert (
            r0 is not None
            and r1 is not None
            and r2 is not None
            and r3 is not None
            and r4 is not None
        )
        self.assertEqual(len(r0), 1)
        self.assertEqual(r0[0]["thread_name"], "binder:100")
        self.assertEqual(len(r1), 1)
        self.assertEqual(r1[0]["queue_latency_ns"], 15000000)
        self.assertEqual(len(r2), 0)
        self.assertEqual(len(r3), 0)
        self.assertEqual(len(r4), 0)

    def test_analyze_instant_events_and_queue_depth(self) -> None:
        """Tests auditing SpawnLooper, no_available_threads, and process_queue_depth."""
        plugin = BinderPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = {
            "thread_state",
            "slice",
            "thread_track",
            "thread",
            "process",
            "process_track",
            "flow",
            "args",
            "trace_bounds",
            "counter",
            "process_counter_track",
        }

        def run_query_mock(sql: str) -> list[dict[str, Any]]:
            if "category = 'starnix:binder'" in sql and "SELECT 1" in sql:
                return [{"1": 1}]
            if "thread_state" in sql:
                return []
            elif "flow" in sql:
                return [
                    {
                        "flow_id": 1,
                        "send_ts": 2000,
                        "recv_ts": 17000,
                        "queue_latency_ns": 15000000,
                        "sender_thread": "sender_thread",
                        "sender_tid": 200,
                        "receiver_thread": "binder:100",
                        "receiver_tid": 100,
                        "status": "Completed",
                        "cmd": "Transaction",
                    }
                ]
            elif "SpawnLooper" in sql:
                return [
                    {
                        "ts": 3000,
                        "process_name": "target_proc",
                        "pid": 50,
                        "thread_name": "binder:100",
                        "tid": 100,
                        "parent_slice_name": "BinderIoctl",
                        "parent_dur_ns": 500000,
                    }
                ]
            elif "no_available_threads" in sql:
                return [
                    {
                        "ts": 3500,
                        "process_name": "target_proc",
                        "pid": 50,
                        "target_pid": 50,
                        "command": "Transaction",
                    }
                ]
            elif "process_queue_depth" in sql:
                return [
                    {
                        "process_name": "target_proc",
                        "pid": 50,
                        "max_queue_depth": 4.0,
                        "avg_queue_depth": 2.5,
                        "sample_count": 10,
                    }
                ]
            return []

        mock_tp.run_query.side_effect = run_query_mock

        with patch("binder.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze(["--threshold-ms", "10.0"], "dummy_trace")

        self.assertEqual(len(results), 5)
        r0 = results[0].results
        r1 = results[1].results
        r2 = results[2].results
        r3 = results[3].results
        r4 = results[4].results
        assert (
            r0 is not None
            and r1 is not None
            and r2 is not None
            and r3 is not None
            and r4 is not None
        )
        self.assertEqual(len(r0), 0)
        self.assertEqual(len(r1), 1)
        self.assertEqual(len(r2), 1)
        self.assertEqual(r2[0]["parent_slice_name"], "BinderIoctl")
        self.assertEqual(len(r3), 1)
        self.assertEqual(r3[0]["command"], "Transaction")
        self.assertEqual(len(r4), 1)
        self.assertEqual(r4[0]["max_queue_depth"], 4.0)

    def test_analyze_negative_threshold(self) -> None:
        """Tests that passing a negative threshold raises PluginArgumentError."""
        plugin = BinderPlugin()
        with self.assertRaises(PluginArgumentError):
            plugin.analyze(["--threshold-ms", "-1.0"], "dummy_trace")

    def test_analyze_missing_tables(self) -> None:
        """Tests behavior when required Perfetto tables are missing."""
        plugin = BinderPlugin()
        mock_tp = MagicMock()
        # Mocking empty database schema (missing required tables)
        mock_tp.get_tables.return_value = set()
        mock_tp.run_query.return_value = []

        with patch("binder.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze(["--threshold-ms", "10.0"], "dummy_trace")

        self.assertEqual(len(results), 6)
        self.assertEqual(results[0].name, "Trace Category Status")
        expected_names = [
            "Missed Wakeups (Wakeup Latencies)",
            "Binder Delays (Transaction Queue Latencies)",
            "Spawn Looper Events (Late-Spawned Wakers)",
            "No Available Threads Events (Thread Pool Exhaustion)",
            "Binder Process Queue Depth (Counter Summary)",
        ]
        self.assertEqual([item.name for item in results[1:]], expected_names)
        for item in results[1:]:
            self.assertIsNotNone(item.error)
            assert item.error is not None
            self.assertIn("Required schema tables/views missing", item.error)

    def test_analyze_incomplete_transactions(self) -> None:
        """Tests that incomplete transactions are returned by default and filtered with --complete-only."""
        plugin = BinderPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = {
            "thread_state",
            "slice",
            "thread_track",
            "thread",
            "process",
            "process_track",
            "flow",
            "args",
            "trace_bounds",
            "counter",
            "process_counter_track",
        }

        def run_query_mock(sql: str) -> list[dict[str, Any]]:
            if "category = 'starnix:binder'" in sql and "SELECT 1" in sql:
                return [{"1": 1}]
            if "thread_state" in sql:
                return []
            elif "flow" in sql and "SpawnLooper" not in sql:
                if "UNION ALL" in sql:
                    # Default: includes incomplete
                    return [
                        {
                            "flow_id": None,
                            "send_ts": 1000,
                            "recv_ts": None,
                            "queue_latency_ns": 50000000,
                            "sender_thread": "sender_thread",
                            "sender_tid": 200,
                            "receiver_thread": None,
                            "receiver_tid": None,
                            "status": "Incomplete",
                            "cmd": "Transaction { ... }",
                        },
                        {
                            "flow_id": 1,
                            "send_ts": 2000,
                            "recv_ts": 17000,
                            "queue_latency_ns": 15000000,
                            "sender_thread": "sender_thread",
                            "sender_tid": 200,
                            "receiver_thread": "binder:100",
                            "receiver_tid": 100,
                            "status": "Completed",
                            "cmd": "Transaction { ... }",
                        },
                    ]
                else:
                    # complete-only
                    return [
                        {
                            "flow_id": 1,
                            "send_ts": 2000,
                            "recv_ts": 17000,
                            "queue_latency_ns": 15000000,
                            "sender_thread": "sender_thread",
                            "sender_tid": 200,
                            "receiver_thread": "binder:100",
                            "receiver_tid": 100,
                            "status": "Completed",
                            "cmd": "Transaction { ... }",
                        }
                    ]
            return []

        mock_tp.run_query.side_effect = run_query_mock

        with patch("binder.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp

            # Test default (includes incomplete)
            results_default = plugin.analyze(
                ["--threshold-ms", "10.0"], "dummy_trace"
            )
            self.assertEqual(len(results_default), 5)
            delays_default = results_default[1].results
            self.assertIsNotNone(delays_default)
            assert delays_default is not None
            self.assertEqual(len(delays_default), 2)
            self.assertEqual(delays_default[0]["status"], "Incomplete")
            self.assertEqual(delays_default[1]["status"], "Completed")

            # Test --complete-only
            results_complete_only = plugin.analyze(
                ["--threshold-ms", "10.0", "--complete-only"], "dummy_trace"
            )
            self.assertEqual(len(results_complete_only), 5)
            delays_complete_only = results_complete_only[1].results
            self.assertIsNotNone(delays_complete_only)
            assert delays_complete_only is not None
            self.assertEqual(len(delays_complete_only), 1)
            self.assertEqual(delays_complete_only[0]["status"], "Completed")

    def test_has_starnix_binder_category(self) -> None:
        """Tests _has_starnix_binder_category logic."""
        mock_tp = MagicMock()

        # Missing slice table
        self.assertFalse(_has_starnix_binder_category(mock_tp, set()))

        # Present
        mock_tp.run_query.return_value = [{"1": 1}]
        self.assertTrue(_has_starnix_binder_category(mock_tp, {"slice"}))

        # Empty result
        mock_tp.run_query.return_value = []
        self.assertFalse(_has_starnix_binder_category(mock_tp, {"slice"}))

        # Query exception
        mock_tp.run_query.side_effect = RuntimeError("database locked")
        self.assertFalse(_has_starnix_binder_category(mock_tp, {"slice"}))

    def test_analyze_category_missing_fallback(self) -> None:
        """Tests that missing starnix:binder triggers category status, warning note, and fallback queries."""
        plugin = BinderPlugin()
        mock_tp = MagicMock()
        mock_tp.get_tables.return_value = {
            "thread_state",
            "slice",
            "thread_track",
            "thread",
            "process",
            "process_track",
            "flow",
            "args",
            "trace_bounds",
            "counter",
            "process_counter_track",
        }

        queries_executed: list[str] = []

        def run_query_mock(sql: str) -> list[dict[str, Any]]:
            queries_executed.append(sql)
            if "category = 'starnix:binder'" in sql and "SELECT 1" in sql:
                return []
            if "thread_state" in sql:
                return [
                    {
                        "wakeup_ts": 1000,
                        "scheduling_delay_ns": 15000000,
                        "thread_name": "binder:100",
                        "tid": 100,
                        "process_name": "target_proc",
                        "pid": 50,
                        "waker_thread_name": "waker_proc",
                        "waker_tid": 200,
                    }
                ]
            elif "flow" in sql and "SpawnLooper" not in sql:
                return [
                    {
                        "flow_id": 1,
                        "send_ts": 2000,
                        "recv_ts": 17000,
                        "queue_latency_ns": 15000000,
                        "sender_thread": "sender_thread",
                        "sender_tid": 200,
                        "receiver_thread": "binder:100",
                        "receiver_tid": 100,
                        "status": "Completed",
                        "cmd": "Transaction { ... }",
                    }
                ]
            return []

        mock_tp.run_query.side_effect = run_query_mock

        with patch("binder.PerfettoTraceProcessor") as mock_tp_class:
            mock_tp_class.return_value.__enter__.return_value = mock_tp
            results = plugin.analyze(["--threshold-ms", "10.0"], "dummy_trace")

        self.assertEqual(len(results), 6)
        self.assertEqual(results[0].name, "Trace Category Status")
        self.assertEqual(results[0].note, CATEGORY_NOT_FOUND_NOTE)
        r0 = results[0].results
        assert r0 is not None
        self.assertIn("recommendation", r0[0])

        for item in results[1:]:
            self.assertEqual(item.note, CATEGORY_NOT_FOUND_NOTE)

        # Verify fallback query SQL used binder:%, binder_%, and binder_ioctl heuristics
        wakeups_query = [q for q in queries_executed if "thread_state" in q][0]
        self.assertIn("binder:%", wakeups_query)
        self.assertIn("binder_%", wakeups_query)
        self.assertIn("binder_ioctl", wakeups_query)
        self.assertIn("ts.state IN ('R', 'R+')", wakeups_query)

        delays_query = [
            q
            for q in queries_executed
            if "flow" in q and "SpawnLooper" not in q
        ][0]
        self.assertIn("binder:%", delays_query)
        self.assertIn("binder_%", delays_query)
        self.assertIn("binder_ioctl", delays_query)

        # Verify SpawnLooper, no_available_threads, and queue_depth queries were skipped
        self.assertFalse(any("SpawnLooper" in q for q in queries_executed))
        self.assertFalse(
            any("no_available_threads" in q for q in queries_executed)
        )
        self.assertFalse(
            any("process_queue_depth" in q for q in queries_executed)
        )


if __name__ == "__main__":
    unittest.main()
