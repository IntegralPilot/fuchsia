# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os

import fuchsia_base_test
import test_data
from host_driven import run_test_component
from mobly import asserts, test_runner
from perf_publish import publish
from trace_processing import trace_importing, trace_model, trace_utils

TEST_URL: str = (
    "fuchsia-pkg://fuchsia.com/fuchsia_microbenchmarks#meta/"
    "fuchsia_microbenchmarks.cm"
)

TEST_RUST_URL: str = (
    "fuchsia-pkg://fuchsia.com/rust_trace_events_benchmarks#meta/"
    "trace_events.cm"
)

# For running with tracing enabled we run the following tests:
#
#  - The Tracing suite, which creates exercises the trace-engine code via
#    TRACE macros
#  - Syscall/Null, Syscall/ManyArgs, and Channel/WriteRead which exercise
#    writing events to the kernel trace buffer
FILTER_REGEX: str = (
    "(^Tracing/)|(^Syscall/Null$)|(^Syscall/ManyArgs$)|"
    "(^Channel/WriteRead/1024bytes/1handles$)"
)

# We override the default number of within-process iterations of
# each test case and use a lower value.  This reduces the overall
# time taken and reduces the chance that these invocations hit
# Infra Swarming tasks' IO timeout (swarming_io_timeout_secs --
# the amount of time that a task is allowed to run without
# producing log output).
ITERATIONS_PER_TEST_PER_PROCESS: int = 120

# We run the fuchsia_microbenchmarks process multiple times.  That is
# useful for tests that exhibit between-process variation in results
# (e.g. due to memory layout chosen when a process starts) -- it
# reduces the variation in the average that we report.
PROCESS_RUNS: int = 6


