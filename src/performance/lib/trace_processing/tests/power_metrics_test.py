#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for ../metrics/power.py."""

import collections.abc
import unittest
from collections.abc import Sequence
from typing import Any, Iterable

from reporting import metrics
from trace_processing import trace_model, trace_time
from trace_processing.metrics import power

# Boilerplate-busting constants:
U = metrics.Unit
TestCaseResult = metrics.TestCaseResult


class PowerMetricsTest(unittest.TestCase):
    """Power metrics tests."""

    def assertEmpty(
        self, c: collections.abc.Container[Any], msg: Any = None
    ) -> None:
        self.assertFalse(c, msg)

    def construct_trace_processes(
        self,
        loadgen_tids: Iterable[int],
        power_events_stop_at: trace_time.TimePoint | None = None,
    ) -> Sequence[trace_model.Process]:
        """Builds a fake trace model.

        Args:
            loadgen_tids: The load generator process will have threads with these IDs.

        The fake returned by this method contains a load generator process with the specified
        number of threads, using the provided tids. It also contains a fake process containing
        power data held in CounterEvents. Voltage is always 12V.

        The timeline of power events is:
        500000µs, 1000mA --- 750000µs, 2000mA --- 1000000µs, 100mA --- 1250000, 600mA

        Tests should populate `model.scheduling_records` in order to orchestrate the desired
        interactions between load generation and other threads on the system.
        """
        event_template = {
            "cat": "Metrics",
            "name": "Metrics",
            "pid": 0x8C01_1EC7_EDDA_7A10,
            "tid": 0x8C01_1EC7_EDDA_7A20,
        }

        power_events: list[trace_model.Event] = list(
            filter(
                lambda e: (
                    power_events_stop_at is None
                    or e.start < power_events_stop_at
                ),
                [
                    trace_model.CounterEvent.consume_dict(
                        {  # during sync signal
                            **event_template,
                            "ts": 500000,  # microseconds
                            "args": {"Voltage": 12, "Current": 1000},
                        }
                    ),
                    trace_model.CounterEvent.consume_dict(
                        {  # during sync signal
                            **event_template,
                            "ts": 750000,  # microseconds
                            "args": {"Voltage": 12, "Current": 2000},
                        }
                    ),
                    trace_model.CounterEvent.consume_dict(
                        {
                            **event_template,
                            "ts": 1000000,  # microseconds
                            "args": {"Voltage": 12, "Current": 100},
                        }
                    ),
                    trace_model.CounterEvent.consume_dict(
                        {
                            **event_template,
                            "ts": 1250000,  # microseconds
                            "args": {"Voltage": 12, "Current": 600},
                        }
                    ),
                ],
            )
        )

        fake_power_process = trace_model.Process(
            0x8C01_1EC7_EDDA_7A10,
            "PowerData",
            [
                trace_model.Thread(
                    0x8C01_1EC7_EDDA_7A20,
                    "Fake",
                    power_events,
                ),
            ],
        )

        threads = [trace_model.Thread(i, f"thread-{i}") for i in loadgen_tids]
        return [
            # load_generator process with PID 1000 and threads with TIDs 1, 2.
            trace_model.Process(1000, "load_generator.cm", threads),
            fake_power_process,
        ]

    def test_process_metrics(self) -> None:
        """Correctly exclude power readings occurring during synchronization."""
        threads = (1,)
        processes = self.construct_trace_processes(threads)

        records_0: list[trace_model.SchedulingRecord] = [
            # "thread-1" is active from 0 - 1000, then exits.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                threads[0],
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(1000000000),
                70,
                threads[0],
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
        ]

        records_1: list[trace_model.SchedulingRecord] = [
            # small-thread
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                100,
                70,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
        ]

        model = trace_model.Model(processes, {0: records_0, 1: records_1})

        results = power.PowerMetricsProcessor().process_metrics(model)
        # Power samples should start to count the instant load generation stops, so expect
        # to count the .1A and .6A sample
        desc_base = power._AggregateMetrics.DESCRIPTION_BASE
        expected_results = [
            TestCaseResult("MinPower", U.watts, [1.2], f"{desc_base}, minimum"),
            TestCaseResult("MeanPower", U.watts, [4.2], f"{desc_base}, mean"),
            TestCaseResult("MaxPower", U.watts, [7.2], f"{desc_base}, maximum"),
        ]
        self.assertEqual(expected_results, results)

    def test_sync_multithread(self) -> None:
        """Detect sync happening across multiple CPUs."""
        (t_1, t_2, t_3) = (1, 2, 3)
        processes = self.construct_trace_processes((t_1, t_2, t_3))

        records_0: list[trace_model.SchedulingRecord] = [
            # "thread-1" is active from 0 - 500, then exits.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                t_1,
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "thread-3" starts waiting to run on this processor at 450
            trace_model.Waking(trace_time.TimePoint(450000000), t_3, 612, {}),
            # "thread-3" takes over from 500 - 1000, then exits
            trace_model.ContextSwitch(
                trace_time.TimePoint(500000000),
                t_3,
                t_1,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(1000000000),
                9999,
                t_3,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
        ]

        records_1: list[trace_model.SchedulingRecord] = [
            # Some other thread is active from 0 - 250, then blocks.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                9999,
                8888,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            # "thread-2" is active from 250 - 750, then exits
            trace_model.ContextSwitch(
                trace_time.TimePoint(250000000),
                t_2,
                9999,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(750000000),
                8888,
                t_2,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
        ]

        model = trace_model.Model(processes, {0: records_0, 1: records_1})

        results = power.PowerMetricsProcessor().process_metrics(model)
        desc_base = power._AggregateMetrics.DESCRIPTION_BASE
        expected_results = [
            TestCaseResult("MinPower", U.watts, [1.2], f"{desc_base}, minimum"),
            TestCaseResult("MeanPower", U.watts, [4.2], f"{desc_base}, mean"),
            TestCaseResult("MaxPower", U.watts, [7.2], f"{desc_base}, maximum"),
        ]
        self.assertEqual(expected_results, results)

    def test_sync_gets_descheduled(self) -> None:
        """Detect sync getting descheduled in the middle and then coming back."""
        t_1 = 1
        processes = self.construct_trace_processes([t_1])

        records_0: list[trace_model.SchedulingRecord] = [
            # "thread-1" is active from 0 - 500, 750-1000, then exits.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                t_1,
                8888,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(500000000),
                8888,
                t_1,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(750000000),
                t_1,
                8888,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(1000000000),
                70,
                t_1,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
            # "thread-1" starts waiting to run on this processor at 700
            trace_model.Waking(trace_time.TimePoint(700000000), t_1, 612, {}),
        ]

        records_1: list[trace_model.SchedulingRecord] = [
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                100,
                70,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
        ]

        model = trace_model.Model(processes, {0: records_0, 1: records_1})

        results = power.PowerMetricsProcessor().process_metrics(model)
        # Power samples should start to count the instant load generation stops, so expect
        # to count the .1A and .6A sample
        desc_base = power._AggregateMetrics.DESCRIPTION_BASE
        expected_results = [
            TestCaseResult("MinPower", U.watts, [1.2], f"{desc_base}, minimum"),
            TestCaseResult("MeanPower", U.watts, [4.2], f"{desc_base}, mean"),
            TestCaseResult("MaxPower", U.watts, [7.2], f"{desc_base}, maximum"),
        ]
        self.assertEqual(expected_results, results)

    def test_no_sync_signal(self) -> None:
        """Detect sync not being present."""
        processes = self.construct_trace_processes([])

        records_0: list[trace_model.SchedulingRecord] = [
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                10,
                30,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(1000000000),
                70,
                30,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
        ]

        records_1: list[trace_model.SchedulingRecord] = [
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                40,
                70,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
        ]

        model = trace_model.Model(processes, {0: records_0, 1: records_1})
        self.assertEqual(
            [], power.PowerMetricsProcessor().process_metrics(model)
        )

    def test_no_power_data_after_sync_signal(self) -> None:
        """Handle a lack of aggregated power metrics."""
        end_of_load = trace_time.TimePoint(1000000000)
        threads = (1, 2)
        processes = self.construct_trace_processes(threads, end_of_load)

        records_0: list[trace_model.SchedulingRecord] = [
            # "thread-1" is active from 0 - 1000, then exits.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                threads[0],
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                end_of_load,
                70,
                threads[0],
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
        ]

        records_1: list[trace_model.SchedulingRecord] = [
            # small-thread
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                100,
                70,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
        ]

        model = trace_model.Model(processes, {0: records_0, 1: records_1})
        self.assertEmpty(power.PowerMetricsProcessor().process_metrics(model))

    def test_find_suspend_windows(self) -> None:
        """Find periods during which device was suspended."""
        threads = (1, 2)
        initial_processes = self.construct_trace_processes(threads)
        windows = [
            trace_time.Window(
                trace_time.TimePoint(900_000_000),
                trace_time.TimePoint(1_100_000_000),
            ),
            trace_time.Window(
                trace_time.TimePoint(1_150_000_000),
                trace_time.TimePoint(1_350_000_000),
            ),
        ]
        suspender = _build_suspender(windows)
        processes = tuple(initial_processes) + (suspender,)
        model = trace_model.Model(processes, {})
        self.assertCountEqual(power._find_suspend_windows(model), windows)

    def test_suspended_power_metrics(self) -> None:
        """Power measurements during a suspend are captured, aggregated."""
        threads = (1,)
        initial_processes = self.construct_trace_processes(threads)

        records_0: list[trace_model.SchedulingRecord] = [
            # "thread-1" is active from 0 - 1000, then exits.
            trace_model.ContextSwitch(
                trace_time.TimePoint(0),
                threads[0],
                100,
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED,
                {},
            ),
            trace_model.ContextSwitch(
                trace_time.TimePoint(800_000_000),
                70,
                threads[0],
                612,
                612,
                trace_model.ThreadState.ZX_THREAD_STATE_DEAD,
                {},
            ),
        ]
        suspender = _build_suspender(
            [
                trace_time.Window(
                    trace_time.TimePoint(900_000_000),
                    trace_time.TimePoint(1_100_000_000),
                ),
            ]
        )
        model = trace_model.Model(
            tuple(initial_processes) + (suspender,), {0: records_0}
        )
        results = power.PowerMetricsProcessor().process_metrics(model)
        desc_base = power._AggregateMetrics.DESCRIPTION_BASE
        suspend_condition = "while device is suspended"
        awake_condition = "while device is awake"
        expected_results = frozenset(
            (
                TestCaseResult(
                    label="MinPower_suspend",
                    unit=U.watts,
                    values=[1.2],
                    doc=f"{desc_base} {suspend_condition}, minimum",
                ),
                TestCaseResult(
                    label="MeanPower_suspend",
                    unit=U.watts,
                    values=[1.2],
                    doc=f"{desc_base} {suspend_condition}, mean",
                ),
                TestCaseResult(
                    label="MaxPower_suspend",
                    unit=U.watts,
                    values=[1.2],
                    doc=f"{desc_base} {suspend_condition}, maximum",
                ),
                TestCaseResult(
                    label="MinPower_running",
                    unit=U.watts,
                    values=[7.2],
                    doc=f"{desc_base} {awake_condition}, minimum",
                ),
                TestCaseResult(
                    label="MeanPower_running",
                    unit=U.watts,
                    values=[7.2],
                    doc=f"{desc_base} {awake_condition}, mean",
                ),
                TestCaseResult(
                    label="MaxPower_running",
                    unit=U.watts,
                    values=[7.2],
                    doc=f"{desc_base} {awake_condition}, maximum",
                ),
            )
        )
        self.assertLessEqual(expected_results, set(results))
        results = power.PowerMetricsProcessor(lambda w: False).process_metrics(
            model
        )
        self.assertEmpty(expected_results & set(results))


def _build_suspender(
    windows: Sequence[trace_time.Window],
) -> trace_model.Process:
    pid = 5555
    tid = 6666
    return trace_model.Process(
        pid=pid,
        name=f"{power._SAG}.cm",
        threads=[
            trace_model.Thread(
                tid=tid,
                name="initial-thread",
                events=[
                    _make_synthetic_event(
                        timestamp_usec=w.start.to_epoch_delta().to_microseconds(),
                        pid=pid,
                        tid=tid,
                        duration_usec=(w.end - w.start).to_microseconds(),
                    )
                    for w in windows
                ],
            ),
        ],
    )


def _make_synthetic_event(
    timestamp_usec: int, pid: int, tid: int, duration_usec: int
) -> trace_model.DurationEvent:
    """Build a synthetic suspend DurationEvent.

    Providing this function enables building fake traces for unittests.
    """

    # LINT.IfChange
    _SAG_EVENT_NAME = "system-activity-governor:suspend"
    # LINT.ThenChange(//src/power/system-activity-governor/src/cpu_manager.rs)

    return trace_model.DurationEvent.consume_dict(
        {
            "cat": "power",
            "name": _SAG_EVENT_NAME,
            "ts": timestamp_usec,
            "pid": pid,
            "tid": tid,
            "dur": duration_usec,
        }
    )
