#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from gn_labels import GnLabelQualifier, qualify_gn_target_name, split_gn_label


class GnLabelsTest(unittest.TestCase):
    def test_split_gn_label(self) -> None:
        _INVALID_CASES = [
            # Does not start with //
            "foo/bar",
            # Extra junk after toolchain suffix
            "//foo(//toolchain)extrajunk",
            # No closing paren for toolchain suffix
            "//foo(//toolchain",
            # Missing open paren
            "//foo//toolchain"
            # Malformed content inside paren
            "//foo(toolchain//)",
        ]

        for label in _INVALID_CASES:
            with self.assertRaises(AssertionError):
                split_gn_label(label)

        _TEST_CASES = {
            "//foo": ("//foo", ""),
            "//:foo": ("//:foo", ""),
            "//foo/bar": ("//foo/bar", ""),
            "//foo/bar:bar": ("//foo/bar:bar", ""),
            "//foo(//toolchain)": ("//foo", "//toolchain"),
            "//foo(//build/toolchain/fuchsia:y64)": (
                "//foo",
                "//build/toolchain/fuchsia:y64",
            ),
        }

        for label, target_toolchain in _TEST_CASES.items():
            self.assertEqual(
                target_toolchain,
                split_gn_label(label),
                f"When splitting [{label}]",
            )

    def test_qualify_gn_target_name(self) -> None:
        _INVALID_CASES = [
            # Does not start with //
            "foo/bar",
        ]
        for label in _INVALID_CASES:
            with self.assertRaises(AssertionError):
                qualify_gn_target_name(label)

        _TEST_CASES = {
            "//foo": "//foo:foo",
            "//foo:foo": "//foo:foo",
            "//:foo": "//:foo",
            "//foo/bar": "//foo/bar:bar",
        }
        for label, expected in _TEST_CASES.items():
            self.assertEqual(
                expected,
                qualify_gn_target_name(label),
                f"When qualifying [{label}]",
            )

    def test_GnLabelQualifier_qualify_toolchain(self) -> None:
        qualifier = GnLabelQualifier("y64", "aRm64")

        _TOOLCHAIN_TEST_CASES = {
            "//toolchain": "//toolchain:toolchain",
            "//toolchain:name": "//toolchain:name",
            "//toolchain/name": "//toolchain/name:name",
            "host": "//build/toolchain:host_y64",
            "default": "//build/toolchain/fuchsia:aRm64",
            "fuchsia": "//build/toolchain/fuchsia:aRm64",
            "fidl": "//build/fidl:fidling",
            "toolchain": "ERROR",
        }
        for toolchain, expected in _TOOLCHAIN_TEST_CASES.items():
            msg = f"When qualifying {toolchain}"
            if expected == "ERROR":
                with self.assertRaises(AssertionError, msg=msg):
                    qualifier.qualify_toolchain(toolchain)
            else:
                self.assertEqual(
                    expected, qualifier.qualify_toolchain(toolchain), msg=msg
                )

    def test_GnLabelQualifier_qualify_label(self) -> None:
        qualifier = GnLabelQualifier("y64", "aRm64")

        _LABEL_TEST_CASES = {
            "//foo": "//foo:foo",
            "//foo(default)": "//foo:foo",
            "//foo(//build/toolchain/fuchsia:aRm64)": "//foo:foo",
            "//foo(host)": "//foo:foo(//build/toolchain:host_y64)",
            "//foo(//build/toolchain/fuchsia:y64)": "//foo:foo(//build/toolchain/fuchsia:y64)",
            "//foo(fidl)": "//foo:foo(//build/fidl:fidling)",
            "//foo/bar": "//foo/bar:bar",
            "//foo/bar:zoo": "//foo/bar:zoo",
            "//foo/bar(default)": "//foo/bar:bar",
            "//foo/bar:zoo(default)": "//foo/bar:zoo",
            "//foo/bar(fuchsia)": "//foo/bar:bar",
            "//foo/bar:zoo(fuchsia)": "//foo/bar:zoo",
            "//foo/bar(//build/toolchain/fuchsia:aRm64)": "//foo/bar:bar",
            "//foo/bar:zoo(//build/toolchain/fuchsia:aRm64)": "//foo/bar:zoo",
            "//foo/bar(//build/toolchain/fuchsia:y64)": "//foo/bar:bar(//build/toolchain/fuchsia:y64)",
            "//foo/bar:zoo(//build/toolchain/fuchsia:y64)": "//foo/bar:zoo(//build/toolchain/fuchsia:y64)",
        }
        for label, expected in _LABEL_TEST_CASES.items():
            msg = f"When qualifying {label}"
            if expected == "ERROR":
                with self.assertRaises(AssertionError, msg=msg):
                    qualifier.qualify_label(label)
            else:
                self.assertEqual(
                    expected, qualifier.qualify_label(label), msg=msg
                )

    def test_GnLabelQualifier_label_to_build_args(self) -> None:
        qualifier = GnLabelQualifier("y64", "aRm64")
        qualifier.set_ninja_path_to_gn_label(lambda x: f"NINJA_PATH<{x}>")

        _TEST_CASES = {
            "//foo": ["//foo"],
            "//foo:foo": ["//foo"],
            "//foo:bar": ["//foo:bar"],
            "//foo(//build/toolchain:host_y64)": ["--host", "//foo"],
            "//foo:foo(//build/toolchain/fuchsia:aRm64)": ["//foo"],
            "//foo/bar:bar(//build/toolchain/fuchsia:riscv64)": [
                "--toolchain=//build/toolchain/fuchsia:riscv64",
                "//foo/bar",
            ],
            "//foo/bar:bar(//build/fidl:fidling)": ["--fidl", "//foo/bar"],
            "foo": ["ERROR"],
        }
        for label, expected in _TEST_CASES.items():
            msg = f"When parsing {label}"
            if expected == ["ERROR"]:
                with self.assertRaises(AssertionError, msg=msg):
                    qualifier.label_to_build_args(label)
            else:
                self.assertListEqual(
                    expected, qualifier.label_to_build_args(label), msg=msg
                )

    def test_GnLabelQualifier_build_args_to_labels(self) -> None:
        qualifier = GnLabelQualifier("y64", "aRm64")
        qualifier.set_ninja_path_to_gn_label(lambda x: f"NINJA_PATH<{x}>")

        _TEST_CASES = [
            (
                ["//foo"],
                ["//foo:foo"],
            ),
            (
                ["//foo(host)"],
                ["//foo:foo(//build/toolchain:host_y64)"],
            ),
            (
                ["--host", "//foo"],
                ["//foo:foo(//build/toolchain:host_y64)"],
            ),
            (
                [
                    "//foo",
                    "--host",
                    "//foo:bar",
                    "--toolchain=fidl",
                    "//fidl:zoo",
                ],
                [
                    "//foo:foo",
                    "//foo:bar(//build/toolchain:host_y64)",
                    "//fidl:zoo(//build/fidl:fidling)",
                ],
            ),
            (
                ["--toolchain=host", "//foo"],
                ["//foo:foo(//build/toolchain:host_y64)"],
            ),
            (
                ["//foo", "--unknown"],
                ["ERROR"],
            ),
            (
                ["//foo", "host_x64/foo"],
                ["//foo:foo", "NINJA_PATH<host_x64/foo>"],
            ),
        ]
        for labels, expected_labels in _TEST_CASES:
            msg = f"When parsing {labels}"
            if expected_labels == ["ERROR"]:
                with self.assertRaises(AssertionError, msg=msg):
                    qualifier.build_args_to_labels(labels)
            else:
                self.assertListEqual(
                    expected_labels,
                    qualifier.build_args_to_labels(labels),
                    msg=msg,
                )


if __name__ == "__main__":
    unittest.main()
