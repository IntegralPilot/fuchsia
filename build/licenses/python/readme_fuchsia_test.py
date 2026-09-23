#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for Readme and ReadmesDB."""


import dataclasses
import unittest

from file_access import FileAccess
from gn_label import GnLabel
from readme_fuchsia import Readme, ReadmesDB


class ReadmeTest(unittest.TestCase):
    def setUp(self) -> None:
        return super().setUp()

    def test_readme_from_text(self) -> None:
        readme = Readme.from_text(
            readme_label=GnLabel.from_str("//some/path/README.fuchsia"),
            applicable_target=GnLabel.from_str("//some/other/path:to_target"),
            file_text="""
X: 123
Y: abc
Name: Some Name
License File: license1
License File: child/license2
License File: ../sibling/license3

Some text
""",
        )

        self.assertEqual(readme.package_name, "Some Name")
        self.assertEqual(
            readme.license_files,
            (
                GnLabel.from_str("//some/other/path/license1"),
                GnLabel.from_str("//some/other/path/child/license2"),
                GnLabel.from_str("//some/other/sibling/license3"),
            ),
        )

    def test_readme_from_text_missing_fields(self) -> None:
        readme = Readme.from_text(
            readme_label=GnLabel.from_str("//some/path/README.fuchsia"),
            applicable_target=GnLabel.from_str("//some/other/path:to_target"),
            file_text="""
X: 123
Y: abc

Some text
""",
        )

        self.assertIsNone(readme.package_name)
        self.assertEqual(readme.license_files, tuple())


@dataclasses.dataclass
class MockFileAccess(FileAccess):
    readme_content_by_path: dict[str, str] | None = None

    def read_text(self, label: GnLabel) -> str:
        if self.readme_content_by_path is None:
            return ""
        return self.readme_content_by_path[label.path_str]

    def file_exists(self, label: GnLabel) -> bool:
        return (
            self.readme_content_by_path is not None
            and label.path_str in self.readme_content_by_path
        )


class ReadmesDBTest(unittest.TestCase):
    db: ReadmesDB
    mock_content_by_path: dict[str, str] | None

    def setUp(self) -> None:
        self.mock_content_by_path = {}

        file_access = MockFileAccess(
            fuchsia_source_path_str="",
            readme_content_by_path=self.mock_content_by_path,
        )
        self.db = ReadmesDB(file_access=file_access)

        return super().setUp()

    def _mock_readme_files(self, file_paths: list[str]) -> None:
        for path in file_paths:
            if self.mock_content_by_path is None:
                self.mock_content_by_path = {}
            self.mock_content_by_path[path] = "Name: Foo"

    def _assert_found_readme(
        self, for_label: str, expected_readme_path: str
    ) -> None:
        readme = self.db.find_readme_for_label(GnLabel.from_str(for_label))
        self.assertIsNotNone(readme)
        if readme:
            self.assertEqual(readme.readme_label.path_str, expected_readme_path)

    def _assert_readme_not_found(self, for_label: str) -> None:
        readme = self.db.find_readme_for_label(GnLabel.from_str(for_label))
        self.assertIsNone(readme)

    def test_find_readme_in_same_folder(self) -> None:
        self._mock_readme_files(["foo/README.fuchsia"])
        self._assert_found_readme("//foo", "foo/README.fuchsia")
        self._assert_found_readme("//foo:foo", "foo/README.fuchsia")
        self._assert_found_readme("//foo:bar", "foo/README.fuchsia")

    def test_find_readme_for_sub_folder(self) -> None:
        self._mock_readme_files(["foo/README.fuchsia"])
        self._assert_found_readme("//foo/bar", "foo/README.fuchsia")
        self._assert_found_readme("//foo/bar/baz", "foo/README.fuchsia")

    def test_barrier_folders(self) -> None:
        self._mock_readme_files(["foo/README.fuchsia"])

        self._assert_found_readme("//foo", "foo/README.fuchsia")

        self._assert_readme_not_found("//foo/third_party:bar")
        self._assert_readme_not_found("//foo/third_party/bar")
        self._assert_readme_not_found("//foo/third_party/bar:baz")
        self._assert_readme_not_found("//foo/third_party/bar/baz")
        self._assert_readme_not_found("//foo/thirdparty/bar")
        self._assert_readme_not_found("//foo/prebuilt/bar")
        self._assert_readme_not_found("//foo/prebuilts/bar")

    def test_find_closest_readme(self) -> None:
        self._mock_readme_files(
            [
                "foo/README.fuchsia",
                "foo/bar/README.fuchsia",
                "foo/bar/baz/README.fuchsia",
            ]
        )
        self._assert_found_readme("//foo", "foo/README.fuchsia")
        self._assert_found_readme("//foo/qaz", "foo/README.fuchsia")
        self._assert_found_readme("//foo/bar", "foo/bar/README.fuchsia")
        self._assert_found_readme("//foo/bar/qaz", "foo/bar/README.fuchsia")
        self._assert_found_readme("//foo/bar/baz", "foo/bar/baz/README.fuchsia")
        self._assert_found_readme(
            "//foo/bar/baz/qaz", "foo/bar/baz/README.fuchsia"
        )

    def test_find_in_assets_folders(self) -> None:
        self._mock_readme_files(
            [
                "foo/README.fuchsia",
                "tools/check-licenses/assets/readmes/foo/README.fuchsia",
                "vendor/google/tools/check-licenses/assets/readmes/foo/README.fuchsia",
                "bar/README.fuchsia",
                "tools/check-licenses/assets/readmes/bar/README.fuchsia",
            ]
        )

        # vendor/google/tools/check-licenses/assets takes precedence
        self._assert_found_readme(
            "//foo",
            "vendor/google/tools/check-licenses/assets/readmes/foo/README.fuchsia",
        )

        # otherwise tools/check-licenses/assets takes precedence
        self._assert_found_readme(
            "//bar", "tools/check-licenses/assets/readmes/bar/README.fuchsia"
        )


if __name__ == "__main__":
    unittest.main()
