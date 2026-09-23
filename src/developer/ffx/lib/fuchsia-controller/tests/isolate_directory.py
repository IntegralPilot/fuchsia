# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import os
import shutil
import tempfile
import unittest

from fuchsia_controller_py import IsolateDir


class IsolateDirectory(unittest.TestCase):
    def test_isolate_dir_creation_empty(self) -> None:
        isolate_dir: IsolateDir | None = IsolateDir()
        assert isolate_dir is not None
        temp_dir = isolate_dir.directory()

        self.assertTrue(os.path.exists(temp_dir))

        isolate_dir = None
        self.assertFalse(os.path.exists(temp_dir))

    def test_isolate_dir_creation_new_dir(self) -> None:
        temp_dir = tempfile.mkdtemp()  # Get a random directory path
        shutil.rmtree(temp_dir)  # Guarantee directory doesn't exist yet
        self.assertFalse(os.path.exists(temp_dir))

        isolate_dir: IsolateDir | None = IsolateDir(dir=temp_dir)
        assert isolate_dir is not None
        self.assertEqual(isolate_dir.directory(), temp_dir)
        self.assertTrue(os.path.exists(temp_dir))

        isolate_dir = None
        self.assertFalse(os.path.exists(temp_dir))

    def test_isolate_dir_creation_existing_dir(self) -> None:
        temp_dir = tempfile.mkdtemp()
        self.assertTrue(os.path.exists(temp_dir))

        isolate_dir: IsolateDir | None = IsolateDir(dir=temp_dir)
        assert isolate_dir is not None
        self.assertEqual(isolate_dir.directory(), temp_dir)
        self.assertTrue(os.path.exists(temp_dir))

        isolate_dir = None
        self.assertFalse(os.path.exists(temp_dir))
