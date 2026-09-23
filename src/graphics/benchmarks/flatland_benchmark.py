#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Flatland Benchmark."""

import asyncio
import itertools
import logging
import os
from pathlib import Path

import fuchsia_base_test
import test_data
from mobly import asserts, test_runner
from perf_publish import publish
from reporting import metrics
from trace_processing import trace_importing
from trace_processing.metrics import app_render, cpu

TILE_URL = (
    "fuchsia-pkg://fuchsia.com/flatland-examples#meta/flatland-rainbow.cm"
)
BENCHMARK_DURATION_SEC = 10
TEST_NAME: str = "fuchsia.app_render_latency"
LOGGER = logging.getLogger(__name__)


class FlatlandBenchmark(fuchsia_base_test.FuchsiaBaseTest):
    """Flatland Benchmark.

    Attributes:
        dut: FuchsiaDevice object.

    This test traces graphic performance in tile-session
    (src/ui/bin/tiles-session) and flatland-rainbow-example
    (src/ui/examples/flatland-rainbow).
    """

    async def setup_test(self) -> None:
        await super().setup_test()

        self.dut.session.ensure_started()

    async def teardown_test(self) -> None:
        self.dut.session.cleanup()
        await super().teardown_test()

    async def test_flatland(self) -> None:
        # Add flatland-rainbow tile
        self.dut.session.add_component(TILE_URL)

        async with self.dut.tracing.trace_session(
            categories=[
                "input",
                "gfx",
                "kernel:sched",
                "magma",
                "system_metrics",
                "system_metrics_logger",
            ],
            buffer_size=36,
            download=True,
            directory=self.log_path,
            trace_file="trace.fxt",
        ):
            await asyncio.sleep(BENCHMARK_DURATION_SEC)

        expected_trace_filename: str = os.path.join(self.log_path, "trace.fxt")

        asserts.assert_true(
            os.path.exists(expected_trace_filename), msg="trace failed"
        )

        app_render_processor = app_render.AppRenderLatencyMetricsProcessor(
            debug_name="flatland-rainbow-example",
            aggregates_only=True,
        )
        cpu_processor = cpu.CpuMetricsProcessor(aggregates_only=False)

        model = trace_importing.create_model_from_trace_file_path(
            expected_trace_filename,
            patterns=app_render_processor.event_patterns
            | cpu_processor.event_patterns,
            categories=app_render_processor.category_names
            | cpu_processor.category_names,
        )

        fuchsiaperf_json_path = Path(
            os.path.join(self.log_path, f"{TEST_NAME}.fuchsiaperf.json")
        )

        metrics.TestCaseResult.write_fuchsiaperf_json(
            results=itertools.chain(
                app_render_processor.process_metrics(model),
                cpu_processor.process_metrics(model),
            ),
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