class TracingMicrobenchmarksTest(fuchsia_base_test.FuchsiaBaseTest):
    # Run some of the microbenchmarks with tracing enabled to measure the
    # overhead of tracing.
    async def test_tracing_categories_enabled(self) -> None:
        results_files = []
        userspace_events = {"InstantEvent", "ScopedDuration", "DurationBegin"}
        syscall_events = {"syscall_test_0", "syscall_test_8"}
        for i in range(PROCESS_RUNS):
            results_files.append(
                await self._run_tracing_microbenchmark(
                    i, ["kernel", "benchmark"], ".tracing"
                )
            )
            model = trace_importing.create_model_from_trace_file_path(
                os.path.join(self.test_case_path, "trace.fxt"),
                # DurationEnd records pair with DurationBegin records to create DurationBegin events
                # so we need them too.
                patterns=userspace_events | {"DurationEnd"} | syscall_events,
            )

            events = list(
                trace_utils.filter_events(
                    model.all_events(),
                    category="benchmark",
                    type=trace_model.Event,
                )
            )
            for event_name in userspace_events:
                asserts.assert_equal(
                    len([e for e in events if e.name == event_name]),
                    ITERATIONS_PER_TEST_PER_PROCESS,
                    f"Wrong number of {event_name} events in trace.",
                )

            events = list(
                trace_utils.filter_events(
                    model.all_events(),
                    category="kernel:syscall",
                    type=trace_model.Event,
                )
            )
            for event_name in syscall_events:
                asserts.assert_equal(
                    len([e for e in events if e.name == event_name]),
                    ITERATIONS_PER_TEST_PER_PROCESS,
                    f"Wrong number of {event_name} events in trace.",
                )

        publish.publish_fuchsiaperf(
            results_files,
            "fuchsia.microbenchmarks.tracing.txt",
            test_data_module=test_data,
        )

    # Run some of the microbenchmarks with tracing enabled but each category
    # disabled to measure the overhead of a trace event with the category turned
    # off.
    async def test_tracing_categories_disabled(self) -> None:
        results_files = []
        for i in range(PROCESS_RUNS):
            results_files.append(
                await self._run_tracing_microbenchmark(
                    i, ["nonexistent_category"], ".tracing_categories_disabled"
                )
            )
            model = trace_importing.create_model_from_trace_file_path(
                os.path.join(self.test_case_path, "trace.fxt")
            )
            # All the real tracing categories are disabled, so we should get no trace events.
            asserts.assert_equal(list(model.all_events()), [])

        publish.publish_fuchsiaperf(
            results_files,
            "fuchsia.microbenchmarks.tracing_categories_disabled.txt",
            test_data_module=test_data,
        )

    # --- Rust Trace Library Benchmarks
    #
    # We take a similar approach with the rust benchmarks. The library currently calls into the c
    # trace bindings, so our aim here is to ensure we aren't accidentally adding significant
    # overhead in the translation layer.
    #
    # These benchmarks are in a separate rust based binary, so we run 2 variants:
    # - Tracing disabled
    # - Tracing enabled, but "benchmark" category disabled
    #
    # TODO(b/295183613): Add a benchmark with tracing enabled. Currently the buffer fills up
    # instantly and then trace manager attempts to empty the buffer during the run, blocking its
    # async loop. At the same time sl4f tries to block on stopping the trace but doesn't try to read
    # the trace buffer resulting in a deadlock. Once that's done, it should be as easy as copying
    # this benchmark and changing the categories enabled to 'benchmark'.
    async def test_tracing_rust_categories_disabled(self) -> None:
        async with self.dut.tracing.trace_session(
            categories=["nonexistent_category"],
            buffer_size=1,
            download=True,
            directory=self.test_case_path,
            trace_file="trace.fxt",
        ):
            results_file: str = run_test_component.once(
                self.dut.ffx,
                TEST_RUST_URL,
                self.test_case_path,
                test_component_args=[
                    run_test_component.DEFAULT_TARGET_RESULTS_PATH,
                ],
            )

        self._add_test_suite_suffix(
            results_file, ".tracing_categories_disabled"
        )
        publish.publish_fuchsiaperf(
            [results_file],
            "fuchsia.trace_records.rust.tracing_categories_disabled.txt",
            test_data_module=test_data,
        )

    def test_tracing_rust_tracing_disabled(self) -> None:
        results_file: str = run_test_component.once(
            self.dut.ffx,
            TEST_RUST_URL,
            self.test_case_path,
            test_component_args=[
                run_test_component.DEFAULT_TARGET_RESULTS_PATH,
            ],
        )

        self._add_test_suite_suffix(results_file, ".tracing_disabled")
        publish.publish_fuchsiaperf(
            [results_file],
            "fuchsia.trace_records.rust.tracing_disabled.txt",
            test_data_module=test_data,
        )

    async def _run_tracing_microbenchmark(
        self, run_id: int, categories: list[str], results_suffix: str
    ) -> str:
        async with self.dut.tracing.trace_session(
            categories=categories,
            buffer_size=36,
            download=True,
            directory=self.test_case_path,
            trace_file="trace.fxt",
        ):
            results_file: str = run_test_component.once(
                self.dut.ffx,
                TEST_URL,
                self.test_case_path,
                ffx_test_args=["--realm", "/core/testing/system-tests"],
                test_component_args=[
                    "-p",
                    "--quiet",
                    "--out",
                    run_test_component.DEFAULT_TARGET_RESULTS_PATH,
                    "--runs",
                    str(ITERATIONS_PER_TEST_PER_PROCESS),
                    "--filter",
                    FILTER_REGEX,
                    "--enable-tracing",
                ],
                host_results_file=f"results_process{run_id}.fuchsiaperf_full.json",
            )

        self._add_test_suite_suffix(results_file, results_suffix)
        return results_file

    def _add_test_suite_suffix(self, results_file: str, suffix: str) -> None:
        with open(results_file, "r") as f:
            entries = json.load(f)
        for test_result in entries:
            test_result["test_suite"] += suffix
        with open(results_file, "w") as f:
            json.dump(entries, f)


if __name__ == "__main__":
    test_runner.main()
