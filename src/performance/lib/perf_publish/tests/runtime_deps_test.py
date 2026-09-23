#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for the utils.py."""

import pathlib
import tempfile
import unittest

import perf_publish.publish as publish


class RuntimeDepsDirTest(unittest.TestCase):
    """Tests Runtime-deps-dir helper"""

    def test_get_associated_runtime_deps_dir_success(self) -> None:
        """
        Check that get_associated_runtime_deps_dir() returns successfully.
        """
        with tempfile.TemporaryDirectory() as tmpdir:
            search_path = pathlib.Path(tmpdir) / "runtime_deps" / "dir"
            search_path.mkdir(mode=711, parents=True, exist_ok=True)
            res = publish.get_associated_runtime_deps_dir(search_path)
            self.assertEqual(pathlib.Path(tmpdir) / "runtime_deps", res)

    def test_get_runtime_deps_ancestor_path_not_found(self) -> None:
        """
        Check that get_associated_runtime_deps_dir() raises exception if not
        found.
        """
        with tempfile.TemporaryDirectory() as tmpdir:
            with self.assertRaises(ValueError):
                publish.get_associated_runtime_deps_dir(tmpdir)
