#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for trace metrics processors."""

import inspect as py_inspect
import json
import os
import pathlib
import tempfile
import unittest
from collections.abc import Collection

import trace_processing.metrics.app_render as app_render_metrics
import trace_processing.metrics.cpu as cpu_metrics
import trace_processing.metrics.fps as fps_metrics
import trace_processing.metrics.input_latency as input_latency_metrics
import trace_processing.metrics.scenic as scenic_metrics
from parameterized import param, parameterized
from reporting import metrics
from trace_processing import (
    trace_importing,
    trace_metrics,
    trace_model,
    trace_time,
    trace_utils,
)

# Boilerplate-busting constants:
U = metrics.Unit
TCR = metrics.TestCaseResult


class TestCaseResultTest(unittest.TestCase):
    """Tests TestCaseResult"""

    def test_to_json(self) -> None:
        label = "L1"
        test_suite = "bar"

        result = TCR(
            label=label, unit=U.bytesPerSecond, values=[0, 0.1, 23.45, 6]
        )

        self.assertEqual(result.label, label)
        self.assertEqual(
            result.to_json(test_suite=test_suite),
            {
                "label": label,
                "test_suite": test_suite,
                "unit": "bytes/second",
                "values": [0, 0.1, 23.45, 6],
            },
        )

    def test_write_fuchsia_perf_json(self) -> None:
        test_suite = "ts"
        results = [
            TCR(label="l1", unit=U.percent, values=[1]),
            TCR(label="l2", unit=U.framesPerSecond, values=[2]),
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            actual_output_path = (
                pathlib.Path(tmpdir) / "actual_output.fuchsiaperf.json"
            )

            metrics.TestCaseResult.write_fuchsiaperf_json(
                results,
                test_suite=test_suite,
                output_path=actual_output_path,
            )

            actual_output = json.loads(actual_output_path.read_text())
            self.assertEqual(
                actual_output, [r.to_json(test_suite) for r in results]
            )


_METRICS_TEST_PARAMS = [
    param(
        "cpu",
        processor=cpu_metrics.CpuMetricsProcessor(aggregates_only=False),
        model_file="cpu_metric.json",
        expected_results=[
            TCR(label="CpuLoad", unit=U.percent, values=[43, 20]),
        ],
    ),
    param(
        "cpu_aggregates",
        processor=cpu_metrics.CpuMetricsProcessor(aggregates_only=True),
        model_file="cpu_metric.json",
        expected_results=trace_utils.standard_metrics_set(
            # ts in the json is in microseconds, durations is in ns.
            [43, 20],
            "Cpu",
            U.percent,
            durations=[1000000000, 1100000000],
        ),
    ),
    param(
        "fps",
        processor=fps_metrics.FpsMetricsProcessor(aggregates_only=False),
        model_file="fps_metric.json",
        expected_results=[
            TCR(
                label="Fps",
                unit=U.framesPerSecond,
                values=[10000000.0, 5000000.0],
            )
        ],
    ),
    param(
        "fps_aggregates",
        processor=fps_metrics.FpsMetricsProcessor(aggregates_only=True),
        model_file="fps_metric.json",
        expected_results=trace_utils.standard_metrics_set(
            [10000000.0, 5000000.0],
            "Fps",
            U.framesPerSecond,
        ),
    ),
    param(
        "scenic",
        processor=scenic_metrics.ScenicMetricsProcessor(aggregates_only=False),
        model_file="scenic_metric.json",
        expected_results=[
            TCR(
                label="RenderCpu",
                unit=U.milliseconds,
                values=[0.09, 0.08],
            ),
            TCR(
                label="RenderTotal",
                unit=U.milliseconds,
                values=[0.11, 0.112],
            ),
        ],
    ),
    param(
        "scenic_aggregates",
        processor=scenic_metrics.ScenicMetricsProcessor(aggregates_only=True),
        model_file="scenic_metric.json",
        expected_results=(
            trace_utils.standard_metrics_set(
                [0.09, 0.08],
                "RenderCpu",
                U.milliseconds,
            )
            + trace_utils.standard_metrics_set(
                [0.11, 0.112],
                "RenderTotal",
                U.milliseconds,
            )
        ),
    ),
    param(
        "scenic_no_render_total",
        processor=scenic_metrics.ScenicMetricsProcessor(
            aggregates_only=False, include_render_total=False
        ),
        model_file="scenic_metric.json",
        expected_results=[
            TCR(
                label="RenderCpu",
                unit=U.milliseconds,
                values=[0.09, 0.08],
            ),
        ],
    ),
    param(
        "app_render",
        processor=app_render_metrics.AppRenderLatencyMetricsProcessor(
            "flatland-view-provider-example",
            aggregates_only=False,
        ),
        model_file="app_render_metric.json",
        expected_results=[
            TCR(
                label="AppRenderVsyncLatency",
                unit=U.milliseconds,
                values=[0.004, 0.002, 0.001, 0.006, 0.008],
            ),
            TCR(
                label="AppFps",
                unit=U.framesPerSecond,
                values=[
                    125000.0,
                    111111.11111111111,
                    66666.66666666667,
                    83333.33333333333,
                ],
            ),
        ],
    ),
    param(
        "app_render_aggregates",
        processor=app_render_metrics.AppRenderLatencyMetricsProcessor(
            "flatland-view-provider-example",
            aggregates_only=True,
        ),
        model_file="app_render_metric.json",
        expected_results=(
            trace_utils.standard_metrics_set(
                [0.004, 0.002, 0.001, 0.006, 0.008],
                "AppRenderVsyncLatency",
                U.milliseconds,
            )
            + trace_utils.standard_metrics_set(
                [
                    125000.0,
                    111111.11111111111,
                    66666.66666666667,
                    83333.33333333333,
                ],
                "AppFps",
                U.framesPerSecond,
            )
        ),
    ),
    param(
        "input",
        processor=input_latency_metrics.InputLatencyMetricsProcessor(
            aggregates_only=False,
        ),
        model_file="input_latency_metric.json",
        expected_results=[
            TCR(
                label="total_input_latency",
                unit=U.milliseconds,
                values=[0.003, 0.002, 0.007, 0.009],
            ),
        ],
    ),
    param(
        "input_aggregates",
        processor=input_latency_metrics.InputLatencyMetricsProcessor(
            aggregates_only=True,
        ),
        model_file="input_latency_metric.json",
        expected_results=trace_utils.standard_metrics_set(
            [0.003, 0.002, 0.007, 0.009],
            "InputLatency",
            U.milliseconds,
        ),
    ),
]


class MetricProcessorsTest(unittest.TestCase):
    """Tests for the various MetricProcessors."""

    def _load_model(self, model_file_name: str) -> trace_model.Model:
        # A second dirname is required to account for the .pyz archive which
        # contains the test and a third one since data is a sibling of the test.
        runtime_deps_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
            "runtime_deps",
        )
        return trace_importing.create_model_from_file_path(
            os.path.join(runtime_deps_path, model_file_name)
        )

    def _load_fxt_model(self, fxt_file: str) -> trace_model.Model:
        runtime_deps_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
            "runtime_deps",
        )
        return trace_importing.create_model_from_trace_file_path(
            os.path.join(runtime_deps_path, fxt_file),
            without_json_conversion=True,
        )

    @parameterized.expand(_METRICS_TEST_PARAMS)
    def test_processor(
        self,
        _: str,
        processor: trace_metrics.MetricsProcessor,
        model_file: str,
        expected_results: list[TCR],
    ) -> None:
        """Tests a processor's outputs with a given input model loaded from a json file"""
        model = self._load_model(model_file)
        actual_results = list(processor.process_metrics(model))

        docs = processor.describe(actual_results)
        self.assertEqual(
            docs.get("classname", ""), processor.__class__.__name__
        )
        self.assertNotEqual(docs["doc"], "")
        self.assertNotEqual(docs["code_path"], "")

        # Improves assertEqual output when comparing lists.
        self.maxDiff = 10000
        self.assertEqual(actual_results, expected_results)

    @parameterized.expand(_METRICS_TEST_PARAMS)
    def test_processor_fxt(
        self,
        _: str,
        processor: trace_metrics.MetricsProcessor,
        model_file: str,
        expected_results: list[TCR],
    ) -> None:
        """Tests a processor's outputs with a given input model loaded from an FXT file"""
        fxt_file = model_file.replace(".json", ".fxt")
        model = self._load_fxt_model(fxt_file)
        actual_results = list(processor.process_metrics(model))

        docs = processor.describe(actual_results)
        self.assertEqual(
            docs.get("classname", ""), processor.__class__.__name__
        )
        self.assertNotEqual(docs["doc"], "")
        self.assertNotEqual(docs["code_path"], "")

        self.maxDiff = 10000
        self.assertEqual(actual_results, expected_results)

    class TestMetricsProcessor(trace_metrics.MetricsProcessor):
        """TEST"""

        def process_metrics(
            self, model: trace_model.Model
        ) -> Collection[metrics.TestCaseResult]:
            # If ever called, this can be implemented.
            raise NotImplementedError()

    def test_auto_doc(self) -> None:
        docs = self.TestMetricsProcessor().describe([])
        self.assertEqual(docs.get("classname", ""), "TestMetricsProcessor")
        self.assertEqual(docs["doc"], "TEST")
        self.assertEqual(docs["code_path"], py_inspect.getfile(self.__class__))

    def test_get_thread_running_durations(self) -> None:
        def make_switch(
            start_us: int, incoming_tid: int, outgoing_tid: int
        ) -> trace_model.ContextSwitch:
            return trace_model.ContextSwitch(
                start=trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_microseconds(start_us)
                ),
                incoming_tid=incoming_tid,
                outgoing_tid=outgoing_tid,
                incoming_prio=10,
                outgoing_prio=10,
                outgoing_state=trace_model.ThreadState.ZX_THREAD_STATE_RUNNING,
                args={},
            )

        scheduling_records = {
            0: [
                make_switch(1000, 10, 0),
                make_switch(3000, 20, 10),
                make_switch(5000, 10, 20),
                make_switch(6000, 0, 10),
            ]
        }

        windows = [
            (
                trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_microseconds(2000)
                ),
                trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_microseconds(7000)
                ),
            ),
            (
                trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_microseconds(1500)
                ),
                trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_microseconds(4000)
                ),
            ),
            (
                trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_microseconds(800)
                ),
                trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_microseconds(1200)
                ),
            ),
        ]

        durations = scenic_metrics._get_thread_running_durations(
            trace_model.Model([], scheduling_records), tid=10, windows=windows
        )

        self.assertEqual(len(durations), 3)
        self.assertEqual(durations[0].to_microseconds(), 2000)
        self.assertEqual(durations[1].to_microseconds(), 1500)
        self.assertEqual(durations[2].to_microseconds(), 200)
