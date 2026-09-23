# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import tempfile
import unittest

from tests_json_file import TestEntry
from tests_json_file import TestFileError
from tests_json_file import TestSection


class TestFileTest(unittest.TestCase):
    """Test processing tests.json"""

    def test_from_file(self) -> None:
        """Test basic loading of a tests.json file."""
        contents = [
            TestEntry(
                test=TestSection(
                    "my_test",
                    "//src:my_test",
                    "linux",
                )
            ).to_dict(),  # type:ignore
            TestEntry(
                test=TestSection(
                    "my_test2",
                    "//src:my_test2",
                    "linux",
                )
            ).to_dict(),  # type:ignore
        ]

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "tests.json")
            with open(path, "w") as f:
                json.dump(contents, f)

            entries: list[TestEntry] = TestEntry.from_file(path)
            self.assertListEqual(
                [t.test.name for t in entries], ["my_test", "my_test2"]
            )

    def test_duplicate_name_error(self) -> None:
        """Ensure that loading a tests.json file with duplicate names raises an exception."""
        contents = [
            TestEntry(
                test=TestSection(
                    "my_test",
                    "//src:my_test",
                    "linux",
                )
            ).to_dict(),  # type:ignore
            TestEntry(
                test=TestSection(
                    "my_test",
                    "//src:my_test2",
                    "linux",
                )
            ).to_dict(),  # type:ignore
        ]

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "tests.json")
            with open(path, "w") as f:
                json.dump(contents, f)

            self.assertRaises(TestFileError, lambda: TestEntry.from_file(path))

    def test_duplicate_name_path_ok(self) -> None:
        """Ensure that loading a tests.json file with duplicate names but without duplicate paths is OK."""
        contents = [
            TestEntry(
                test=TestSection(
                    "my_test",
                    "//src:my_test",
                    "linux",
                    path="foo.sh",
                )
            ).to_dict(),  # type:ignore
            TestEntry(
                test=TestSection(
                    "my_test",
                    "//src:my_test2",
                    "linux",
                    path="bar.sh",
                )
            ).to_dict(),  # type:ignore
        ]

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "tests.json")
            with open(path, "w") as f:
                json.dump(contents, f)

            self.assertEqual(len(TestEntry.from_file(path)), 2)

    def test_invalid_format(self) -> None:
        """Ensure an exception is raised if the top-level JSON field is not a list."""
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "tests.json")
            with open(path, "w") as f:
                f.write("{}")

            self.assertRaises(TestFileError, lambda: TestEntry.from_file(path))

    def test_invalid_json(self) -> None:
        """Ensure an exception is raised if the file does not contain valid JSON."""
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "tests.json")
            with open(path, "w") as f:
                f.write("3 {}")

            self.assertRaises(
                json.JSONDecodeError, lambda: TestEntry.from_file(path)
            )

    def test_missing_file(self) -> None:
        """Ensure an exception is raised if the file does not exist."""
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "tests.json")
            self.assertRaises(IOError, lambda: TestEntry.from_file(path))
