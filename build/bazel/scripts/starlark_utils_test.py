#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import unittest

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import starlark_utils


class ToStarlarkExprTest(unittest.TestCase):
    def test_format(self) -> None:
        TEST_CASES = [
            # String inputs
            ("foo bar", '"foo bar"'),
            ("foo\nbar", '"foo\\nbar"'),
            ("\nfoo\nbar\n", '"\\nfoo\\nbar\\n"'),
            ("foo\tbar", '"foo\\tbar"'),
            ('foo "bar"', '"foo \\"bar\\""'),
            ("foo\r\nbar\r\n", '"foo\\r\\nbar\\r\\n"'),
            ("foo\0bar", '"foo\\u00bar"'),
            ("foo\\bar\\zoo", '"foo\\\\bar\\\\zoo"'),
            ("foo'bar", '"foo\'bar"'),
            # Integer inputs
            (1, "1"),
            (-2, "-2"),
            (42, "42"),
            # List inputs
            ([], "[]"),
            ([1], "[1]"),
            (["foo"], '["foo"]'),
            ([1, 2], "[\n    1,\n    2\n]"),
            ([1, 2, 3], "[\n    1,\n    2,\n    3\n]"),
            (["foo", "bar"], '[\n    "foo",\n    "bar"\n]'),
            # Dictionary inputs
            ({}, "{}"),
            ({1: 2}, "{1: 2}"),
            ({1: 2, 3: 4}, "{\n    1: 2,\n    3: 4\n}"),
            (
                {"foo": [1, 2], "bar": [3, 4]},
                """{
    "foo": [
        1,
        2
    ],
    "bar": [
        3,
        4
    ]
}""",
            ),
        ]
        for input, expected in TEST_CASES:
            self.assertEqual(
                starlark_utils.to_starlark_expr(input),
                expected,
                f"from {repr(input)}",
            )


if __name__ == "__main__":
    unittest.main()
