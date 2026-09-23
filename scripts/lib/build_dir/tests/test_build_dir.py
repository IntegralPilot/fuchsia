# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
import unittest.mock as mock

import build_dir


class TestBuildDir(unittest.TestCase):
    @mock.patch.dict("os.environ", clear=True)
    def test_missing(self) -> None:
        self.assertRaises(
            build_dir.GetBuildDirectoryError,
            lambda: build_dir.get_build_directory(),
        )

    @mock.patch.dict(
        "os.environ", {"FUCHSIA_BUILD_DIR_FROM_FX": "/tmp/out/foo"}
    )
    def test_present(self) -> None:
        self.assertEqual(
            "/tmp/out/foo", build_dir.get_build_directory().as_posix()
        )
