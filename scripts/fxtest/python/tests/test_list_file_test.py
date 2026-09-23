# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import tempfile
import unittest

from test_list_file import Test
from test_list_file import TestListEntry
from test_list_file import TestListFile
from test_list_file import TestListTagKV
from tests_json_file import DimensionsEntry
from tests_json_file import EnvironmentEntry
from tests_json_file import TestEntry
from tests_json_file import TestSection


class TestListFileParsingTest(unittest.TestCase):
    """Test processing test-list.json"""

    def test_from_file(self) -> None:
        """Test basic loading of a test-list.json file."""
        contents = TestListFile(
            data=[
                TestListEntry("my_test", tags=[]),
                TestListEntry("my_test2", tags=[]),
            ]
        ).to_dict()  # type:ignore

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "test-list.json")
            with open(path, "w") as f:
                json.dump(contents, f)

            entries = TestListFile.entries_from_file(path)
            self.assertSetEqual(
                set(entries.keys()), set(["my_test", "my_test2"])
            )


class TestListFileJoiningTest(unittest.TestCase):
    """Test joining tests.json with test-list.json"""

    def test_joining_files(self) -> None:
        """Test joining the contents of tests.json and test-list.json into Test objects."""

        test_list = [
            Test(
                TestEntry(
                    test=TestSection(
                        name="my_test", label="//src/my_test", os="linux"
                    )
                )
            ),
            Test(
                TestEntry(
                    test=TestSection(
                        name="my_test2", label="//src/my_test2", os="linux"
                    )
                )
            ),
        ]
        test_list_file = {
            "my_test": TestListEntry("my_test", tags=[]),
            "my_test2": TestListEntry("my_test2", tags=[]),
            "extra_test": TestListEntry("extra_test", tags=[]),
        }

        Test.augment_tests_with_info(test_list, test_list_file)

        self.assertSetEqual(
            set([t.name() for t in test_list]), set(["my_test", "my_test2"])
        )

        # Names are consistent between build and info contents.
        for test in test_list:
            self.assertEqual(test.build.test.name, test.name())

        # Test implements equals.
        self.assertNotEqual(test_list[0], test_list[1])
        self.assertEqual(test_list[0], test_list[0])
        self.assertEqual(test_list[1], test_list[1])

        # Test implements hash.
        set(test_list)

    def test_missing_from_test_list(self) -> None:
        """It is an error for tests.json to contain a test test-list.json omits."""

        test_list = [
            Test(
                TestEntry(
                    test=TestSection(
                        name="my_test", label="//src/my_test", os="linux"
                    )
                )
            ),
        ]
        test_list_file: dict[str, TestListEntry] = {}

        with self.assertRaises(ValueError) as ctx:
            Test.augment_tests_with_info(test_list, test_list_file)
        self.assertIn(
            "Test 'my_test' was found in tests.json", str(ctx.exception)
        )

    def test_equality_distinct_instances(self) -> None:
        """Two distinct Test instances wrapping equal TestEntries must compare equal."""
        t1 = Test(
            TestEntry(
                test=TestSection(
                    name="my_test", label="//src/my_test", os="linux"
                )
            )
        )
        t2 = Test(
            TestEntry(
                test=TestSection(
                    name="my_test", label="//src/my_test", os="linux"
                )
            )
        )
        self.assertEqual(t1, t2)
        self.assertFalse(t1 == "not_a_test")


class TestListEntryMethodTest(unittest.TestCase):
    """Test methods on TestListEntry"""

    def test_hermetic(self) -> None:
        hermetic = TestListEntry(
            "foo", tags=[TestListTagKV(key="hermetic", value="true")]
        )
        not_hermetic1 = TestListEntry(
            "foo", tags=[TestListTagKV(key="hermetic", value="false")]
        )
        not_hermetic2 = TestListEntry(
            "foo", tags=[TestListTagKV(key="hermetic", value="")]
        )
        not_hermetic3 = TestListEntry("foo", tags=[])

        self.assertTrue(hermetic.is_hermetic())
        self.assertFalse(not_hermetic1.is_hermetic())
        self.assertFalse(not_hermetic2.is_hermetic())
        self.assertFalse(not_hermetic3.is_hermetic())


class TestMethodTest(unittest.TestCase):
    """Test methods on Test"""

    def test_is_e2e_test(self) -> None:
        e2e_test = Test(
            TestEntry(
                test=TestSection(
                    name="e2e_test", label="//src/e2e_test", os="linux"
                ),
                environments=[
                    EnvironmentEntry(
                        dimensions=DimensionsEntry(device_type="AEMU")
                    )
                ],
            )
        )
        boot_test = Test(
            TestEntry(
                test=TestSection(
                    name="boot_test", label="//src/boot_test", os="linux"
                ),
                environments=[
                    EnvironmentEntry(
                        dimensions=DimensionsEntry(device_type="AEMU")
                    )
                ],
                is_boot_test=True,
            )
        )

        self.assertTrue(e2e_test.is_e2e_test())
        self.assertTrue(boot_test.is_boot_test())
        self.assertFalse(boot_test.is_e2e_test())
