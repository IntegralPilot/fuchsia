#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for memory.py."""

import unittest

from reporting import metrics as reporting_metrics
from trace_processing import trace_model, trace_time
from trace_processing.metrics import memory


class MemoryTest(unittest.TestCase):
    @staticmethod
    def construct_trace_model(event_count: int) -> trace_model.Model:
        power_events: list[trace_model.Event] = [
            trace_model.CounterEvent.consume_dict(
                {
                    "cat": "memory:kernel",
                    "name": "kmem_stats_a",
                    "pid": 0x8C01_1EC7_EDDA_7A10,
                    "tid": 0x8C01_1EC7_EDDA_7A20,
                    "ts": 500000 + i,  # microseconds
                    "args": {
                        "total_memory": 1000 + i,
                        "free_bytes": 200 + i,
                        "stall_time_some_ns": 10 + i,
                        "page_refaults": 100 + i,
                    },
                }
            )
            for i in range(0, event_count)
        ]

        power_process = trace_model.Process(
            0x8C01_1EC7_EDDA_7A10,
            "MemoryData",
            [
                trace_model.Thread(
                    0x8C01_1EC7_EDDA_7A20,
                    "Fake",
                    power_events,
                ),
            ],
        )

        eviction_thread_tid = 300
        eviction_thread = trace_model.Thread(
            eviction_thread_tid, "eviction-thread"
        )
        scanner_request_thread_tid = 400
        scanner_request_thread = trace_model.Thread(
            scanner_request_thread_tid, "scanner-request-thread"
        )
        scheduling_records: dict[int, list[trace_model.SchedulingRecord]] = {
            0: [
                trace_model.ContextSwitch(
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(1000)
                    ),
                    incoming_tid=eviction_thread_tid,
                    outgoing_tid=15,
                    incoming_prio=None,
                    outgoing_prio=None,
                    outgoing_state=trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    args={},
                ),
                trace_model.ContextSwitch(
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(1400)
                    ),
                    incoming_tid=scanner_request_thread_tid,
                    outgoing_tid=eviction_thread_tid,
                    incoming_prio=None,
                    outgoing_prio=None,
                    outgoing_state=trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    args={},
                ),
                trace_model.ContextSwitch(
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(1500)
                    ),
                    incoming_tid=19,
                    outgoing_tid=scanner_request_thread_tid,
                    incoming_prio=None,
                    outgoing_prio=None,
                    outgoing_state=trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    args={},
                ),
            ]
        }

        processes = [
            power_process,
            trace_model.Process(
                1000,
                "load_generator.cm",
                [trace_model.Thread(1, "load-generator-thread-1")],
            ),
            trace_model.Process(
                0, "kernel", [eviction_thread, scanner_request_thread]
            ),
        ]

        return trace_model.Model(processes, scheduling_records)

    def test_process_metrics(self) -> None:
        model = self.construct_trace_model(100)
        metrics = memory.MemoryMetricsProcessor().process_metrics(model)
        self.assertEqual(
            metrics,
            [
                reporting_metrics.TestCaseResult(
                    label="Memory/System/StallTimeSome",
                    unit=reporting_metrics.Unit.nanoseconds,
                    values=(99,),
                ),
                reporting_metrics.TestCaseResult(
                    label="Memory/System/PageRefaults",
                    unit=reporting_metrics.Unit.count,
                    values=(99,),
                ),
                reporting_metrics.TestCaseResult(
                    label="Memory/System/ManagementTime",
                    unit=reporting_metrics.Unit.milliseconds,
                    values=(0.5,),
                    direction=reporting_metrics.Direction.smallerIsBetter,
                ),
            ],
        )

    def test_process_freeform_metrics(self) -> None:
        processor = memory.MemoryMetricsProcessor()
        name, metrics = processor.process_freeform_metrics(
            self.construct_trace_model(100)
        )
        self.assertEqual(name, processor.FREEFORM_METRICS_FILENAME)
        self.assertEqual(
            metrics,
            {
                "kernel": {
                    "total_memory": {
                        "P5": 1004.95,
                        "P25": 1024.75,
                        "P50": 1049.5,
                        "P75": 1074.25,
                        "P95": 1094.05,
                        "Min": 1000,
                        "Max": 1099,
                        "Average": 1049.5,
                    },
                    "free_bytes": {
                        "P5": 204.95,
                        "P25": 224.75,
                        "P50": 249.5,
                        "P75": 274.25,
                        "P95": 294.05,
                        "Min": 200,
                        "Max": 299,
                        "Average": 249.5,
                    },
                    "stall_time_some_ns": {
                        "Delta": 99,
                        "Rate": 0.001,
                    },
                    "page_refaults": {
                        "Delta": 99,
                        "Rate": 0.001,
                    },
                }
            },
        )

    def test_process_single_sample(self) -> None:
        processor = memory.MemoryMetricsProcessor()
        name, metrics = processor.process_freeform_metrics(
            self.construct_trace_model(1)
        )
        self.assertEqual(name, processor.FREEFORM_METRICS_FILENAME)
        print(metrics)
        self.assertEqual(
            metrics,
            {
                "kernel": {
                    "total_memory": {
                        "P5": 1000.0,
                        "P25": 1000.0,
                        "P50": 1000.0,
                        "P75": 1000.0,
                        "P95": 1000.0,
                        "Min": 1000,
                        "Max": 1000,
                        "Average": 1000,
                    },
                    "free_bytes": {
                        "P5": 200.0,
                        "P25": 200.0,
                        "P50": 200.0,
                        "P75": 200.0,
                        "P95": 200.0,
                        "Min": 200,
                        "Max": 200,
                        "Average": 200,
                    },
                    "stall_time_some_ns": {"Delta": 0, "Rate": None},
                    "page_refaults": {"Delta": 0, "Rate": None},
                }
            },
        )
