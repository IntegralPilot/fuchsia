# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import tempfile
import unittest
import zipfile

import list_host_python_unittests


class TestListHostPythonUnittests(unittest.TestCase):
    def test_missing_file_returns_empty(self) -> None:
        """Missing file should return empty list without crashing."""
        self.assertEqual(
            list_host_python_unittests.load_tests_from_file(
                "/nonexistent/path/foo.pyz"
            ),
            [],
        )

    def test_corrupt_file_returns_empty(self) -> None:
        """Non-zip or corrupt file should return empty list without raising BadZipFile."""
        with tempfile.NamedTemporaryFile("wb") as f:
            f.write(b"NOT A VALID ZIP FILE")
            f.flush()
            self.assertEqual(
                list_host_python_unittests.load_tests_from_file(f.name),
                [],
            )

    def test_valid_pyz_archive(self) -> None:
        """Valid zip archive containing unittest module should discover tests."""
        with tempfile.TemporaryDirectory() as td:
            pyz_path = os.path.join(td, "dummy_test.pyz")
            with zipfile.ZipFile(pyz_path, "w") as zf:
                zf.writestr(
                    "dummy_test.py",
                    (
                        "import unittest\n\n"
                        "class SampleTest(unittest.TestCase):\n"
                        "    def test_feature(self):\n"
                        "        pass\n"
                    ),
                )
            tests = list_host_python_unittests.load_tests_from_file(pyz_path)
            self.assertIn("SampleTest.test_feature", tests)


if __name__ == "__main__":
    unittest.main()
