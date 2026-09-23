#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for agg_cpu_breakdown.py."""

import unittest
from typing import List

import trace_processing.trace_model as trace_model
import trace_processing.trace_time as trace_time
from trace_processing.metrics import agg_cpu_breakdown, cpu


class AggCpuBreakdownTest(unittest.TestCase):
    """Aggregated CPU breakdown tests."""

    def construct_trace_model(self) -> trace_model.Model:
        model = trace_model.Model((), {})
        threads: List[trace_model.Thread] = []
        for i in range(1, 5):
            threads.append(trace_model.Thread(i, "thread-%d" % i))
        model.processes = [
            # Process with PID 1000 and threads with TIDs 1, 2, 3, 4.
            trace_model.Process(1000, "big_process", threads),
            # Process with PID 2000 and thread with TID 100.
            trace_model.Process(
                2000, "small_process", [trace_model.Thread(100, "small-thread")]
            ),
        ]

        # CPU 0.
        # Total time: 0 to 9100000000 = 9100 ms.
        records_0: List[trace_model.SchedulingRecord] = [
            # "thread-0" is active from 0 - 1800 ms, 1800 total duration.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                1,
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(1800000000),
                100,
                1,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "small-thread" is active from 1800 to 2300 ms, 500 total duration.
            trace_model.ContextSwitch(
                trace_time.TimePoint(2300000000),
                1,
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "thread-2" is active from 3500-9100 ms, 5000 total duration.
            # Switch the order of adding to records - shouldn't change behavior.
            trace_model.ContextSwitch(
                trace_time.TimePoint(9100000000),
                100,
                2,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(3500000000),
                2,
                70,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.Waking(trace_time.TimePoint(3500000000), 2, 612, {}),
        ]

        # CPU 2.
        # Total time: 0 to 8000000000 = 8000 ms.
        records_2: List[trace_model.SchedulingRecord] = [
            # "thread-1" is active from 0 - 1500 ms, 1500 total duration.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                1,
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(1500000000),
                100,
                1,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "small-thread" is active from 1500 to 2000 ms, 500 total duration.
            trace_model.ContextSwitch(
                trace_time.TimePoint(2000000000),
                1,
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "thread-2" is active from 3000-8000 ms, 5000 total duration.
            # Switch the order of adding to records - shouldn't change behavior.
            trace_model.ContextSwitch(
                trace_time.TimePoint(8000000000),
                100,
                2,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(3000000000),
                2,
                70,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.Waking(trace_time.TimePoint(3000000000), 2, 612, {}),
        ]

        # CPU 3.
        # Total time: 3000000000 to 6000000000 = 3000 ms.
        records_3: List[trace_model.SchedulingRecord] = [
            # "thread-3" (incoming) is idle; don't log it in breakdown,
            # but do log it in the total CPU duration for CPU 3.
            trace_model.ContextSwitch(
                trace_time.TimePoint(3000000000),
                3,
                70,
                trace_model.INT32_MIN,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "thread-3" (outgoing) is idle; don't log it.
            trace_model.ContextSwitch(
                trace_time.TimePoint(4000000000),
                70,
                3,
                trace_model.INT32_MIN,
                trace_model.INT32_MIN,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # TID 70 (outgoing) is idle; don't log it in breakdown,
            # but do log it in the total CPU duration for CPU 3.
            # "thread-2" (incoming) is not idle.
            trace_model.ContextSwitch(
                trace_time.TimePoint(5000000000),
                2,
                70,
                612,
                trace_model.INT32_MIN,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "thread-2" (outgoing) is not idle; log it.
            trace_model.ContextSwitch(
                trace_time.TimePoint(6000000000),
                3,
                2,
                trace_model.INT32_MIN,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
        ]

        # CPU 5.
        # Total time: 500000000 to 6000000000 = 5500
        records_5: List[trace_model.SchedulingRecord] = []
        # Add 5 Waking and ContextSwitch events for "thread-1" where the incoming_tid
        # is 1 and the outgoing_tid (70) is not mapped to a Thread in our Process.
        for i in range(1, 7):
            records_5.append(
                trace_model.Waking(
                    trace_time.TimePoint(i * 1000000000 - 500000000),
                    1,
                    3122,
                    {},
                )
            )
            records_5.append(
                trace_model.ContextSwitch(
                    trace_time.TimePoint(i * 1000000000 - 500000000),
                    1,
                    70,
                    3122,
                    3122,
                    trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    {},
                )
            )

            # Add 5 Waking events for the tid = 70 Thread.
            records_5.append(
                trace_model.Waking(
                    trace_time.TimePoint(i * 1000000000), 70, 3122, {}
                )
            )
            # Add 5 ContextSwitch events for "thread-1" where the outgoing_tid is 1.
            # "thread-1" is active from i*500 to i*1000, i.e. 500 ms increments. This happens 5 times = 2500 ms.
            records_5.append(
                trace_model.ContextSwitch(
                    trace_time.TimePoint(i * 1000000000),
                    70,
                    1,
                    3122,
                    3122,
                    trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    {},
                )
            )

        model.scheduling_records = {
            0: records_0,
            2: records_2,
            3: records_3,
            5: records_5,
        }
        return model

    def test_aggregate_metrics(self) -> None:
        model = self.construct_trace_model()

        self.assertEqual(model.processes[0].name, "big_process")
        self.assertEqual(len(model.processes[0].threads), 4)
        self.assertEqual(model.processes[1].name, "small_process")
        self.assertEqual(len(model.processes[1].threads), 1)

        processor = cpu.CpuMetricsProcessor()
        breakdown, total_time = processor.process_metrics_and_get_total_time(
            model
        )
        print("total_time: %f" % total_time)
        agg_processor = agg_cpu_breakdown.AggCpuBreakdownMetricsProcessor(
            {0: 1.8, 2: 2.2, 3: 2.2, 5: 2.2}, total_time
        )
        agg_breakdown = agg_processor.aggregate_metrics(breakdown)

        self.assertEqual(len(agg_breakdown), 2)
        self.assertEqual(len(agg_breakdown[1.8]), 3)
        self.assertEqual(len(agg_breakdown[2.2]), 3)

        # Each process: thread has the correct numbers for each CPU.
        # Sorted by descending cpu and descending percent.
        # Note that neither thread-3 nor thread-4 are logged because
        # they are idle or there is no duration.
        self.assertEqual(
            agg_breakdown,
            {
                2.2: [
                    {
                        "process_name": "big_process",
                        "thread_name": "thread-2",
                        "percent": 65.934,
                        "duration": 6000.0,
                    },
                    {
                        "process_name": "big_process",
                        "thread_name": "thread-1",
                        "percent": 49.451,
                        "duration": 4500.0,
                    },
                    {
                        "process_name": "small_process",
                        "thread_name": "small-thread",
                        "percent": 5.495,
                        "duration": 500.0,
                    },
                ],
                1.8: [
                    {
                        "process_name": "big_process",
                        "thread_name": "thread-2",
                        "percent": 61.538,
                        "duration": 5600.0,
                    },
                    {
                        "process_name": "big_process",
                        "thread_name": "thread-1",
                        "percent": 19.78,
                        "duration": 1800.0,
                    },
                    {
                        "process_name": "small_process",
                        "thread_name": "small-thread",
                        "percent": 5.495,
                        "duration": 500.0,
                    },
                ],
            },
        )
