#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for ../metrics/wakeup.py."""

import logging
import os
import unittest

from reporting import metrics
from trace_processing import trace_importing, trace_model
from trace_processing.metrics import wakeup

_LOGGER: logging.Logger = logging.getLogger(__name__)
_LABEL = "WakeupTime"
_EVENTS = ["WakeupEvent1", "WakeupEvent2", "WakeupEvent3"]

# Boilerplate-busting constants:
U = metrics.Unit
TCR = metrics.TestCaseResult


class WakeupMetricsTest(unittest.TestCase):
    """Wakeup Metrics tests."""

    @staticmethod
    def _load_model(model_file_name: str) -> trace_model.Model:
        # A second dirname is required to account for the .pyz archive which
        # contains the test and a third one since data is a sibling of the test.
        runtime_deps_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
            "runtime_deps/wakeup_metrics_processor",
        )
        return trace_importing.create_model_from_file_path(
            os.path.join(runtime_deps_path, model_file_name)
        )

    def test_process_metrics_handles_zero_wakeups(self) -> None:
        model = WakeupMetricsTest._load_model("zero_wakeups.json")
        result = wakeup.WakeupMetricsProcessor(_LABEL, _EVENTS).process_metrics(
            model
        )
        self.assertEqual(result, [])

    def test_process_metrics_raises_on_zero_wakeups(self) -> None:
        model = WakeupMetricsTest._load_model("zero_wakeups.json")
        with self.assertRaises(RuntimeError) as context:
            wakeup.WakeupMetricsProcessor(
                _LABEL, _EVENTS, require_wakeup=True
            ).process_metrics(model)
        self.assertEqual(
            str(context.exception),
            "Required wakeup not present in trace, observed events: '', missing event: 'WakeupEvent1'",
        )

    def test_process_metrics_raises_on_incomplete_wakeup(self) -> None:
        model = WakeupMetricsTest._load_model("incomplete_wakeup.json")
        with self.assertRaises(RuntimeError) as context:
            wakeup.WakeupMetricsProcessor(
                _LABEL, _EVENTS, require_wakeup=True
            ).process_metrics(model)
        self.assertEqual(
            str(context.exception),
            "Required wakeup not present in trace, observed events: 'WakeupEvent1,WakeupEvent2', missing event: 'WakeupEvent3'",
        )

    def test_process_metrics_handles_one_wakeup(self) -> None:
        model = WakeupMetricsTest._load_model("one_wakeup.json")
        result = wakeup.WakeupMetricsProcessor(_LABEL, _EVENTS).process_metrics(
            model
        )
        self.assertEqual(
            result,
            [
                TCR(
                    label=_LABEL,
                    unit=U.nanoseconds,
                    # Measured from end of WakeupEvent1 to end of WakeupEvent3.
                    values=[31000000],
                )
            ],
        )

    def test_process_metrics_handles_many_wakeup(self) -> None:
        model = WakeupMetricsTest._load_model("many_wakeups.json")
        result = wakeup.WakeupMetricsProcessor(_LABEL, _EVENTS).process_metrics(
            model
        )
        self.assertEqual(
            result,
            [
                TCR(
                    label=_LABEL,
                    unit=U.nanoseconds,
                    # Measured from end of WakeupEvent1 to end of WakeupEvent3.
                    values=[31000000, 22000000],
                )
            ],
        )

    def test_event_0_restarts_sequence(self) -> None:
        model = WakeupMetricsTest._load_model("restarted_wakeup.json")
        result = wakeup.WakeupMetricsProcessor(_LABEL, _EVENTS).process_metrics(
            model
        )
        self.assertEqual(
            result,
            [
                TCR(
                    label=_LABEL,
                    unit=U.nanoseconds,
                    # Measured from end of the second instance WakeupEvent1 to end of WakeupEvent3.
                    values=[22000000],
                )
            ],
        )
