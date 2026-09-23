#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import typing as T
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import stdio_redirection
from bazel_action_utils import (
    BazelStderrDebugLineFilter,
    BazelStderrDebugLineRecorder,
    find_prefix_in_input,
)


class FindPrefixInInputTest(unittest.TestCase):
    def test_find_prefix_in_input(self) -> None:
        TEST_CASES = [
            ("foo", "-------", (0, 7)),  # no match
            ("foo", "foo----", (2, 0)),  # full matches
            ("foo", "--foo--", (2, 2)),
            ("foo", "----foo", (2, 4)),
            ("foo", "-----fo", (1, 5)),  # partial matches
            ("foo", "------f", (1, 6)),
        ]
        for prefix, input, expected in TEST_CASES:
            self.assertEqual(
                find_prefix_in_input(prefix, input),
                expected,
                msg=f"For prefix={prefix} and input={input}",
            )


class BazelStderrDebugLineFilterTest(unittest.TestCase):
    def setUp(self) -> None:
        self.output = stdio_redirection.BytesOutputSink()

    def test_no_filtering(self) -> None:
        filter_sink = BazelStderrDebugLineFilter(self.output)
        self.assertTrue(filter_sink.write(b"foooo\nsomethingDEBUG: bar"))
        self.assertEqual(self.output.data, b"foooo\nsomething")
        self.assertTrue(filter_sink.write(b"\nfinish"))
        self.assertEqual(
            self.output.data, b"foooo\nsomethingDEBUG: bar\nfinish"
        )

    def test_no_filtering_colored(self) -> None:
        filter_sink = BazelStderrDebugLineFilter(self.output)
        self.assertTrue(
            filter_sink.write(b"foooo\nsomething\x1b[33mDEBUG: \x1b[0mbar")
        )
        self.assertEqual(self.output.data, b"foooo\nsomething")
        self.assertTrue(filter_sink.write(b"\nfinish"))
        self.assertEqual(
            self.output.data,
            b"foooo\nsomething\x1b[33mDEBUG: \x1b[0mbar\nfinish",
        )

    def test_no_filtering_bad_color(self) -> None:
        filter_sink = BazelStderrDebugLineFilter(self.output)
        self.assertTrue(
            filter_sink.write(b"foooo\nsomething\x1b[31mDEBUG: \x1b[0mbar")
        )
        self.assertEqual(self.output.data, b"foooo\nsomething\x1b[31m")
        self.assertTrue(filter_sink.write(b"\nfinish"))
        self.assertEqual(
            self.output.data,
            b"foooo\nsomething\x1b[31mDEBUG: \x1b[0mbar\nfinish",
        )

    def test_partial_writes(self) -> None:
        filter_sink = BazelStderrDebugLineFilter(self.output)
        self.assertTrue(filter_sink.write(b"foooo\nsomethingDEB"))
        self.assertEqual(self.output.data, b"foooo\nsomething")
        self.assertTrue(filter_sink.write(b"UG: bar"))
        self.assertEqual(self.output.data, b"foooo\nsomething")
        self.assertTrue(filter_sink.write(b"\nfinish"))
        self.assertEqual(
            self.output.data, b"foooo\nsomethingDEBUG: bar\nfinish"
        )

    def test_with_filtering_all(self) -> None:
        filter_sink = BazelStderrDebugLineFilter(self.output, lambda x: True)
        self.assertTrue(
            filter_sink.write(
                b"foooo\nsomething\nDEBUG: bar\nsomething else\nDEBUG: zoo\n"
            )
        )
        self.assertEqual(
            self.output.data, b"foooo\nsomething\nsomething else\n"
        )

    def test_with_filtering_some(self) -> None:
        filter_sink = BazelStderrDebugLineFilter(
            self.output, lambda x: b"SKIP" in x
        )
        self.assertTrue(
            filter_sink.write(
                b"foooo\nsomething\nDEBUG: KEEP ME\nsomething else\nDEBUG: SKIP ME\n"
            )
        )
        self.assertEqual(
            self.output.data,
            b"foooo\nsomething\nDEBUG: KEEP ME\nsomething else\n",
        )


