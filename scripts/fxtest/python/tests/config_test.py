# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import tempfile
import unittest
import unittest.mock as mock

import config


class TestConfig(unittest.TestCase):
    def test_config_loading(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            config_path = os.path.join(td, "testrc")
            with open(config_path, "w") as f:
                f.writelines(
                    [
                        " # This is a comment\n",
                        "--parallel\n",
                        "2\n",
                        "# Another comment\n",
                        "--no-style\n" "--status-lines 40\n\n",
                    ]
                )
            config_file = config.load_config(config_path)
            self.assertTrue(config_file.is_loaded())
            self.assertEqual(config_path, config_file.path)
            self.assertEqual(
                config_file.command_line,
                ["--parallel", "2", "--no-style", "--status-lines", "40"],
            )
            assert config_file.default_flags is not None
            self.assertEqual(config_file.default_flags.parallel, 2)
            self.assertEqual(config_file.default_flags.style, False)
            self.assertEqual(config_file.default_flags.status_lines, 40)

    def test_config_load_fails(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            missing_path = os.path.join(td, "missing")
            config_file = config.load_config(missing_path)
            self.assertFalse(config_file.is_loaded())
            self.assertIsNone(config_file.default_flags)
            self.assertEqual(missing_path, config_file.path)

    @mock.patch.dict(os.environ, {}, clear=True)
    def test_no_home_dir(self) -> None:
        config_file = config.load_config()
        self.assertFalse(config_file.is_loaded())
        self.assertIsNone(config_file.path)

    def test_load_from_home(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            with open(os.path.join(td, ".fxtestrc"), "w") as f:
                f.writelines(
                    [
                        "--parallel 100\n",
                    ]
                )
            with mock.patch.dict(os.environ, {"HOME": td}):
                config_file = config.load_config()
                self.assertTrue(config_file.is_loaded())
                self.assertIsNotNone(config_file.path)
                assert config_file.default_flags is not None
                self.assertEqual(config_file.default_flags.parallel, 100)

    def test_invalid_flags(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            config_path = os.path.join(td, "config")
            with open(config_path, "w") as f:
                f.writelines(
                    [
                        "--invalid-arg 33\n",
                    ]
                )

            self.assertRaises(
                (SystemExit, argparse.ArgumentError),
                lambda: config.load_config(config_path),
            )
