#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import shutil
import tempfile
import time
import unittest
from unittest import mock

import restat_cacher


class TempFileTransformTests(unittest.TestCase):
    def test_invalid(self):
        self.assertFalse(restat_cacher.TempFileTransform().valid)

    def test_temp_dir_only(self):
        self.assertTrue(
            restat_cacher.TempFileTransform(temp_dir="my_tmp").valid
        )
        self.assertTrue(
            restat_cacher.TempFileTransform(temp_dir="/my_tmp").valid
        )

    def test_basename_prefix_only(self):
        self.assertTrue(
            restat_cacher.TempFileTransform(basename_prefix="temp-").valid
        )

    def test_suffix_only(self):
        self.assertTrue(restat_cacher.TempFileTransform(suffix=".tmp").valid)

    def test_temp_dir_and_suffix(self):
        self.assertTrue(
            restat_cacher.TempFileTransform(
                temp_dir="throw/away", suffix=".tmp"
            ).valid
        )

    def test_transform_suffix_only(self):
        self.assertEqual(
            restat_cacher.TempFileTransform(suffix=".tmp").transform(
                "foo/bar.o"
            ),
            "foo/bar.o.tmp",
        )

    def test_transform_temp_dir_only(self):
        self.assertEqual(
            restat_cacher.TempFileTransform(temp_dir="t/m/p").transform(
                "foo/bar.o"
            ),
            "t/m/p/foo/bar.o",
        )

    def test_transform_suffix_and_temp_dir(self):
        self.assertEqual(
            restat_cacher.TempFileTransform(
                temp_dir="/t/m/p", suffix=".foo"
            ).transform("foo/bar.o"),
            "/t/m/p/foo/bar.o.foo",
        )

    def test_transform_basename_prefix_only(self):
        self.assertEqual(
            restat_cacher.TempFileTransform(basename_prefix="fake.").transform(
                "bar.o"
            ),
            "fake.bar.o",
        )
        self.assertEqual(
            restat_cacher.TempFileTransform(basename_prefix="fake.").transform(
                "foo/bar.o"
            ),
            "foo/fake.bar.o",
        )

    def test_transform_basename_prefix_and_temp_dir(self):
        self.assertEqual(
            restat_cacher.TempFileTransform(
                temp_dir="/t/m/p", basename_prefix="xyz-"
            ).transform("foo/bar.o"),
            "/t/m/p/foo/xyz-bar.o",
        )


class EnsureFileExists(unittest.TestCase):
    def test_file_exists(self):
        with mock.patch.object(
            os.path, "exists", return_value=True
        ) as mock_exists:
            restat_cacher.ensure_file_exists("some/file.txt")
        mock_exists.assert_called()
        # just make sure no exception raised

    def test_file_not_exists(self):
        with mock.patch.object(
            os.path, "exists", return_value=False
        ) as mock_exists:
            with mock.patch.object(time, "sleep") as mock_sleep:
                with self.assertRaises(FileNotFoundError):
                    restat_cacher.ensure_file_exists("not/a/file.txt")
        mock_exists.assert_called()

    def test_file_exists_later(self):
        with mock.patch.object(
            os.path, "exists", side_effect=[False, True]
        ) as mock_exists:
            with mock.patch.object(time, "sleep") as mock_sleep:
                restat_cacher.ensure_file_exists("late/file.txt")
        mock_exists.assert_called()
        mock_sleep.assert_called()
        # just make sure no exception raised


