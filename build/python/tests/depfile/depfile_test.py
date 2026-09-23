#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import io
import os
import tempfile
import unittest

from depfile import DepFile


class DepFileTests(unittest.TestCase):
    """Validate the depfile generation

    This validates the rebasing behavior using the following imaginary set of
    files::

        /foo/
             bar/
                 baz/
                     output
                 things/
                        input_a
                        input_b
             input_c

    Assume a CWD of /foo/bar
    """

    expected = "baz/output: \\\n  ../input_c \\\n  things/input_a \\\n  things/input_b\n"

    def test_specified_cwd(self) -> None:
        output = "/foo/bar/baz/output"
        input_a = "/foo/bar/things/input_a"
        input_b = "/foo/bar/things/input_b"
        input_c = "/foo/input_c"

        rebased_depfile = DepFile(output, rebase="/foo/bar")
        rebased_depfile.add_input(input_a)
        rebased_depfile.add_input(input_b)
        rebased_depfile.update([input_b, input_c])

        self.assertEqual(str(rebased_depfile), DepFileTests.expected)

    def test_inferred_cwd(self) -> None:
        """Validate the standard behavior, with a mix of absolute and real paths."""

        # make the output absolute (from a path relative to the cwd)
        output = os.path.abspath("baz/output")
        input_a = os.path.abspath("things/input_a")
        input_b = "things/input_b"
        input_c = os.path.abspath("../input_c")

        depfile = DepFile(output)
        depfile.update([input_a, input_b, input_c])

        self.assertEqual(str(depfile), DepFileTests.expected)

    def test_depfile_writing(self) -> None:
        depfile = DepFile("/foo/bar/baz/output", rebase="/foo/bar")
        depfile.update(
            [
                "/foo/bar/things/input_a",
                "/foo/bar/things/input_b",
                "/foo/input_c",
            ]
        )

        with tempfile.TemporaryFile("w+") as outfile:
            # Write out the depfile
            depfile.write_to(outfile)

            # Read the contents back in
            outfile.seek(0)
            contents = outfile.read()
            self.assertEqual(contents, DepFileTests.expected)

    def test_empty(self) -> None:
        depfile = DepFile("foo/bar/baz/output")
        self.assertEqual(str(depfile), "foo/bar/baz/output:\n")

    def test_extra_outputs(self) -> None:
        depfile = DepFile("foo/bar/baz/output")
        depfile.add_output("some/other/output")
        self.assertEqual(
            str(depfile), "foo/bar/baz/output some/other/output:\n"
        )

    def test_spaces_in_paths(self) -> None:
        depfile = DepFile("foo/bar/output with space")
        depfile.add_input("things/input with space")
        self.assertEqual(
            str(depfile),
            "foo/bar/output\\ with\\ space: \\\n  things/input\\ with\\ space\n",
        )


class DepFileSingleLineReadingTests(unittest.TestCase):
    def test_single_output_and_input(self) -> None:
        raw = "some/output: some/input"
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output"])
        self.assertEqual(depfile.deps, set(["some/input"]))

    def test_spaces_in_paths(self) -> None:
        raw = "some/output\\ with\\ space: some/input\\ with\\ space"
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output with space"])
        self.assertEqual(depfile.deps, set(["some/input with space"]))

    def test_single_output_and_multiple_inputs(self) -> None:
        raw = "some/output: some/input1 some/input2"
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output"])
        self.assertEqual(depfile.deps, set(["some/input1", "some/input2"]))

    def test_multiple_outputs_and_multiple_inputs(self) -> None:
        raw = "some/output1 some/output2: some/input1 some/input2"
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output1", "some/output2"])
        self.assertEqual(depfile.deps, set(["some/input1", "some/input2"]))


class DepFileMultiLineReadingTests(unittest.TestCase):
    def test_single_output_and_input(self) -> None:
        raw = """some/output: \
some/input """
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output"])
        self.assertEqual(depfile.deps, set(["some/input"]))

    def test_single_output_and_input_with_trailing_continuation(self) -> None:
        raw = """some/output: \
some/input \
"""
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output"])
        self.assertEqual(depfile.deps, set(["some/input"]))

    def test_multiple_outputs_and_inputs(self) -> None:
        raw = """some/output1 some/output2: some/input1 some/input2 \
some/input3 \
"""
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output1", "some/output2"])
        self.assertEqual(
            depfile.deps, set(["some/input1", "some/input2", "some/input3"])
        )

    def test_literal_backslash_continuation(self) -> None:
        raw = "some/output: \\\n  some/input1 \\\n  some/input2\n"
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(depfile.outputs, ["some/output"])
        self.assertEqual(depfile.deps, set(["some/input1", "some/input2"]))

    def test_multiple_rules(self) -> None:
        raw = """some/output1.d: some/input1 some/input2
some/output1.rmeta: \\
  some/input1 \\
  some/input3
"""
        depfile = DepFile.read_from(io.StringIO(raw))
        self.assertEqual(
            depfile.outputs, ["some/output1.d", "some/output1.rmeta"]
        )
        self.assertEqual(
            depfile.deps, set(["some/input1", "some/input2", "some/input3"])
        )


if __name__ == "__main__":
    unittest.main()
