#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import importlib.machinery
import os
import re
import unittest
import uuid


def import_module(name, path):
    return importlib.machinery.SourceFileLoader(name, path).load_module()


def import_acts():
    return importlib.import_module("antlion")


PY_FILE_REGEX = re.compile(".+\.py$")

DENYLIST_DIRECTORIES = []


class ActsImportUnitTest(unittest.TestCase):
    """Test that all acts framework imports work."""

    def test_import_acts_successful(self):
        """Test that importing ACTS works."""
        acts = import_acts()
        self.assertIsNotNone(acts)

    # TODO(b/190659975): Re-enable once permission issue is resolved.
    @unittest.skip("Permission error: b/190659975")
    def test_import_framework_successful(self):
        """Dynamically test all imports from the framework."""
        acts = import_acts()
        if hasattr(acts, "__path__") and len(antlion.__path__) > 0:
            acts_path = antlion.__path__[0]
        else:
            acts_path = os.path.dirname(antlion.__file__)

        for root, _, files in os.walk(acts_path):
            for f in files:
                full_path = os.path.join(root, f)
                if any(full_path.endswith(e) for e in DENYLIST) or any(
                    e in full_path for e in DENYLIST_DIRECTORIES
                ):
                    continue

                path = os.path.relpath(os.path.join(root, f), os.getcwd())

                if PY_FILE_REGEX.match(full_path):
                    with self.subTest(msg=f"import {path}"):
                        fake_module_name = str(uuid.uuid4())
                        module = import_module(fake_module_name, path)
                        self.assertIsNotNone(module)


if __name__ == "__main__":
    unittest.main()
