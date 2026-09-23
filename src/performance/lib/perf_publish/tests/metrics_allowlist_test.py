#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for the metrics allowlist code."""

import os
import tempfile
import unittest

import perf_publish.metrics_allowlist as metrics_allowlist


class CatapultConverterTest(unittest.TestCase):
    """Catapult converter metric publishing tests"""

    def test_expected_metrics(self) -> None:
        """Test case that asserts the expected test metrics and ignoring comments"""
        with tempfile.TemporaryDirectory() as tmpdir:
            metrics_file = os.path.join(tmpdir, "metrics.txt")
            with open(metrics_file, "w") as f:
                f.write(
                    "# Comment line\n\n"
                    "fuchsia.suite1: foo\n"
                    "fuchsia.suite1: bar\n"
                )
            allowlist = metrics_allowlist.MetricsAllowlist(metrics_file)
        self.assertSetEqual(
            allowlist.expected_metrics,
            {"fuchsia.suite1: foo", "fuchsia.suite1: bar"},
        )
        self.assertSetEqual(allowlist.optional_metrics, frozenset())
        self.assertTrue(allowlist.should_summarize)

        # This succeeds without raising any exception.
        allowlist.check({"fuchsia.suite1: foo", "fuchsia.suite1: bar"})

        with self.assertRaises(ValueError) as exception_context:
            allowlist.check({"fuchsia.suite1: foo", "fuchsia.suite1: new"})

        self.assertIn(
            ("\t-fuchsia.suite1: bar\n" "\t+fuchsia.suite1: new"),
            str(exception_context.exception),
        )

    def test_optional_metrics(self) -> None:
        """Test case that asserts optional metrics behavior"""
        with tempfile.TemporaryDirectory() as tmpdir:
            metrics_file = os.path.join(tmpdir, "metrics.txt")
            with open(metrics_file, "w") as f:
                f.write(
                    "fuchsia.suite1: foo\n"
                    "fuchsia.suite1: bar\n"
                    "fuchsia.suite1: opt1 [optional]\n"
                    "fuchsia.suite1: opt2 [optional]\n"
                )
            allowlist = metrics_allowlist.MetricsAllowlist(metrics_file)

        self.assertSetEqual(
            allowlist.expected_metrics,
            {"fuchsia.suite1: foo", "fuchsia.suite1: bar"},
        )
        self.assertSetEqual(
            allowlist.optional_metrics,
            {"fuchsia.suite1: opt1", "fuchsia.suite1: opt2"},
        )

        # These succeed without raising an exception.
        allowlist.check({"fuchsia.suite1: foo", "fuchsia.suite1: bar"})
        allowlist.check(
            {
                "fuchsia.suite1: foo",
                "fuchsia.suite1: bar",
                "fuchsia.suite1: opt1",
            }
        )

        with self.assertRaises(ValueError) as exception_context:
            allowlist.check(
                {
                    "fuchsia.suite1: foo",
                    "fuchsia.suite1: new",
                    "fuchsia.suite1: opt2",
                }
            )

        self.assertIn(
            ("\t-fuchsia.suite1: bar\n" "\t+fuchsia.suite1: new"),
            str(exception_context.exception),
        )

    def test_no_summarize(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            metrics_file = os.path.join(tmpdir, "metrics.txt")
            with open(metrics_file, "w") as f:
                f.write(
                    "[no-summarize-metrics]\n"
                    "# Comment line\n\n"
                    "fuchsia.suite1: foo\n"
                    "fuchsia.suite1: bar\n"
                )
            allowlist = metrics_allowlist.MetricsAllowlist(metrics_file)
        self.assertSetEqual(
            allowlist.expected_metrics,
            {"fuchsia.suite1: foo", "fuchsia.suite1: bar"},
        )
        self.assertSetEqual(allowlist.optional_metrics, frozenset())
        self.assertFalse(allowlist.should_summarize)

    def test_command_can_be_present_after_comments_or_blank_lines(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            metrics_file = os.path.join(tmpdir, "metrics.txt")
            with open(metrics_file, "w") as f:
                f.write(
                    "# Comment line\n\n"
                    "[no-summarize-metrics]\n"
                    "fuchsia.suite1: foo\n"
                    "fuchsia.suite1: bar\n"
                )
            allowlist = metrics_allowlist.MetricsAllowlist(metrics_file)
        self.assertSetEqual(
            allowlist.expected_metrics,
            {"fuchsia.suite1: foo", "fuchsia.suite1: bar"},
        )

    def test_command_rejected_in_between_metrics(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            metrics_file = os.path.join(tmpdir, "metrics.txt")
            with open(metrics_file, "w") as f:
                f.write(
                    "# Comment line\n\n"
                    "fuchsia.suite1: foo\n"
                    "[no-summarize-metrics]\n"
                    "fuchsia.suite1: bar\n"
                )
            with self.assertRaises(ValueError) as exception_context:
                metrics_allowlist.MetricsAllowlist(metrics_file)
        self.assertIn(
            "[no-summarize-metrics] can only appear at the beginning of the file",
            str(exception_context.exception),
        )
