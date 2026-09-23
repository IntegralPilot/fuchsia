# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for trace_importing_fxt.py."""

import contextlib
import pathlib
import unittest

from tp_shell import PerfettoTraceProcessor
from trace_processing import (
    trace_importing_fxt,
    trace_model,
)


class QueriesTest(unittest.TestCase):
    """Dedicated unit test suite validating trace query helper methods on _FxtImporter."""

    _exit_stack: contextlib.ExitStack
    session: PerfettoTraceProcessor
    importer: trace_importing_fxt._FxtImporter

    @classmethod
    def setUpClass(cls) -> None:
        cls._exit_stack = contextlib.ExitStack()
        fxt_path = (
            pathlib.Path(__file__).resolve().parent.parent.parent
            / "runtime_deps"
            / "model.fxt"
        )
        cls.session = cls._exit_stack.enter_context(
            PerfettoTraceProcessor(trace_path=str(fxt_path))
        )
        cls.importer = trace_importing_fxt._FxtImporter(cls.session)

    @classmethod
    def tearDownClass(cls) -> None:
        cls._exit_stack.close()

    def test_get_slices_with_filtering(self) -> None:
        # 1. Filter by pattern
        importer_filtered = trace_importing_fxt._FxtImporter(
            self.session, patterns={"Read"}
        )
        events, _, _ = importer_filtered.get_slices()
        self.assertGreater(len(events), 0)
        for event in events:
            self.assertIn("Read", event.name)

        # 2. Filter by category
        all_events, _, _ = self.importer.get_slices()
        target_cat = all_events[0].category
        importer_cat = trace_importing_fxt._FxtImporter(
            self.session, categories={target_cat}
        )
        cat_events, _, _ = importer_cat.get_slices()
        self.assertGreater(len(cat_events), 0)
        for event in cat_events:
            self.assertEqual(event.category, target_cat)

        # 3. Empty pattern set (discards all events)
        importer_empty = trace_importing_fxt._FxtImporter(
            self.session, patterns=set()
        )
        empty_events, _, _ = importer_empty.get_slices()
        self.assertEqual(len(empty_events), 0)

    def test_get_pid_to_name(self) -> None:
        processes = self.importer.get_pid_to_name()
        # Verify process mapping resolved actual Fuchsia processes from trace
        self.assertGreater(len(processes.pid_to_name), 0)
        self.assertIn("process_foo", processes.pid_to_name.values())

    def test_get_threads(self) -> None:
        tid_to_name, tid_to_pid = self.importer.get_threads()
        # Verify thread names mapped correctly
        self.assertGreater(len(tid_to_name), 0)
        self.assertIn("initial-thread", tid_to_name.values())

    def test_get_args_map(self) -> None:
        arg_set_id_to_args = self.importer._get_args_map()
        self.assertGreater(len(arg_set_id_to_args), 0)
        found_bool = False
        for args in arg_set_id_to_args.values():
            if "trace_id_is_process_scoped" in args:
                val = args["trace_id_is_process_scoped"]
                self.assertEqual(val, 0)
                found_bool = True
        self.assertTrue(found_bool)

    def test_get_slices(self) -> None:
        (
            result_events,
            slice_id_to_event,
            slice_parent_map,
        ) = self.importer.get_slices()

        self.importer._restore_slice_hierarchy(
            slice_parent_map, slice_id_to_event
        )
        self.assertGreater(len(result_events), 0)
        slice_names = [e.name for e in result_events]
        self.assertIn("Read", slice_names)

        async_events = [
            e for e in result_events if isinstance(e, trace_model.AsyncEvent)
        ]
        self.assertGreater(len(async_events), 0)
        self.assertIsInstance(async_events[0], trace_model.AsyncEvent)

    def test_get_counter_events(self) -> None:
        counters = self.importer.get_counter_events()
        self.assertGreater(len(counters.events), 0)
        self.assertIsInstance(counters.events[0], trace_model.CounterEvent)
        self.assertEqual(counters.events[0].name, "cpu_usage")

    def test_get_scheduling_records(self) -> None:
        scheduling = self.importer.get_scheduling_records()
        # Verify context switches scheduling tracks populated with actual trace records
        self.assertIn(0, scheduling.records)
        self.assertGreater(len(scheduling.records[0]), 0)
        first_switch = scheduling.records[0][0]
        self.assertIsInstance(first_switch, trace_model.ContextSwitch)

    def test_get_flow_events(self) -> None:
        (
            _,
            slice_id_to_event,
            slice_parent_map,
        ) = self.importer.get_slices()
        flows = self.importer.get_flow_events(
            slice_id_to_event, slice_parent_map
        )

        flow_events = [
            e for e in flows.events if isinstance(e, trace_model.FlowEvent)
        ]
        self.assertGreater(len(flow_events), 0)
        self.assertIsNotNone(flow_events[0].id)

    def test_get_flow_events_with_category_filtering(self) -> None:
        all_slices = self.importer.get_slices()
        target_cat = all_slices.events[0].category
        filtered_importer = trace_importing_fxt._FxtImporter(
            self.session, categories={target_cat}
        )
        filtered_slices = filtered_importer.get_slices()
        flows = filtered_importer.get_flow_events(
            filtered_slices.id_to_event, filtered_slices.parent_map
        )
        flow_events = [
            e for e in flows.events if isinstance(e, trace_model.FlowEvent)
        ]
        self.assertGreater(len(flow_events), 0)
        has_enclosing = any(
            f.enclosing_duration is not None for f in flow_events
        )
        self.assertTrue(has_enclosing)

    def test_tp_session_query_and_create_model(self) -> None:
        fxt_path = (
            pathlib.Path(__file__).resolve().parent.parent.parent
            / "runtime_deps"
            / "model.fxt"
        )
        with PerfettoTraceProcessor(trace_path=str(fxt_path)) as session:
            # Query directly against the trace session without building a model
            res = session.run_query("SELECT count(*) as cnt FROM slice")
            self.assertEqual(len(res), 1)
            self.assertGreater(res[0]["cnt"], 0)

            # Build model from existing session without closing the session
            model = trace_importing_fxt.create_model_from_tp_session(session)
            self.assertGreater(len(list(model.all_events())), 0)

            # Verify session is still active after building model
            res2 = session.run_query("SELECT count(*) as cnt FROM slice")
            self.assertEqual(res, res2)

        # After context manager exits, querying closed session raises RuntimeError
        with self.assertRaises(RuntimeError):
            session.run_query("SELECT count(*) FROM slice")
