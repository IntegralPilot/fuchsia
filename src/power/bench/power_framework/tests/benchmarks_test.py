# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import os

import fuchsia_base_test
from mobly import test_runner
from trace_processing import trace_importing

_LOGGER = logging.getLogger(__name__)


class PowerBenchmarksTest(fuchsia_base_test.FuchsiaBaseTest):
    """Runs Power Framework benchmarks with trace collection."""

    async def test_integration_testcase(self) -> None:
        _LOGGER.info("Running power framework benchmarks Lacewing test...")

        host_output_path = self.test_case_path
        async with self.dut.tracing.trace_session(
            categories=[
                "kernel:sched",
                "kernel:meta",
                "power",
                "power-broker",
            ],
            buffer_size=36,
            download=True,
            directory=host_output_path,
            trace_file="trace.fxt",
        ):
            self.dut.ffx.run_test_component(
                "fuchsia-pkg://fuchsia.com/power-framework-bench-integration-tests#meta/integration.cm",
                ffx_test_args=[
                    "--realm",
                    "/core/testing/system-tests",
                    "--parallel",
                    "1",
                ],
                test_component_args=[
                    "--repeat",
                    "100",
                ],
                capture_output=False,
            )

        json_trace_file: str = trace_importing.convert_trace_file_to_json(
            os.path.join(host_output_path, "trace.fxt")
        )
        _LOGGER.info("Json Trace file name: %s", json_trace_file)


if __name__ == "__main__":
    test_runner.main()
