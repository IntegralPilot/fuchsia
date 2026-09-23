# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import unittest
from pathlib import Path
from subprocess import check_call
from sys import argv, executable

assert len(argv) > 0, "host_test.py expects to be passed fake root"
_fake_build_dir = argv.pop()


class Test(unittest.TestCase):
    _fake_build_dir: Path

    @classmethod
    def setUpClass(cls) -> None:
        cls._fake_build_dir = Path(_fake_build_dir)

    def test_by_inspecting_json(self) -> None:
        with Path(self._fake_build_dir, "tool_paths.json").open("r") as f:
            tool_paths_json = json.load(f)
        # is an array of objects, each with name, path fields

        find_rustdoc_link = [
            o for o in tool_paths_json if o["name"] == "rustdoc-link"
        ]
        self.assertEqual(len(find_rustdoc_link), 1)

        # assert the script exists at the path
        check_call(
            [
                executable,
                # paths in tool_paths.json should be relative to build dir
                self._fake_build_dir / find_rustdoc_link[0]["path"],
                "--help",
            ]
        )
