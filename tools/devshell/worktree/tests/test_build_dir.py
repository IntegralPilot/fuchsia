# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import tempfile
import unittest
from pathlib import Path

# Add tools/devshell/worktree to sys.path
worktree_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, worktree_dir)

from build_dir import BuildDir, BuildStatus


class TestBuildDir(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.wt_root = Path(self.temp_dir.name)
        self.build_path = self.wt_root / "out" / "default"
        self.build_path.mkdir(parents=True, exist_ok=True)
        self.bd = BuildDir(self.build_path)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_backup_and_restore(self) -> None:
        args_gn = self.build_path / "args.gn"
        args_gn_ref = self.build_path / "args.gn.ref"

        args_gn.write_text('key = "value"\n')

        self.bd.backup_args()
        self.assertTrue(args_gn_ref.exists())
        self.assertEqual(args_gn_ref.read_text(), 'key = "value"\n')

        args_gn.write_text('key = "new_value"\n')

        self.bd.restore_args()
        self.assertFalse(args_gn_ref.exists())
        self.assertEqual(args_gn.read_text(), 'key = "value"\n')

    def test_get_status(self) -> None:
        self.assertEqual(self.bd.get_build_status(), BuildStatus.NOT_CONFIGURED)
        args_gn = self.build_path / "args.gn"
        args_gn.write_text("")
        self.assertEqual(self.bd.get_build_status(), BuildStatus.CONFIGURED)


if __name__ == "__main__":
    unittest.main()
