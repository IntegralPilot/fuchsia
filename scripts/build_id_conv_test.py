#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import shutil
import tempfile
import unittest

from build_id_conv import main, touch


# Tests reading and writing ids.txt files.
class TestIdsTxt(unittest.TestCase):
    def setUp(self) -> None:
        # Create a temporary directory.
        self.test_dir = tempfile.mkdtemp()

        # Create a subdirectory in it and write some placeholder symbol files in it.
        self.sofiles_dir = os.path.join(self.test_dir, "sofiles_dir")
        os.mkdir(self.sofiles_dir)
        touch(os.path.join(self.sofiles_dir, "foo.so"))
        touch(os.path.join(self.sofiles_dir, "bar.so"))

    def tearDown(self) -> None:
        shutil.rmtree(self.test_dir)

    # Tests reading/writing ids.txt files without explicit reference input/output directories:
    # - paths read from the input file should be interpreted as relative to its directory
    # - paths written in the output file should be absolute
    def test_read_and_write(self) -> None:
        input_path = os.path.join(self.test_dir, "input-ids.txt")
        output_path = os.path.join(self.test_dir, "output-ids.txt")
        with open(input_path, mode="wt") as input_file:
            input_file.write("12345678 sofiles_dir/foo.so\n")
            input_file.write("abcdef00 sofiles_dir/bar.so\n")

        main(
            [
                "--input",
                os.path.relpath(input_path),
                "--output-format",
                "ids.txt",
                os.path.relpath(output_path),
            ]
        )
        with open(output_path, mode="rt") as output_file:
            self.assertSequenceEqual(
                output_file.readlines(),
                [
                    "12345678 %s\n" % os.path.join(self.sofiles_dir, "foo.so"),
                    "abcdef00 %s\n" % os.path.join(self.sofiles_dir, "bar.so"),
                ],
            )

    # Tests that input paths are expanded correctly if a reference input directory is given.
    def test_read_with_rel_in(self) -> None:
        input_path = os.path.join(self.test_dir, "input-ids.txt")
        output_path = os.path.join(self.test_dir, "output-ids.txt")
        with open(input_path, mode="wt") as input_file:
            input_file.write("12345678 foo.so\n")
            input_file.write("abcdef00 bar.so\n")

        main(
            [
                "--input",
                os.path.relpath(input_path),
                "--ids-rel-to-in",
                os.path.relpath(self.sofiles_dir),
                "--output-format",
                "ids.txt",
                os.path.relpath(output_path),
            ]
        )
        with open(output_path, mode="rt") as output_file:
            self.assertSequenceEqual(
                output_file.readlines(),
                [
                    "12345678 %s\n" % os.path.join(self.sofiles_dir, "foo.so"),
                    "abcdef00 %s\n" % os.path.join(self.sofiles_dir, "bar.so"),
                ],
            )

    # Tests that output paths are written correctly if a reference output directory is given.
    def test_write_with_rel_out(self) -> None:
        input_path = os.path.join(self.test_dir, "input-ids.txt")
        output_path = os.path.join(self.test_dir, "output-ids.txt")
        with open(input_path, mode="wt") as input_file:
            input_file.write("12345678 sofiles_dir/foo.so\n")
            input_file.write("abcdef00 sofiles_dir/bar.so\n")

        main(
            [
                "--input",
                os.path.relpath(input_path),
                "--ids-rel-to-out",
                os.path.relpath(self.sofiles_dir),
                "--output-format",
                "ids.txt",
                os.path.relpath(output_path),
            ]
        )
        with open(output_path, mode="rt") as output_file:
            self.assertSequenceEqual(
                output_file.readlines(),
                [
                    "12345678 foo.so\n",
                    "abcdef00 bar.so\n",
                ],
            )


