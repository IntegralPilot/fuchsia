#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for trace_utils.py."""

import unittest
from typing import List

import test_utils
from reporting import metrics
from trace_processing import trace_model, trace_time, trace_utils

# Boilerplate-busting constants:
U = metrics.Unit
TCR = metrics.TestCaseResult


class TraceUtilsTest(unittest.TestCase):
    """Trace utils tests"""

    def test_compute_stats(self) -> None:
        self.assertAlmostEqual(
            trace_utils.percentile(
                [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0], 25
            ),
            3.0,
        )
        self.assertAlmostEqual(
            trace_utils.percentile(
                [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0], 50
            ),
            5.0,
        )
        self.assertAlmostEqual(
            trace_utils.percentile(
                [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0], 75
            ),
            7.0,
        )
        self.assertAlmostEqual(
            trace_utils.percentile(
                [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0], 25
            ),
            3.25,
        )
        self.assertAlmostEqual(
            trace_utils.percentile(
                [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0], 50
            ),
            5.5,
        )
        self.assertAlmostEqual(
            trace_utils.percentile(
                [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0], 75
            ),
            7.75,
        )

    def test_filter_events(self) -> None:
        events: List[trace_model.Event] = [
            trace_model.DurationEvent(
                duration=None,
                parent=None,
                child_durations=[],
                child_flows=[],
                base=trace_model.Event(
                    category="cat_a",
                    name="name_a",
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(
                            697778328.2160872
                        )
                    ),
                    pid=7009,
                    tid=7022,
                    args={},
                ),
            ),
            trace_model.DurationEvent(
                duration=None,
                parent=None,
                child_durations=[],
                child_flows=[],
                base=trace_model.Event(
                    category="cat_b",
                    name="name_b",
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(
                            697778328.2160872
                        )
                    ),
                    pid=7009,
                    tid=7022,
                    args={},
                ),
            ),
        ]

        filtered = list(
            trace_utils.filter_events(
                events, category="cat_a", name="name_a", type=trace_model.Event
            )
        )
        self.assertEqual(filtered, [events[0]])

        filtered2 = list(
            trace_utils.filter_events(
                events, category="cat_c", name="name_c", type=trace_model.Event
            )
        )
        self.assertEqual(filtered2, [])

    def test_filter_events_with_name_collection(self) -> None:
        events: List[trace_model.Event] = [
            trace_model.DurationEvent(
                duration=None,
                parent=None,
                child_durations=[],
                child_flows=[],
                base=trace_model.Event(
                    category="cat_a",
                    name="name_a",
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(0)
                    ),
                    pid=7009,
                    tid=7022,
                    args={},
                ),
            ),
            trace_model.DurationEvent(
                duration=None,
                parent=None,
                child_durations=[],
                child_flows=[],
                base=trace_model.Event(
                    category="cat_b",
                    name="name_b",
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(0)
                    ),
                    pid=7009,
                    tid=7022,
                    args={},
                ),
            ),
            trace_model.DurationEvent(
                duration=None,
                parent=None,
                child_durations=[],
                child_flows=[],
                base=trace_model.Event(
                    category="cat_c",
                    name="name_c",
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(0)
                    ),
                    pid=7009,
                    tid=7022,
                    args={},
                ),
            ),
        ]

        filtered = list(
            trace_utils.filter_events(
                events, name={"name_a", "name_c"}, type=trace_model.Event
            )
        )
        self.assertEqual(filtered, [events[0], events[2]])

        filtered2 = list(
            trace_utils.filter_events(
                events, name=["name_b", "name_c"], type=trace_model.Event
            )
        )
        self.assertEqual(filtered2, [events[1], events[2]])

    def test_filter_events_with_type(self) -> None:
        events: List[trace_model.Event] = [
            trace_model.DurationEvent(
                duration=None,
                parent=None,
                child_durations=[],
                child_flows=[],
                base=trace_model.Event(
                    category="cat_a",
                    name="name_a",
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(
                            697778328.2160872
                        )
                    ),
                    pid=7009,
                    tid=7022,
                    args={},
                ),
            ),
            trace_model.DurationEvent(
                duration=None,
                parent=None,
                child_durations=[],
                child_flows=[],
                base=trace_model.Event(
                    category="cat_b",
                    name="name_b",
                    start=trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(
                            697778328.2160872
                        )
                    ),
                    pid=7009,
                    tid=7022,
                    args={},
                ),
            ),
        ]

        filtered: List[trace_model.Event] = list(
            trace_utils.filter_events(
                events,
                category="cat_a",
                name="name_a",
                type=trace_model.DurationEvent,
            )
        )
        self.assertEqual(filtered, [events[0]])

        filtered2: List[trace_model.Event] = list(
            trace_utils.filter_events(
                events,
                category="cat_c",
                name="name_c",
                type=trace_model.DurationEvent,
            )
        )
        self.assertEqual(filtered2, [])

        filtered3: List[trace_model.Event] = list(
            trace_utils.filter_events(
                events,
                category="cat_a",
                name="name_a",
                type=trace_model.InstantEvent,
            )
        )
        self.assertEqual(filtered3, [])

    def test_total_event_duration(self) -> None:
        model: trace_model.Model = test_utils.get_test_model()
        mode_duration: trace_time.TimeDelta = (
            trace_time.TimeDelta.from_microseconds(
                test_utils.TEST_MODEL_END_TIME_IN_US
            )
            - trace_time.TimeDelta.from_microseconds(
                test_utils.TEST_MODEL_BEGIN_TIME_IN_US
            )
        )
        self.assertEqual(
            trace_utils.total_event_duration(model.all_events()), mode_duration
        )

    def test_standard_metrics_set(self) -> None:
        values: list[float] = [0.0, 10.0]
        unit = U.milliseconds

        results = trace_utils.standard_metrics_set(
            values=values, label_prefix="Foo", unit=unit
        )
        p_format_str = "{}, {}th percentile"
        self.maxDiff = None
        self.assertEqual(
            results,
            [
                TCR("FooP5", unit, [0.5], p_format_str.format("Foo", 5)),
                TCR("FooP25", unit, [2.5], p_format_str.format("Foo", 25)),
                TCR("FooP50", unit, [5.0], p_format_str.format("Foo", 50)),
                TCR("FooP75", unit, [7.5], p_format_str.format("Foo", 75)),
                TCR("FooP95", unit, [9.5], p_format_str.format("Foo", 95)),
                TCR("FooMin", unit, [0.0], "Foo, minimum"),
                TCR("FooMax", unit, [10.0], "Foo, maximum"),
                TCR("FooAverage", unit, [5.0], "Foo, mean"),
            ],
        )

        results = trace_utils.standard_metrics_set(
            values=values, label_prefix="Foo", unit=unit, doc_prefix="Foo doc"
        )
        self.assertEqual(
            results,
            [
                TCR("FooP5", unit, [0.5], p_format_str.format("Foo doc", 5)),
                TCR("FooP25", unit, [2.5], p_format_str.format("Foo doc", 25)),
                TCR("FooP50", unit, [5.0], p_format_str.format("Foo doc", 50)),
                TCR("FooP75", unit, [7.5], p_format_str.format("Foo doc", 75)),
                TCR("FooP95", unit, [9.5], p_format_str.format("Foo doc", 95)),
                TCR("FooMin", unit, [0.0], "Foo doc, minimum"),
                TCR("FooMax", unit, [10.0], "Foo doc, maximum"),
                TCR("FooAverage", unit, [5.0], "Foo doc, mean"),
            ],
        )
