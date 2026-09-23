#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import sys
import unittest
import unittest.mock
from pathlib import Path

# Ensure script import path
sys.path.append(str(Path(__file__).parent))
import generate_host_test_wrapper


class HostTestWrapperGeneratorTest(unittest.TestCase):
    def test_ld_library_path_formatting_and_isolation(self) -> None:
        # Input variables
        env_vars = [
            ("A", "1"),
            ("LD_LIBRARY_PATH", "user_path"),
            ("B", "2"),
        ]
        so_dirs = {".", "lib/foo"}

        # Call format helper directly to verify actual behavior
        (
            filtered_env,
            export_statement,
        ) = generate_host_test_wrapper.format_ld_library_path_export(
            env_vars, so_dirs
        )

        # Verify LD_LIBRARY_PATH isolation
        self.assertEqual(filtered_env, [("A", "1"), ("B", "2")])

        # Verify unescaped double-quoted export string using built-in ${PWD}
        expected = 'export LD_LIBRARY_PATH="${PWD}:${PWD}/lib/foo:user_path${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"'
        self.assertEqual(export_statement, expected)


if __name__ == "__main__":
    unittest.main()
