# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os

import fuchsia_base_test
from mobly import asserts, test_runner
from trace_processing import trace_importing, trace_model, trace_utils


class PerfTestTraceEventsTest(fuchsia_base_test.FuchsiaBaseTest):
    async def test_perftest_library_trace_events(self) -> None:
        async with self.dut.tracing.trace_session(
            categories=[
                "kernel",
                "perftest",
            ],
            buffer_size=36,
            download=True,
            directory=self.log_path,
            trace_file="trace.fxt",
        ):
            self.dut.ffx.run_test_component(
                "fuchsia-pkg://fuchsia.com/fuchsia_microbenchmarks#meta/fuchsia_microbenchmarks.cm",
                ffx_test_args=["--realm", "/core/testing/system-tests"],
                test_component_args=[
                    "-p",
                    "--quiet",
                    "--runs",
                    "4",
                    "--enable-tracing",
                    "--filter=^Null$",
                ],
                capture_output=False,
            )

        expected_event_names = [
            "test_group",
            "test_setup",
            "test_run",
            "test_run",
            "test_run",
            "test_run",
            "test_teardown",
        ]
        model = trace_importing.create_model_from_trace_file_path(
            os.path.join(self.log_path, "trace.fxt"),
            patterns=set(expected_event_names),
        )
        event_names = [
            event.name
            for event in trace_utils.filter_events(
                model.all_events(),
                category="perftest",
                type=trace_model.Event,
            )
        ]
        asserts.assert_equal(event_names, expected_event_names)


if __name__ == "__main__":
    test_runner.main()