class MoveIfIdenticalTests(unittest.TestCase):
    def test_nonexistent_source(self):
        with mock.patch.object(
            os.path, "exists", return_value=False
        ) as mock_exists:
            with mock.patch.object(shutil, "move") as mock_move:
                with mock.patch.object(restat_cacher, "remove") as mock_remove:
                    with mock.patch.object(time, "sleep") as mock_sleep:
                        with self.assertRaises(FileNotFoundError):
                            restat_cacher.move_if_identical(
                                "source.txt", "dest.txt"
                            )
        mock_exists.assert_called()
        mock_move.assert_not_called()
        mock_remove.assert_not_called()

    def test_updating_output(self):
        with mock.patch.object(
            os.path, "exists", return_value=True
        ) as mock_exists:
            with mock.patch.object(
                restat_cacher, "files_match", return_value=False
            ) as mock_diff:
                with mock.patch.object(shutil, "move") as mock_move:
                    with mock.patch.object(
                        restat_cacher, "remove"
                    ) as mock_remove:
                        moved = restat_cacher.move_if_identical(
                            "source2.txt", "dest2.txt"
                        )
        self.assertFalse(moved)
        mock_exists.assert_called()
        mock_diff.assert_called()
        mock_move.assert_not_called()
        mock_remove.assert_called_with("source2.txt")

    def test_cached_output(self):
        with mock.patch.object(
            os.path, "exists", return_value=True
        ) as mock_exists:
            with mock.patch.object(
                restat_cacher, "files_match", return_value=True
            ) as mock_diff:
                with mock.patch.object(shutil, "move") as mock_move:
                    with mock.patch.object(
                        restat_cacher, "remove"
                    ) as mock_remove:
                        with mock.patch.object(
                            restat_cacher, "ensure_file_exists"
                        ) as mock_exists:
                            moved = restat_cacher.move_if_identical(
                                "source3.txt", "dest3.txt"
                            )
        self.assertTrue(moved)
        mock_exists.assert_called()
        mock_diff.assert_called()
        mock_move.assert_called_with("source3.txt", "dest3.txt")
        mock_remove.assert_not_called()


class ActionRunCachedTests(unittest.TestCase):
    def test_touch_twice(self):
        with tempfile.TemporaryDirectory() as tempdir:
            output = os.path.join(tempdir, __name__ + "-output.txt")
            action = restat_cacher.Action(
                command=["touch", output],
                outputs=[output],
            )
            tempfile_transform = restat_cacher.TempFileTransform(suffix=".bkp")

            # First time, output is fresh
            first_exit_code = action.run_cached(
                tempfile_transform=tempfile_transform
            )
            self.assertEqual(first_exit_code, 0)
            orig_mtime = os.path.getmtime(output)

            # Run it a second time, expecting output to be cached, mtime preserved
            time.sleep(0.1)
            second_exit_code = action.run_cached(
                tempfile_transform=tempfile_transform
            )
            self.assertEqual(second_exit_code, 0)
            new_mtime = os.path.getmtime(output)
            self.assertEqual(new_mtime, orig_mtime)

    def test_updated_output(self):
        with tempfile.TemporaryDirectory() as tempdir:
            output = os.path.join(tempdir, __name__ + "-output.txt")
            with open(output, "w") as f:  # pre-populate file with contents
                f.write("not empty\n")

            orig_mtime = os.path.getmtime(output)

            action = restat_cacher.Action(
                command=["touch", output],  # expect to create empty file
                outputs=[output],
            )
            tempfile_transform = restat_cacher.TempFileTransform(suffix=".bkp")
            time.sleep(0.1)
            exit_code = action.run_cached(tempfile_transform=tempfile_transform)
            self.assertEqual(exit_code, 0)
            new_mtime = os.path.getmtime(output)
            self.assertGreater(new_mtime, orig_mtime)

    def test_command_failed(self):
        with tempfile.TemporaryDirectory() as tempdir:
            output = os.path.join(tempdir, __name__ + "-output.txt")
            with open(output, "w") as f:  # pre-populate file with contents
                f.write("not empty\n")

            orig_mtime = os.path.getmtime(output)

            action = restat_cacher.Action(
                command=["false"],  # exit 1 (failure)
                outputs=[output],
            )
            tempfile_transform = restat_cacher.TempFileTransform(suffix=".bkp")
            time.sleep(0.1)
            exit_code = action.run_cached(tempfile_transform=tempfile_transform)
            self.assertEqual(exit_code, 1)
            new_mtime = os.path.getmtime(output)
            # existing file is still stale, after being restored
            self.assertEqual(new_mtime, orig_mtime)


if __name__ == "__main__":
    unittest.main()
