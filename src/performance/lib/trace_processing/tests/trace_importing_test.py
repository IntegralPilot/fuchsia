#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for trace_importing.py."""
import os
import pathlib
import unittest
from typing import Any, Dict, List

import test_utils
import trace_processing.trace_importing as trace_importing
import trace_processing.trace_model as trace_model
import trace_processing.trace_utils as trace_utils
from trace_processing.trace_time import TimePoint


class TraceImportingTest(unittest.TestCase):
    """Trace importing tests"""

    def setUp(self) -> None:
        # A second `parent` is required to account for the .pyz archive which
        # contains the test and a third one since data is a sibling of the test.
        self._runtime_deps_path = (
            pathlib.Path(__file__).parent.parent.parent / "runtime_deps"
        )

    def test_create_model(self) -> None:
        """Test case to ensure we can load a model from a file"""

        model: trace_model.Model = test_utils.get_test_model()
        model_from_json: trace_model.Model = (
            trace_importing.create_model_from_file_path(
                os.path.join(self._runtime_deps_path, "model.json")
            )
        )
        test_utils.assertModelsEqual(self, model, model_from_json)

    def test_create_model_from_fxt_directly(self) -> None:
        """Test case to ensure we can load a model from an FXT file directly."""

        model: trace_model.Model = test_utils.get_test_model(is_fxt=True)
        model_from_fxt: trace_model.Model = (
            trace_importing.create_model_from_trace_file_path(
                os.path.join(self._runtime_deps_path, "model.fxt"),
                without_json_conversion=True,
            )
        )
        test_utils.assertModelsEqual(self, model, model_from_fxt)

    def test_create_model_from_split_files(self) -> None:
        """Test case to ensure we can load a model from split files"""

        model: trace_model.Model = test_utils.get_test_model()
        model_from_split_json: trace_model.Model = (
            trace_importing.create_model_from_file_paths(
                os.path.join(self._runtime_deps_path, "split_model.json"),
                os.path.join(
                    self._runtime_deps_path,
                    "split_model.systemTraceEvents.jsonlines",
                ),
            )
        )
        test_utils.assertModelsEqual(self, model, model_from_split_json)

    def test_time_convert_trace_file_to_json_split(self) -> None:
        """Test converting a trace file to split JSON files."""
        # Temporarily refer directly to these dependencies until we align behavior
        # across packages.
        trace2json_data_path = (
            pathlib.Path("host_x64").absolute() / "test_data" / "trace2json"
        )

        trace_file = trace2json_data_path / "simple_trace.fxt"

        (
            output_json,
            jsonlines_path,
            _,
        ) = trace_importing.time_convert_trace_file_to_json(
            trace_file, split=True
        )

        model_from_split = trace_importing.create_model_from_file_paths(
            output_json, jsonlines_path
        )

        expected_model_path = (
            trace2json_data_path / "simple_trace_expected.json"
        )

        expected_model = trace_importing.create_model_from_file_path(
            expected_model_path
        )

        test_utils.assertModelsEqual(self, expected_model, model_from_split)

    def test_convert_trace_file_to_json_rusage_max_rss(self) -> None:
        # Temporarily refer directly to these dependencies until we align behavior
        # across packages.
        trace2json_data_path = (
            pathlib.Path("host_x64").absolute() / "test_data" / "trace2json"
        )
        trace_file = trace2json_data_path / "simple_trace.fxt"

        _, _, output = trace_importing.convert_trace_file_to_json_rusage(
            trace_file, split=False
        )
        self.assertIn("Maximum resident set size (kbytes):", output)

    def test_dangling_begin_event(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_string(
            """
{
  "displayTimeUnit": "ns",
  "traceEvents": [
    {
      "cat": "category",
      "name": "name",
      "ts": 0.0,
      "ph": "B",
      "tid": 0,
      "pid": 0
    }
  ],
  "systemTraceEvents": {
    "events": [],
    "type": "fuchsia"
  }
}
"""
        )
        self.assertEqual(len(list(model.all_events())), 0)

    def test_missing_end_events(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_string(
            """
{
  "displayTimeUnit": "ns",
  "traceEvents": [
    {
      "cat": "test",
      "name": "name",
      "ts": 0.0,
      "ph": "B",
      "tid": 0,
      "pid": 0
    },
    {
      "cat": "test",
      "name": "complete",
      "ts": 1.0,
      "pid": 0,
      "tid": 0,
      "ph": "X",
      "dur": 4.0
    },
    {
      "cat": "test",
      "name": "some_other_name",
      "ts": 3.0,
      "ph": "B",
      "tid": 0,
      "pid": 0
    },
    {
      "cat": "test",
      "name": "name",
      "ts": 8.0,
      "ph": "E",
      "tid": 0,
      "pid": 0
    }
  ],
  "systemTraceEvents": {
    "events": [],
    "type": "fuchsia"
  }
}
"""
        )
        self.assertEqual(len(list(model.all_events())), 2)
        # some_other_name is missing an end and will be dropped
        self.assertEqual(list(model.all_events())[0].name, "name")
        self.assertEqual(list(model.all_events())[1].name, "complete")

    def test_missing_begin_events(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_string(
            """
{
  "displayTimeUnit": "ns",
  "traceEvents": [
    {
      "cat": "category",
      "name": "name",
      "ts": 0.0,
      "ph": "B",
      "tid": 0,
      "pid": 0
    },
    {
      "cat": "category",
      "name": "some_other_name",
      "ts": 1.0,
      "ph": "E",
      "tid": 0,
      "pid": 0
    },
    {
      "cat": "category",
      "name": "some_other_other_name",
      "ts": 2.0,
      "ph": "E",
      "tid": 0,
      "pid": 0
    },
    {
      "cat": "category",
      "name": "name",
      "ts": 3.0,
      "ph": "E",
      "tid": 0,
      "pid": 0
    }
  ],
  "systemTraceEvents": {
    "events": [],
    "type": "fuchsia"
  }
}
"""
        )
        self.assertEqual(len(list(model.all_events())), 1)
        self.assertEqual(list(model.all_events())[0].name, "name")

    def test_missing_begin_events_recovery(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_string(
            """
{
  "displayTimeUnit": "ns",
  "traceEvents": [
    {
      "cat": "category",
      "name": "name",
      "ts": 0.0,
      "ph": "B",
      "tid": 0,
      "pid": 0
    },
    {
      "cat": "test",
      "name": "complete",
      "ts": 1.0,
      "pid": 0,
      "tid": 0,
      "ph": "X",
      "dur": 4.0
    },
    {
      "cat": "category",
      "name": "some_other_name",
      "ts": 2.0,
      "ph": "E",
      "tid": 0,
      "pid": 0
    },
    {
      "cat": "category",
      "name": "name",
      "ts": 9.0,
      "ph": "E",
      "tid": 0,
      "pid": 0
    }
  ],
  "systemTraceEvents": {
    "events": [],
    "type": "fuchsia"
  }
}
"""
        )
        self.assertEqual(len(list(model.all_events())), 2)
        self.assertEqual(list(model.all_events())[0].name, "name")
        self.assertEqual(list(model.all_events())[1].name, "complete")

    def test_integral_timestamp_and_duration(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_string(
            """
{
  "displayTimeUnit": "ns",
  "traceEvents": [
    {
      "cat": "test",
      "name": "integral",
      "ts": 12345,
      "pid": 35204,
      "tid": 323993,
      "ph": "X",
      "dur": 200
    }
  ],
  "systemTraceEvents": {
    "events": [],
    "type": "fuchsia"
  }
}
"""
        )
        self.assertNotEqual(len(list(model.all_events())), 0)

    def test_zero_length_duration_events(self) -> None:
        # This is a regression test for a bug (https://fxbug.dev/42082034) where
        # trace importing fails to correctly handle zero-length trace
        # durations with the ph='X' type.
        #
        # The bug arose because the trace importer sorted the trace events
        # using a non-stable sort (likely quicksort). We use an example input
        # here with a moderate number of trace events (100) because the bug
        # did not reproduce with a small number of trace events (such as 1 or
        # 10) but did reproduce with a larger number (such as 20).
        trace_json: Dict[str, Any] = {
            "displayTimeUnit": "ns",
            "traceEvents": [],
            "systemTraceEvents": {
                "events": [],
                "type": "fuchsia",
            },
        }
        for idx in range(100):
            trace_json["traceEvents"].append(
                {
                    "cat": "some_category",
                    "name": "some_event",
                    "ts": 1000 + idx,
                    "pid": 35204,
                    "tid": 323993,
                    "ph": "X",
                    "dur": 0,
                }
            )

        model = trace_importing.consume_json_to_create_model(trace_json)
        self.assertNotEqual(len(list(model.all_events())), 0)

    def test_preserve_ordering_same_start_time(self) -> None:
        # Test that the ordering of duration events is preserved when the
        # durations have the same start timestamp. The tests the case of
        # separate begin and end records (ph='B' and ph='E') in the input.
        #
        # This is a regression test for a bug (https://fxbug.dev/42082034). The
        # bug arose because the trace importer sorted the trace events using a
        # non-stable sort (likely quicksort). We use an example input here
        # with a moderate number of trace events (100) because the bug did
        # not reproduce with a small number of trace events (such as 1 or 10
        # or 20) but did reproduce with a larger number (such as 50).
        expected_names: List[str] = []
        trace_json: Dict[str, Any] = {
            "displayTimeUnit": "ns",
            "traceEvents": [],
            "systemTraceEvents": {
                "events": [],
                "type": "fuchsia",
            },
        }
        for idx in range(100):
            event_name: str = f"event_{idx}"
            trace_json["traceEvents"].append(
                {
                    "cat": "some_category",
                    "name": event_name,
                    "ts": 1000,
                    "pid": 35204,
                    "tid": 323993,
                    "ph": "B",
                }
            )
            trace_json["traceEvents"].append(
                {
                    "cat": "some_category",
                    "name": event_name,
                    "ts": 1000,
                    "pid": 35204,
                    "tid": 323993,
                    "ph": "E",
                }
            )
            expected_names.append(event_name)

        # Check that the events are imported with the expected ordering.
        model = trace_importing.consume_json_to_create_model(trace_json)
        self.assertEqual(
            [event.name for event in model.all_events()], expected_names
        )

    def test_flow_ids(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_file_path(
            os.path.join(self._runtime_deps_path, "flow_ids.json")
        )

        events: List[trace_model.Event] = list(model.all_events())
        self.assertEqual(len(events), 4)

        flow_events: List[trace_model.FlowEvent] = list(
            trace_utils.filter_events(events, type=trace_model.FlowEvent)
        )
        flow_events.sort(key=lambda x: x.start)
        self.assertEqual(len(flow_events), 3)
        self.assertIsNotNone(flow_events[0].next_flow)
        self.assertIsNotNone(flow_events[1].next_flow)
        self.assertIsNone(flow_events[2].next_flow)

    def test_flow_event_binding_points(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_file_path(
            os.path.join(self._runtime_deps_path, "flow_event_binding.json")
        )

        process: trace_model.Process = model.processes[0]
        thread: trace_model.Thread = process.threads[0]
        self.assertEqual(len(model.processes), 1)
        self.assertEqual(len(process.threads), 1)
        flow_events: List[trace_model.FlowEvent] = list(
            trace_utils.filter_events(thread.events, type=trace_model.FlowEvent)
        )
        self.assertEqual(len(flow_events), 6)
        self.assertEqual(
            flow_events[0]
            .enclosing_duration.start.to_epoch_delta()
            .to_milliseconds_f()
            if flow_events[0].enclosing_duration is not None
            else None,
            10.0,
        )
        self.assertEqual(
            flow_events[1]
            .enclosing_duration.start.to_epoch_delta()
            .to_milliseconds_f()
            if flow_events[1].enclosing_duration is not None
            else None,
            20.0,
        )
        self.assertEqual(
            flow_events[2]
            .enclosing_duration.start.to_epoch_delta()
            .to_milliseconds_f()
            if flow_events[2].enclosing_duration is not None
            else None,
            40.0,
        )
        self.assertEqual(
            flow_events[3]
            .enclosing_duration.start.to_epoch_delta()
            .to_milliseconds_f()
            if flow_events[3].enclosing_duration is not None
            else None,
            50.0,
        )
        self.assertEqual(
            flow_events[4]
            .enclosing_duration.start.to_epoch_delta()
            .to_milliseconds_f()
            if flow_events[4].enclosing_duration is not None
            else None,
            60.0,
        )
        self.assertEqual(
            flow_events[5]
            .enclosing_duration.start.to_epoch_delta()
            .to_milliseconds_f()
            if flow_events[5].enclosing_duration is not None
            else None,
            70.0,
        )

    def test_async_events_with_id2(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_file_path(
            os.path.join(self._runtime_deps_path, "id2_async.json")
        )
        self.assertEqual(len(model.processes), 2)
        self.assertEqual(
            len(
                list(
                    trace_utils.filter_events(
                        model.all_events(),
                        category="test",
                        name="async",
                        type=trace_model.AsyncEvent,
                    )
                )
            ),
            2,
        )
        self.assertEqual(
            len(
                list(
                    trace_utils.filter_events(
                        model.all_events(),
                        category="test",
                        name="async2",
                        type=trace_model.AsyncEvent,
                    )
                )
            ),
            2,
        )

    def test_chrome_metadata_events(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_file_path(
            os.path.join(self._runtime_deps_path, "chrome_metadata.json")
        )
        process: trace_model.Process = model.processes[0]
        thread: trace_model.Thread = process.threads[0]
        self.assertEqual(len(model.processes), 1)
        self.assertEqual(len(process.threads), 1)
        self.assertEqual(process.name, "Test process")
        self.assertEqual(thread.name, "Test thread")

    def test_process_has_all_threads(self) -> None:
        """
        Test that a process contains threads, including threads that don't
        have a trace event associated with them.
        """
        model: trace_model.Model = trace_importing.create_model_from_file_path(
            os.path.join(self._runtime_deps_path, "model.json")
        )
        # Check that each PID we found gets saved as a Process (even those without a name).
        self.assertEqual(
            [process.pid for process in model.processes], [7009, 7010, 7011]
        )

        # Check that each TID gets saved as a thread for the relevant process.
        # Threads that don't have a name get a generated name.
        process_7009 = next(p for p in model.processes if p.pid == 7009)
        threads = process_7009.threads

        # Includes the threads that don't appear in trace event list.
        # Does not include threads that have no mapping to PID. For example,
        # tid: 1037 appears in the JSON but there is no mapping to PID. This
        # gets ignored.
        self.assertEqual(len(threads), 3)

        # Thread with name, without trace events.
        self.assertEqual(threads[0].tid, 1036)
        self.assertEqual(threads[0].name, "memory-pressure-loop")
        self.assertEqual(len(threads[0].events), 0)

        # Thread without thread name gets autogenerated name.
        self.assertEqual(threads[1].tid, 7021)
        self.assertEqual(threads[1].name, "tid: 7021")
        self.assertEqual(len(threads[1].events), 4)

        # Regular Thread with name and event.
        self.assertEqual(threads[2].tid, 7022)
        self.assertEqual(threads[2].name, "initial-thread")
        self.assertEqual(len(threads[2].events), 9)

        # Thread whose Process's PID doesn't show up in trace events.
        process_7011 = next(p for p in model.processes if p.pid == 7011)
        threads = process_7011.threads
        self.assertEqual(threads[0].tid, 5555)
        self.assertEqual(threads[0].name, "initial-thread")
        self.assertEqual(len(threads[0].events), 0)

    def test_scheduling_events(self) -> None:
        model: trace_model.Model = trace_importing.create_model_from_file_path(
            os.path.join(self._runtime_deps_path, "sched_data.json")
        )
        self.assertEqual(len(model.scheduling_records), 1)
        self.assertEqual(len(model.scheduling_records[0]), 3)

        self.assertIsInstance(
            model.scheduling_records[0][0], trace_model.Waking
        )
        self.assertEqual(model.scheduling_records[0][0].start, TimePoint(1000))
        self.assertEqual(model.scheduling_records[0][0].tid, 1)
        self.assertEqual(model.scheduling_records[0][0].prio, 3122)

        self.assertIsInstance(
            model.scheduling_records[0][1], trace_model.ContextSwitch
        )
        assert isinstance(
            model.scheduling_records[0][1], trace_model.ContextSwitch
        )
        self.assertEqual(model.scheduling_records[0][1].start, TimePoint(5000))
        self.assertEqual(model.scheduling_records[0][1].tid, 2)
        self.assertEqual(model.scheduling_records[0][1].prio, 3122)
        self.assertEqual(model.scheduling_records[0][1].outgoing_tid, 1)
        self.assertEqual(model.scheduling_records[0][1].outgoing_prio, 3122)
        self.assertEqual(
            model.scheduling_records[0][1].outgoing_state,
            trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
        )

        self.assertIsInstance(
            model.scheduling_records[0][2], trace_model.ContextSwitch
        )
        self.assertEqual(
            model.scheduling_records[0][2].start,
            TimePoint(10000),
        )
        assert isinstance(
            model.scheduling_records[0][2], trace_model.ContextSwitch
        )
        self.assertEqual(model.scheduling_records[0][2].tid, 1036)
        self.assertEqual(model.scheduling_records[0][2].prio, -0x80000000)
        self.assertEqual(model.scheduling_records[0][2].outgoing_tid, 2)
        self.assertEqual(model.scheduling_records[0][2].outgoing_prio, 3122)
        self.assertEqual(
            model.scheduling_records[0][2].outgoing_state,
            trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
        )
        self.assertTrue(model.scheduling_records[0][2].is_idle())
