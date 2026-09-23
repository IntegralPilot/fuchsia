#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Test runner and reporting utilities for testusb.

Provides TestRunner to execute suites of test cases with loops, random
ordering, and configuration switching, as well as text and JSON reporting.
"""

from __future__ import annotations

import collections.abc
import logging
import os
import random
import sys
import time
from typing import Any

from .backend import USBTestBackend
from .models import (
    ALL_TEST_CASES,
    EP0_GENERIC_TESTS,
    TestParams,
    TestResult,
    TestStatus,
)

_LOGGER = logging.getLogger(__name__)


class TestRunner:
    """Manages execution of test cases with loops, random ordering, and reporting."""

    def __init__(
        self,
        backend: USBTestBackend,
        params: TestParams,
        loop_count: int = 1,
        random_order: bool = False,
        verbose: bool = False,
        mode: str = "auto",
        config: int | None = None,
        loopback: bool = False,
        quiet: bool = False,
    ) -> None:
        """Initializes the TestRunner.

        Args:
            backend: USB backend instance providing device I/O.
            params: Parameters controlling buffer size, iterations, etc.
            loop_count: Number of execution loops (0 or negative = infinite).
            random_order: Whether to shuffle tests in each loop.
            verbose: Whether to print verbose progress information.
            mode: Operating mode ('sourcesink', 'loopback', 'both', 'auto').
            config: Explicit USB configuration value (1 or 2).
            loopback: Whether loopback mode was requested.
            quiet: If True, suppress stdout printing of running/pass lines.
        """
        self.backend = backend
        self.params = params
        self.loop_count = loop_count  # 0 or negative means infinite
        self.random_order = random_order
        self.verbose = verbose
        self.mode = mode.lower()
        self.config = config
        self.loopback = loopback
        self.quiet = quiet
        self.results: list[TestResult] = []
        self.interrupted: bool = False

    def run(self, test_ids: collections.abc.Sequence[int]) -> list[TestResult]:
        """Run the specified test cases for the configured number of loops.

        Args:
            test_ids: Sequence of integer test IDs to execute.

        Returns:
            List of TestResult instances for all executed tests.
        """
        self.results.clear()
        loop = 0
        infinite = self.loop_count <= 0

        # If user explicitly requested a specific configuration, set it up front
        if self.config is not None:
            self.backend.set_configuration(self.config)

        try:
            while infinite or loop < self.loop_count:
                loop += 1

                if (
                    self.verbose
                    and not self.quiet
                    and (self.loop_count > 1 or infinite)
                ):
                    loop_str = f"Loop {loop}"
                    if infinite:
                        loop_str += " (infinite)"
                    print(f"\n--- {loop_str} ---")

                if self.mode == "both":
                    # Phase 1: Configuration 1 (Source/Sink)
                    ss_tests = [tid for tid in test_ids if tid <= 31]
                    if ss_tests:
                        self.backend.set_configuration(1)
                        if self.verbose and not self.quiet:
                            print(
                                "\n[=== Configuration 1: Source & Sink Mode ===]"
                            )
                        self._execute_test_batch(ss_tests)

                    # Phase 2: Configuration 2 (Loopback)
                    lb_tests = [
                        tid
                        for tid in test_ids
                        if tid >= 32 or tid in EP0_GENERIC_TESTS
                    ]
                    if lb_tests:
                        self.backend.set_configuration(2)
                        if self.verbose and not self.quiet:
                            print("\n[=== Configuration 2: Loopback Mode ===]")
                        self._execute_test_batch(lb_tests)

                elif self.mode == "loopback":
                    self.backend.set_configuration(2)
                    self._execute_test_batch(test_ids)

                else:
                    # mode == "sourcesink" or "auto"
                    if self.mode == "sourcesink":
                        self.backend.set_configuration(1)
                    self._execute_test_batch(test_ids)

        except KeyboardInterrupt:
            self.interrupted = True
            if not self.quiet:
                print(
                    "\n[!] Test execution interrupted by user.", file=sys.stderr
                )

        return self.results

    def _execute_test_batch(
        self, test_ids: collections.abc.Sequence[int]
    ) -> None:
        """Execute a batch of tests, handling random shuffling if enabled.

        Args:
            test_ids: Sequence of test IDs to execute.
        """
        batch = list(test_ids)
        if self.random_order:
            random.shuffle(batch)
        for test_id in batch:
            self._execute_test(test_id)

    def _execute_test(self, test_id: int) -> TestResult:
        """Execute a single test case and record its result.

        Args:
            test_id: Numerical identifier of the test to execute.

        Returns:
            TestResult recording outcome and performance metrics.
        """
        case = ALL_TEST_CASES.get(test_id)
        name = case.name if case else f"Test {test_id}"
        if not self.quiet:
            print(f"  [RUNNING] (Test {test_id}) {name}...", flush=True)
        start_time = time.perf_counter()
        try:
            res = self.backend.run_test(test_id, self.params)
        except Exception as e:
            _LOGGER.warning(
                "Test %d (%s) failed with unexpected exception: %s",
                test_id,
                name,
                e,
                exc_info=True,
            )
            res = TestResult(
                test_id=test_id,
                test_name=name,
                status=TestStatus.ERROR,
                duration_secs=time.perf_counter() - start_time,
                error_message=str(e),
            )
        self.results.append(res)
        self._print_result(res)
        return res

    def _print_result(self, result: TestResult) -> None:
        """Print the result of a single test case to stdout.

        Args:
            result: TestResult to display.
        """
        if self.quiet:
            return
        use_color = sys.stdout.isatty() and "NO_COLOR" not in os.environ
        if use_color:
            status_str = {
                TestStatus.PASS: "\033[32mPASS\033[0m",
                TestStatus.FAIL: "\033[31mFAIL\033[0m",
                TestStatus.SKIP: "\033[33mSKIP\033[0m",
                TestStatus.ERROR: "\033[35mERROR\033[0m",
            }.get(result.status, result.status.value)
        else:
            status_str = result.status.value

        throughput_str = ""
        if result.throughput_mbs > 0:
            throughput_str = (
                f", {result.throughput_mbs:.2f} MB/s "
                f"({result.throughput_mbps:.2f} Mbps)"
            )

        print(
            f"  [{status_str}] (Test {result.test_id}) {result.test_name} "
            f"({result.duration_secs:.4f}s{throughput_str})",
            flush=True,
        )
        if result.error_message:
            print(f"        Error: {result.error_message}", flush=True)


def format_text_report(
    results: collections.abc.Sequence[TestResult],
    device_path: str,
    speed: str,
    backend_name: str,
    total_duration: float,
) -> str:
    """Format test results into a human-readable text report.

    Args:
        results: Sequence of TestResult objects.
        device_path: Path of the target USB device node.
        speed: Operating speed of the USB connection.
        backend_name: Name of the backend used for execution.
        total_duration: Total test duration in seconds.

    Returns:
        Formatted multi-line string table summarizing results.
    """
    lines = []
    lines.append("=" * 65)
    lines.append(" USB Test Results")
    lines.append("=" * 65)
    lines.append(f" Device:   {device_path}")
    lines.append(f" Speed:    {speed}")
    lines.append(f" Backend:  {backend_name}")
    lines.append(f" Duration: {total_duration:.3f} seconds")
    lines.append("-" * 65)

    passed = sum(1 for r in results if r.status == TestStatus.PASS)
    failed = sum(1 for r in results if r.status == TestStatus.FAIL)
    skipped = sum(1 for r in results if r.status == TestStatus.SKIP)
    errors = sum(1 for r in results if r.status == TestStatus.ERROR)

    for r in results:
        status_str = f"[{r.status.value:<5}]"
        if r.status == TestStatus.PASS:
            details = f"{r.duration_secs:8.4f} secs"
            if r.throughput_mbs > 0:
                details += (
                    f" ({r.throughput_mbs:6.2f} MB/s, "
                    f"{r.throughput_mbps:6.2f} Mbps)"
                )
            lines.append(f" {status_str} {r.test_name:<40} {details}")
        elif r.status == TestStatus.SKIP:
            lines.append(
                f" {status_str} {r.test_name:<40} (skipped: {r.error_message})"
            )
        else:
            lines.append(
                f" {status_str} {r.test_name:<40} (error: {r.error_message})"
            )

    lines.append("-" * 65)
    summary_str = (
        f" Summary: {len(results)} executed, {passed} passed, {failed} failed, "
        f"{skipped} skipped, {errors} errors"
    )
    lines.append(summary_str)
    lines.append("=" * 65)
    return "\n".join(lines)


def generate_json_report(
    results: collections.abc.Sequence[TestResult],
    device_path: str,
    speed: str,
    backend_name: str,
    total_duration: float,
    params: TestParams,
) -> dict[str, Any]:
    """Generate a structured dictionary suitable for JSON serialization.

    Args:
        results: Sequence of TestResult objects.
        device_path: Path of the target USB device node.
        speed: Operating speed of the USB connection.
        backend_name: Name of the backend used for execution.
        total_duration: Total test duration in seconds.
        params: TestParams used during execution.

    Returns:
        Dictionary containing metadata, summary counts, and test results.
    """
    passed = sum(1 for r in results if r.status == TestStatus.PASS)
    failed = sum(1 for r in results if r.status == TestStatus.FAIL)
    skipped = sum(1 for r in results if r.status == TestStatus.SKIP)
    errors = sum(1 for r in results if r.status == TestStatus.ERROR)

    non_skipped = len(results) - skipped
    return {
        "metadata": {
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "device_path": device_path,
            "speed": speed,
            "backend": backend_name,
            "total_duration_secs": round(total_duration, 6),
            "parameters": {
                "iterations": params.iterations,
                "length": params.length,
                "vary": params.vary,
                "sglen": params.sglen,
                "buffer_size": params.buffer_size,
                "timeout_ms": params.timeout_ms,
            },
        },
        "summary": {
            "total_executed": len(results),
            "passed": passed,
            "failed": failed,
            "skipped": skipped,
            "errors": errors,
            "success_rate": (
                round(passed / non_skipped * 100, 2) if non_skipped > 0 else 0.0
            ),
        },
        "results": [r.to_dict() for r in results],
    }


class TextReporter:
    """Deprecated namespace; use format_text_report instead."""

    report = staticmethod(format_text_report)


class JSONReporter:
    """Deprecated namespace; use generate_json_report instead."""

    report = staticmethod(generate_json_report)
