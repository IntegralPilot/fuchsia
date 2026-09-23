#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Input Latency Benchmark."""

import os
from pathlib import Path

import fuchsia_base_test
import test_data
from honeydew.affordances.ui.user_input import types as ui_custom_types
from mobly import test_runner
from perf_publish import publish
from reporting import metrics
from trace_processing import trace_importing
from trace_processing.metrics import input_latency

TOUCH_APP = (
    "fuchsia-pkg://fuchsia.com/flatland-examples#meta/"
    "simplest-app-flatland-session.cm"
)
TEST_NAME: str = "fuchsia.input_latency.simplest_app"


class InputBenchmark(fuchsia_base_test.FuchsiaBaseTest):
    """Input Benchmarks.

    Attributes:
        dut: FuchsiaDevice object.

    This test traces touch input performance in
    ui/examples/simplest-app-flatland-session.
    """

    async def setup_test(self) -> None:
        await super().setup_test()

        self.dut.session.ensure_started()

    async def teardown_test(self) -> None:
        self.dut.session.cleanup()
        await super().teardown_test()

    async def test_logic(self) -> None:
        # Add simplest-input-flatland-session-app to session.
        self.dut.session.add_component(TOUCH_APP)

        touch_device = self.dut.user_input.create_touch_device()

        async with self.dut.tracing.trace_session(
            categories=[
                "gfx",
                "input",
                "kernel:ipc",
                "magma",
            ],
            buffer_size=36,
            download=True,
            directory=self.log_path,
            trace_file="trace.fxt",
        ):
            # Each tap will be 33.5ms apart, drifting 0.166ms against regular 60
            # fps vsync interval. 100 taps span the entire vsync interval 1 time at
            # 100 equidistant points.
            await touch_device.tap(
                location=ui_custom_types.Coordinate(x=500, y=500),
                tap_event_count=100,
                duration_ms=3350,
            )

        expected_trace_filename: str = os.path.join(self.log_path, "trace.fxt")

        processor = input_latency.InputLatencyMetricsProcessor(
            aggregates_only=False
        )
        model = trace_importing.create_model_from_trace_file_path(
            expected_trace_filename,
            patterns=processor.event_patterns,
            categories=processor.category_names,
        )

        input_latency_results = processor.process_metrics(model)

        fuchsiaperf_json_path = Path(
            os.path.join(self.log_path, f"{TEST_NAME}.fuchsiaperf.json")
        )

        metrics.TestCaseResult.write_fuchsiaperf_json(
            results=input_latency_results,
            test_suite=f"{TEST_NAME}",
            output_path=fuchsiaperf_json_path,
        )

        publish.publish_fuchsiaperf(
            fuchsia_perf_file_paths=[fuchsiaperf_json_path],
            expected_metric_names_filename=f"{TEST_NAME}.txt",
            test_data_module=test_data,
        )


if __name__ == "__main__":
    test_runner.main()