# Tests reading and writing .build-id directories.
class TestBuildId(unittest.TestCase):
    def setUp(self) -> None:
        # Create a temporary directory.
        self.test_dir = tempfile.mkdtemp()

        # Create a subdirectory in it and write some placeholder symbol files in it.
        self.input_dir = os.path.join(self.test_dir, "input-build-id-dir")
        os.makedirs(os.path.join(self.input_dir, "12"))
        with open(
            os.path.join(self.input_dir, "12/345678.debug"), mode="wt"
        ) as symbol_file:
            symbol_file.write("foo")
        os.makedirs(os.path.join(self.input_dir, "ab"))
        with open(
            os.path.join(self.input_dir, "ab/cdef00.debug"), mode="wt"
        ) as symbol_file:
            symbol_file.write("bar")

    def tearDown(self) -> None:
        shutil.rmtree(self.test_dir)

    # Tests converting a .build-id directory into another .build-id directory containing hard links.
    def test_read_and_write_with_hardlinks(self) -> None:
        output_dir = os.path.join(self.test_dir, "output-build-id-dir")

        main(
            [
                "--input",
                os.path.relpath(self.input_dir),
                "--output-format",
                ".build-id",
                os.path.relpath(output_dir),
            ]
        )
        self.assertTrue(
            os.path.isfile(os.path.join(output_dir, "12/345678.debug"))
        )
        with open(
            os.path.join(output_dir, "12/345678.debug"), mode="rt"
        ) as symbol_file:
            self.assertEqual(symbol_file.read(), "foo")
        self.assertTrue(
            os.path.isfile(os.path.join(output_dir, "ab/cdef00.debug"))
        )
        with open(
            os.path.join(output_dir, "ab/cdef00.debug"), mode="rt"
        ) as symbol_file:
            self.assertEqual(symbol_file.read(), "bar")

    # Tests converting a .build-id directory into another .build-id directory containing symlinks.
    def test_read_and_write_with_symlinks(self) -> None:
        output_dir = os.path.join(self.test_dir, "output-build-id-dir")

        main(
            [
                "--input",
                os.path.relpath(self.input_dir),
                "--output-format",
                ".build-id",
                "--build-id-mode",
                "symlink",
                os.path.relpath(output_dir),
            ]
        )
        self.assertEqual(
            os.readlink(os.path.join(output_dir, "12/345678.debug")),
            os.path.join(self.input_dir, "12/345678.debug"),
        )
        self.assertEqual(
            os.readlink(os.path.join(output_dir, "ab/cdef00.debug")),
            os.path.join(self.input_dir, "ab/cdef00.debug"),
        )


# Tests converting between ids.txt files and .build-id directories.
class TestConversion(unittest.TestCase):
    def test_from_ids_txt_to_build_id_dir(self) -> None:
        with tempfile.TemporaryDirectory() as test_dir:
            # Create an ids.txt file and placeholder symbols files.
            input_path = os.path.join(test_dir, "input-ids.txt")
            with open(input_path, mode="wt") as input_file:
                input_file.write("12345678 foo.so\n")
                input_file.write("abcdef00 bar.so\n")
            touch(os.path.join(test_dir, "foo.so"))
            touch(os.path.join(test_dir, "bar.so"))

            # Convert into a .build-id directory and verify.
            output_dir = os.path.join(test_dir, "output-build-id-dir")
            main(
                [
                    "--input",
                    os.path.relpath(input_path),
                    os.path.relpath(output_dir),
                ]
            )
            self.assertTrue(
                os.path.isfile(os.path.join(output_dir, "12/345678.debug"))
            )
            self.assertTrue(
                os.path.isfile(os.path.join(output_dir, "ab/cdef00.debug"))
            )

    def test_from_build_id_dir_to_ids_txt(self) -> None:
        with tempfile.TemporaryDirectory() as test_dir:
            # Create a .build-id directory with placeholder symbols files in it.
            input_dir = os.path.join(test_dir, "input-build-id-dir")
            os.makedirs(os.path.join(input_dir, "12"))
            touch(os.path.join(input_dir, "12/345678.debug"))
            os.makedirs(os.path.join(input_dir, "ab"))
            touch(os.path.join(input_dir, "ab/cdef00.debug"))

            # Convert into an ids.txt file and verify.
            output_path = os.path.join(test_dir, "output-ids.txt")
            main(
                [
                    "--input",
                    os.path.relpath(input_dir),
                    os.path.relpath(output_path),
                ]
            )
            with open(output_path, mode="rt") as output_file:
                self.assertSequenceEqual(
                    output_file.readlines(),
                    [
                        "12345678 %s\n"
                        % os.path.join(input_dir, "12/345678.debug"),
                        "abcdef00 %s\n"
                        % os.path.join(input_dir, "ab/cdef00.debug"),
                    ],
                )


if __name__ == "__main__":
    unittest.main()
