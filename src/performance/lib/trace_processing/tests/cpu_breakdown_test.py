#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for cpu.py."""

import unittest
from typing import cast

from trace_processing import trace_model, trace_time
from trace_processing.metrics import cpu


class CpuBreakdownTest(unittest.TestCase):
    """CPU breakdown tests."""

    def construct_trace_model(self) -> trace_model.Model:
        threads = [trace_model.Thread(i, f"thread-{i}") for i in range(1, 5)]
        processes = [
            # Process with PID 1000 and threads with TIDs 1, 2, 3, 4.
            trace_model.Process(1000, "big_process", threads),
            # Process with PID 2000 and thread with TID 100.
            trace_model.Process(
                2000, "small_process", [trace_model.Thread(100, "small-thread")]
            ),
        ]

        # CPU 2.
        # Total time: 0 to 8000000000 = 8000 ms.
        records_2: list[trace_model.SchedulingRecord] = [
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
        records_3: list[trace_model.SchedulingRecord] = [
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
        records_5: list[trace_model.SchedulingRecord] = []
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

        scheduling_records = {2: records_2, 3: records_3, 5: records_5}

        # Simulate processing rate shifts on CPU 2
        power_configs = [
            # "thread-1" is active from 0 - 1500 ms.
            (0, 1000),
            (400000, 798),
            (800000, 506),
            (1200000, 360),
            # "small-thread" is active from 1500 - 2000 ms.
            (1500000, 1000),
            (1600000, 798),
            (1700000, 506),
            (1800000, 360),
            # Idle/Skipped from 2000 - 3000 ms.
            (2000000, 506),
            # "thread-2" is active from 3000 - 8000 ms.
            (3000000, 798),
            (4000000, 1000),
            (5000000, 798),
            (6000000, 506),
            (7000000, 360),
        ]
        power_events = [
            trace_model.CounterEvent.consume_dict(
                {
                    "cat": "kernel:power",
                    "name": "Processing Rate:2",
                    "id": "2",
                    "pid": 0,
                    "tid": 0,
                    "ts": ts,
                    "args": {"CPU": rate},
                }
            )
            for ts, rate in power_configs
        ]

        dvfs_process = trace_model.Process(
            9999,
            "KernelPower",
            [
                trace_model.Thread(
                    9999, "DVFS", cast(list[trace_model.Event], power_events)
                )
            ],
        )
        processes.append(dvfs_process)

        return trace_model.Model(processes, scheduling_records)

    def construct_trace_model_with_skips(self) -> trace_model.Model:
        processes = [
            trace_model.Process(
                1000,
                "process",
                [
                    trace_model.Thread(1, "thread-1"),
                    trace_model.Thread(2, "thread-2"),
                    trace_model.Thread(3, "thread-3"),
                ],
            )
        ]
        scheduling_records = {
            2: [
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
                trace_model.Waking(
                    trace_time.TimePoint(3000000000), 2, 612, {}
                ),
                # Missing transition from thread 1 to 2. Log this skip of 1500 ms.
                trace_model.ContextSwitch(
                    trace_time.TimePoint(3000000000),
                    1,
                    2,
                    612,
                    612,
                    trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    {},
                ),
                trace_model.Waking(
                    trace_time.TimePoint(4000000000), 3, 612, {}
                ),
                # Missing transition from thread 3 to 2. Log this skip of 1000 ms.
                trace_model.ContextSwitch(
                    trace_time.TimePoint(4000000000),
                    3,
                    2,
                    612,
                    612,
                    trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    {},
                ),
                # Correct transition from thread 2 to 3, 1000 total duration.
                trace_model.ContextSwitch(
                    trace_time.TimePoint(5000000000),
                    70,
                    3,
                    612,
                    612,
                    trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                    {},
                ),
            ]
        }
        return trace_model.Model(processes, scheduling_records)

    def test_process_metrics(self) -> None:
        model = self.construct_trace_model()

        self.assertEqual(model.processes[0].name, "big_process")
        self.assertEqual(len(model.processes[0].threads), 4)
        self.assertEqual(model.processes[1].name, "small_process")
        self.assertEqual(len(model.processes[1].threads), 1)

        processor = cpu.CpuMetricsProcessor()
        name, breakdown = processor.process_freeform_metrics(model)
        self.assertEqual(name, processor.FREEFORM_METRICS_FILENAME)

        self.assertEqual(len(breakdown), 5)

        # Make it easier to compare breakdown results.
        for b in breakdown:
            b["percent"] = round(cast(float, b["percent"]), 3)
            if "normalized_percent" in b:
                b["normalized_percent"] = round(
                    cast(float, b["normalized_percent"]), 3
                )

        # Each process: thread has the correct numbers for each CPU.
        # Sorted by descending cpu and descending percent.
        # Note that neither thread-3 nor thread-4 are logged because
        # they are idle or there is no duration.
        self.assertEqual(
            breakdown,
            [
                {
                    "process_name": "big_process",
                    "thread_name": "thread-1",
                    "tid": 1,
                    "cpu": 5,
                    "percent": 54.545,
                    "duration": 3000.0,
                    "normalized_percent": 54.545,
                    "normalized_duration": 3000.0,
                    "duration_per_rate": {"1000": 3000.0},
                },
                {
                    "process_name": "big_process",
                    "thread_name": "thread-2",
                    "tid": 2,
                    "cpu": 3,
                    "percent": 33.333,
                    "duration": 1000.0,
                    "normalized_percent": 33.333,
                    "normalized_duration": 1000.0,
                    "duration_per_rate": {"1000": 1000.0},
                },
                {
                    "process_name": "big_process",
                    "thread_name": "thread-2",
                    "tid": 2,
                    "cpu": 2,
                    "percent": 62.5,
                    "duration": 5000.0,
                    "normalized_percent": 65.321,
                    "normalized_duration": 3462.0,
                    "duration_per_rate": {
                        "1000": 1000.0,
                        "798": 2000.0,
                        "506": 1000.0,
                        "360": 1000.0,
                    },
                },
                {
                    "process_name": "big_process",
                    "thread_name": "thread-1",
                    "tid": 1,
                    "cpu": 2,
                    "percent": 18.75,
                    "duration": 1500.0,
                    "normalized_percent": 19.426,
                    "normalized_duration": 1029.6,
                    "duration_per_rate": {
                        "1000": 400.0,
                        "798": 400.0,
                        "506": 400.0,
                        "360": 300.0,
                    },
                },
                {
                    "process_name": "small_process",
                    "thread_name": "small-thread",
                    "tid": 100,
                    "cpu": 2,
                    "percent": 6.25,
                    "duration": 500.0,
                    "normalized_percent": 5.706,
                    "normalized_duration": 302.4,
                    "duration_per_rate": {
                        "1000": 100.0,
                        "798": 100.0,
                        "506": 100.0,
                        "360": 200.0,
                    },
                },
            ],
        )

    def test_group_by_process_name(self) -> None:
        model = self.construct_trace_model()

        processor = cpu.CpuMetricsProcessor()
        name, breakdown = processor.process_freeform_metrics(model)
        self.assertEqual(name, processor.FREEFORM_METRICS_FILENAME)
        consolidated_breakdown = cpu.group_by_process_name(breakdown)
        self.assertEqual(len(consolidated_breakdown), 4)

        # Make it easier to compare breakdown results.
        for b in consolidated_breakdown:
            b["percent"] = round(cast(float, b["percent"]), 3)
            if "normalized_percent" in b:
                b["normalized_percent"] = round(
                    cast(float, b["normalized_percent"]), 3
                )

        # Each process has been consolidated.
        # Sorted by descending cpu and descending percent.
        self.assertEqual(
            consolidated_breakdown,
            [
                {
                    "process_name": "big_process",
                    "cpu": 5,
                    "percent": 54.545,
                    "duration": 3000.0,
                    "normalized_percent": 54.545,
                    "normalized_duration": 3000.0,
                    "duration_per_rate": {"1000": 3000.0},
                },
                {
                    "process_name": "big_process",
                    "cpu": 3,
                    "percent": 33.333,
                    "duration": 1000.0,
                    "normalized_percent": 33.333,
                    "normalized_duration": 1000.0,
                    "duration_per_rate": {"1000": 1000.0},
                },
                {
                    "process_name": "big_process",
                    "cpu": 2,
                    "percent": 81.25,
                    "duration": 6500.0,
                    "normalized_percent": 84.747,
                    "normalized_duration": 4491.6,
                    "duration_per_rate": {
                        "1000": 1400.0,
                        "798": 2400.0,
                        "506": 1400.0,
                        "360": 1300.0,
                    },
                },
                {
                    "process_name": "small_process",
                    "cpu": 2,
                    "percent": 6.25,
                    "duration": 500.0,
                    "normalized_percent": 5.706,
                    "normalized_duration": 302.4,
                    "duration_per_rate": {
                        "1000": 100.0,
                        "798": 100.0,
                        "506": 100.0,
                        "360": 200.0,
                    },
                },
            ],
        )

    def test_process_metrics_with_skips(self) -> None:
        model = self.construct_trace_model_with_skips()

        self.assertEqual(model.processes[0].name, "process")
        self.assertEqual(len(model.processes[0].threads), 3)

        # Keep logs for skipped records logging.
        with self.assertLogs(cpu._LOGGER, level="WARNING") as context_manager:
            processor = cpu.CpuMetricsProcessor()
            name, breakdown = processor.process_freeform_metrics(model)
            self.assertEqual(name, processor.FREEFORM_METRICS_FILENAME)

        self.assertEqual(len(breakdown), 2)

        # Each process: thread has the correct numbers for each CPU.
        self.assertEqual(
            breakdown,
            [
                {
                    "process_name": "process",
                    "thread_name": "thread-1",
                    "tid": 1,
                    "cpu": 2,
                    "percent": 30.0,
                    "duration": 1500.0,
                    "normalized_percent": 30.0,
                    "normalized_duration": 1500.0,
                    "duration_per_rate": {"1000": 1500.0},
                },
                {
                    "process_name": "process",
                    "thread_name": "thread-3",
                    "tid": 3,
                    "cpu": 2,
                    "percent": 20.0,
                    "duration": 1000.0,
                    "normalized_percent": 20.0,
                    "normalized_duration": 1000.0,
                    "duration_per_rate": {"1000": 1000.0},
                },
            ],
        )

        # Skipped records are logged.
        self.assertRegex(
            context_manager.output[0],
            r"Possibly missing ContextSwitch "
            r"record\(s\) in trace for these CPUs and durations: {2: 2500.0}",
        )

    def test_cpu_processing_rate_timeline_with_rates(self) -> None:
        rates = [
            cpu.ProcessingRateSample(timestamp_ms=100.0, rate=1000.0),
            cpu.ProcessingRateSample(timestamp_ms=200.0, rate=798.0),
            cpu.ProcessingRateSample(timestamp_ms=300.0, rate=506.0),
            cpu.ProcessingRateSample(timestamp_ms=400.0, rate=360.0),
        ]
        timeline = cpu.CpuProcessingRateTimeline(rates)

        slices = timeline.get_virtual_slices(0.0, 500.0)
        self.assertEqual(len(slices), 5)

        # 0.0 -> 100.0: rate=1000.0 (default before first change)
        self.assertEqual(
            slices[0],
            cpu.VirtualProcessingSlice(duration=100.0, rate=1000.0),
        )
        self.assertEqual(
            slices[1], cpu.VirtualProcessingSlice(duration=100.0, rate=1000.0)
        )
        self.assertEqual(
            slices[2], cpu.VirtualProcessingSlice(duration=100.0, rate=798.0)
        )
        self.assertEqual(
            slices[3], cpu.VirtualProcessingSlice(duration=100.0, rate=506.0)
        )
        self.assertEqual(
            slices[4], cpu.VirtualProcessingSlice(duration=100.0, rate=360.0)
        )