class BazelStderrDebugLineRecorderTest(unittest.TestCase):
    def setUp(self) -> None:
        self.output = stdio_redirection.BytesOutputSink()

    def test_recording(self) -> None:
        prefix_map = {
            "first": b"PREFIX1=",
            "second": b"PREFIX2=",
        }
        recorder = BazelStderrDebugLineRecorder(self.output, prefix_map)

        self.assertTrue(recorder.write(b"line 1\n"))
        self.assertTrue(recorder.write(b"DEBUG: PREFIX1=value 1\n"))
        self.assertTrue(recorder.write(b"line 2\n"))
        self.assertTrue(recorder.write(b"DEBUG: PREFIX2=value 2\n"))
        self.assertTrue(recorder.write(b"DEBUG: PREFIX1=value 3\n"))
        self.assertTrue(recorder.write(b"line 3\n"))

        # Verify the filtered output stream contains the non-debug lines
        self.assertEqual(
            self.output.data,
            b"line 1\nline 2\nline 3\n",
        )

        # Verify recorded values
        self.assertEqual(
            recorder.get_recorded_values("first"), ["value 1", "value 3"]
        )
        self.assertEqual(recorder.get_recorded_values("second"), ["value 2"])

        self.assertDictEqual(
            recorder.get_all_recorded_values(),
            {
                "first": ["value 1", "value 3"],
                "second": ["value 2"],
            },
        )

        # Verify invalid names throw AssertionError
        with self.assertRaises(AssertionError):
            recorder.get_recorded_values("third")

    def test_recording_with_partial_colored_prefix(self) -> None:
        prefix_map = {
            "first": b"PREFIX1=",
            "second": b"PREFIX2=",
        }
        recorder = BazelStderrDebugLineRecorder(self.output, prefix_map)

        self.assertTrue(recorder.write(b"line 1\nDEBUG"))
        self.assertTrue(recorder.write(b": PREFIX1=value 1\n"))

        # The following lines writes a partial colored prefix that
        # includes a full regular prefix (which should be ignored).
        # This used to raise an assertion, see https://fxbug.dev/515325221
        self.assertTrue(recorder.write(b"line 2\n\x1b[33mDEBUG: "))
        self.assertTrue(recorder.write(b"\x1b[0mPREFIX2=value 2\n"))
        self.assertTrue(recorder.write(b"DEBUG: PREFIX1=value 3\n"))
        self.assertTrue(recorder.write(b"line 3\n"))

        # Verify the filtered output stream contains the non-debug lines
        self.assertEqual(
            self.output.data,
            b"line 1\nline 2\nline 3\n",
        )

        # Verify recorded values
        self.assertEqual(
            recorder.get_recorded_values("first"), ["value 1", "value 3"]
        )
        self.assertEqual(recorder.get_recorded_values("second"), ["value 2"])

        self.assertDictEqual(
            recorder.get_all_recorded_values(),
            {
                "first": ["value 1", "value 3"],
                "second": ["value 2"],
            },
        )


class BazelTargetInfosMapTest(unittest.TestCase):
    def test_update_rust_project(self) -> None:
        from bazel_action_utils import BazelTargetInfosMap

        sample_content: list[dict[str, T.Any]] = [
            {
                "type": "file",
                "bazel_target": "//src:foo",
                "bazel_platform_label": "//build/bazel/platforms:host",
                "bazel_platform_config": "host",
                "ninja_depfile": "obj/src/foo.d",
                "gn_targets_manifest": "gen/gn_targets.manifest",
                "stamp_path": "obj/src/foo.stamp",
                "bazel_file": "foo",
                "ninja_file": "foo",
                "update_rust_project": True,
            },
            {
                "type": "file",
                "bazel_target": "//src:bar",
                "bazel_platform_label": "//build/bazel/platforms:host",
                "bazel_platform_config": "host",
                "ninja_depfile": "obj/src/bar.d",
                "gn_targets_manifest": "gen/gn_targets.manifest",
                "stamp_path": "obj/src/bar.stamp",
                "bazel_file": "bar",
                "ninja_file": "bar",
                "update_rust_project": False,
            },
        ]
        target_map = BazelTargetInfosMap(sample_content)
        foo_info = target_map.get_info(
            "//src:foo", "//build/bazel/platforms:host"
        )
        self.assertIsNotNone(foo_info)
        assert foo_info is not None
        self.assertTrue(foo_info.update_rust_project)

        bar_info = target_map.get_info(
            "//src:bar", "//build/bazel/platforms:host"
        )
        self.assertIsNotNone(bar_info)
        assert bar_info is not None
        self.assertFalse(bar_info.update_rust_project)


if __name__ == "__main__":
    unittest.main()
