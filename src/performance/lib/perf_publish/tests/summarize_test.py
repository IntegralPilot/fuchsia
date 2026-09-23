#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for the metrics allowlist code."""

import io
import json
import operator
import os
import tempfile
import unittest

import perf_publish.summarize as summarize


class SummarizeTest(unittest.TestCase):
    """Fuchsiaperf json summarization tests"""

    def test_mean_excluding_warm_up(self) -> None:
        """Validates the mean without warm up value"""
        self.assertEqual(
            summarize.mean_excluding_warm_up([999, 200, 201, 202, 203]), 201.5
        )
        self.assertEqual(summarize.mean_excluding_warm_up([999, 123]), 123)
        self.assertEqual(summarize.mean_excluding_warm_up([999.88]), 999.88)
        self.assertEqual(summarize.mean_excluding_warm_up([999]), 999)
        # The int (non-double) type should be preserved for single-item inputs.
        self.assertTrue(
            isinstance(summarize.mean_excluding_warm_up([999]), int)
        )
        with self.assertRaises(ValueError):
            summarize.mean_excluding_warm_up([])

    def test_summarize_results_simple_case(self) -> None:
        """
        Simple case where we have one set of results (corresponding to a single process run)
        of a single test case.
        """
        with tempfile.NamedTemporaryFile(delete=False, mode="w") as f:
            json.dump(
                [
                    {
                        "label": "test1",
                        "test_suite": "suite1",
                        "unit": "nanoseconds",
                        "values": [200, 101, 102, 103, 104, 105],
                    }
                ],
                f,
            )
            f.close()
            results = summarize.summarize_perf_files([f.name])
        self.assertEqual(
            results,
            [
                {
                    "label": "test1",
                    "test_suite": "suite1",
                    "unit": "nanoseconds",
                    "values": [103.0],
                }
            ],
        )

    def test_summarize_result_mean_of_means(self) -> None:
        """
        Test summarize_perf_files() on a more complex case: We have
        multiple entries (corresponding to multiple process runs), and we
        take the mean of each entry's mean_excluding_warm_up.
        """
        with tempfile.TemporaryDirectory() as tmpdir:
            file1 = os.path.join(tmpdir, "file1.fuchsiaperf.json")
            with open(file1, "w") as f:
                json.dump(
                    [
                        {
                            "label": "test1",
                            "test_suite": "suite1",
                            "unit": "nanoseconds",
                            # This gives a mean_excluding_warm_up of 110.
                            "values": [2000, 108, 109, 110, 111, 112],
                        }
                    ],
                    f,
                )

            file2 = os.path.join(tmpdir, "file2.fuchsiaperf.json")
            with open(file2, "w") as f:
                json.dump(
                    [
                        {
                            "label": "test1",
                            "test_suite": "suite1",
                            "unit": "nanoseconds",
                            # This gives a mean_excluding_warm_up of 210.
                            "values": [3000, 207, 208, 209, 210, 211, 212, 213],
                        }
                    ],
                    f,
                )
            results = summarize.summarize_perf_files([file1, file2])
        self.assertEqual(
            results,
            [
                {
                    "label": "test1",
                    "test_suite": "suite1",
                    "unit": "nanoseconds",
                    # This expected value is the mean of [110, 210].
                    "values": [160.0],
                }
            ],
        )

    def test_summarize_results_fractions(self) -> None:
        """
        Test that summarize_perf_files() works with fractional
        values.  Use test values that can be represented exactly by
        floating point numbers.
        """
        with tempfile.TemporaryDirectory() as tmpdir:
            file1 = os.path.join(tmpdir, "file1.fuchsiaperf.json")
            with open(file1, "w") as f:
                json.dump(
                    [
                        {
                            "label": "test1",
                            "test_suite": "suite1",
                            "unit": "nanoseconds",
                            # This gives a mean_excluding_warm_up of 2 / 256.
                            "values": [5000, 1 / 256, 3 / 256],
                        }
                    ],
                    f,
                )
            file2 = os.path.join(tmpdir, "file2.fuchsiaperf.json")
            with open(file2, "w") as f:
                json.dump(
                    [
                        {
                            "label": "test1",
                            "test_suite": "suite1",
                            "unit": "nanoseconds",
                            # This gives a mean_excluding_warm_up of 6 / 256.
                            "values": [5000, 5 / 256, 7 / 256],
                        }
                    ],
                    f,
                )
            results = summarize.summarize_perf_files([file1, file2])
        self.assertEqual(
            results,
            [
                {
                    "label": "test1",
                    "test_suite": "suite1",
                    "unit": "nanoseconds",
                    "values": [4 / 256],
                }
            ],
        )

    def test_summarize_results_multiple_metrics(self) -> None:
        """Test case that asserts that a set of files is summarized"""
        with tempfile.TemporaryDirectory() as tmpdir:
            file1 = os.path.join(tmpdir, "file1.fuchsiaperf.json")
            with open(file1, "w") as f:
                json.dump(
                    [
                        {
                            "label": "metric_1",
                            "test_suite": "fuchsia.my.benchmark",
                            "unit": "ms",
                            "values": [1, 2, 3, 4],
                        },
                        {
                            "label": "metric_2",
                            "test_suite": "fuchsia.my.benchmark",
                            "unit": "ms",
                            "values": [9],
                        },
                        {
                            "label": "metric_1",
                            "test_suite": "fuchsia.my.benchmark",
                            "unit": "ms",
                            "values": [5, 6, 7],
                        },
                    ],
                    f,
                )
            file2 = os.path.join(tmpdir, "file2.fuchsiaperf.json")
            with open(file2, "w") as f:
                json.dump(
                    [
                        {
                            "label": "metric_1",
                            "test_suite": "fuchsia.my.benchmark",
                            "unit": "ms",
                            "values": [5, 6, 7, 8],
                        },
                        {
                            "label": "metric_3",
                            "test_suite": "fuchsia.my.benchmark",
                            "unit": "ms",
                            "values": [9, 10],
                        },
                    ],
                    f,
                )
            results = summarize.summarize_perf_files([file1, file2])
        self.assertEqual(
            sorted(results, key=operator.itemgetter("label")),
            [
                {
                    "label": "metric_1",
                    "test_suite": "fuchsia.my.benchmark",
                    "unit": "ms",
                    "values": [5.5],
                },
                {
                    "label": "metric_2",
                    "test_suite": "fuchsia.my.benchmark",
                    "unit": "ms",
                    "values": [9],
                },
                {
                    "label": "metric_3",
                    "test_suite": "fuchsia.my.benchmark",
                    "unit": "ms",
                    "values": [10],
                },
            ],
        )

    def test_summarize_check_unit(self) -> None:
        with tempfile.NamedTemporaryFile(mode="w", delete=False) as f:
            json.dump(
                [
                    {
                        "label": "metric_1",
                        "test_suite": "fuchsia.my.benchmark",
                        "unit": "ms",
                        "values": [1, 2, 3, 4],
                    },
                    {
                        "label": "metric_1",
                        "test_suite": "fuchsia.my.benchmark",
                        "unit": "ns",
                        "values": [9],
                    },
                ],
                f,
            )
            f.close()
            with self.assertRaises(ValueError) as context:
                summarize.summarize_perf_files([f.name])
            self.assertIn("Inconsistent units", str(context.exception))

    def test_write_fuchsiaperf_json_round_trip(self) -> None:
        """
        Check that JSON data written to a file using
        write_fuchsiaperf_json() can be read back successfully and give the
        same value.
        """
        with tempfile.NamedTemporaryFile(delete=False, mode="wb") as temp_file:
            f = io.TextIOWrapper(temp_file)
            json_data = [
                {
                    "foo": "bar",
                    "list": [1, 2, 3],
                },
                {
                    "foo": "bar2",
                    "list": [4, 5, 6],
                },
                "string",
                [4, 5, 6],
            ]
            summarize.write_fuchsiaperf_json(f, json_data)
            f.close()
            with open(f.name, "r", encoding="utf-8") as f:
                self.assertEqual(json.load(f), json_data)

    def test_write_fuchsia_perf_json_newlines(self) -> None:
        """
        Check that write_fuchsiaperf_json() outputs a newline after each
        top-level entry.
        """
        with tempfile.NamedTemporaryFile(delete=False, mode="wb") as temp_file:
            f = io.TextIOWrapper(temp_file)
            summarize.write_fuchsiaperf_json(f, ["foo", "bar", "qux"])
            f.close()
            with open(f.name, "r", encoding="utf-8") as f:
                self.assertEqual(f.read(), '["foo",\n"bar",\n"qux"]\n')
