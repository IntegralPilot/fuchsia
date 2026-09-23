# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from pathlib import Path

import gn_runner


class TestGnRunner(unittest.TestCase):
    def test_run_and_extract_output(self):
        build_dir = Path("/tmp/build")
        mock_output = "some output"
        runner = gn_runner.MockGnRunner(build_dir, mock_output)

        output = runner.run_and_extract_output(["desc", "//:default"])

        self.assertEqual(output, mock_output)
        self.assertEqual(runner.last_gn_args(), ["desc", "//:default"])
        self.assertEqual(
            runner._mock_runner.results[-1].args,
            ["gn", "desc", "/tmp/build", "//:default"],
        )

    def test_build_dir(self):
        build_dir = Path("/tmp/build")
        runner = gn_runner.MockGnRunner(build_dir, "")
        self.assertEqual(runner.build_dir, build_dir)


if __name__ == "__main__":
    unittest.main()
