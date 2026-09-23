#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for Collector."""


import dataclasses
import tempfile
import unittest
from pathlib import Path

from collector import Collector, CollectorErrorKind
from file_access import FileAccess
from gn_label import GnLabel
from gn_license_metadata import (
    GnApplicableLicensesMetadata,
    GnLicenseMetadata,
    GnLicenseMetadataDB,
)
from readme_fuchsia import ReadmesDB


class CollectorTest(unittest.TestCase):
    temp_dir: tempfile.TemporaryDirectory[str]
    temp_dir_path: Path
    golibs_vendor_path: Path
    collector: Collector
    metadata_db: GnLicenseMetadataDB

    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.temp_dir_path = Path(self.temp_dir.name)

        self.golibs_vendor_path = (
            self.temp_dir_path / "third_party" / "golibs" / "vendor"
        )
        self.golibs_vendor_path.mkdir(parents=True)

        file_access = FileAccess(self.temp_dir_path)
        self.metadata_db = GnLicenseMetadataDB.from_json_list([])

        self.collector = Collector(
            file_access=file_access,
            metadata_db=self.metadata_db,
            readmes_db=ReadmesDB(file_access),
            include_host_tools=False,
        )

        return super().setUp()

    def tearDown(self) -> None:
        self.temp_dir.cleanup()
        return super().tearDown()

    def _collect_and_assert_licenses(
        self, expected_names_and_licenses: dict[str, list[str]]
    ) -> None:
        self.collector.collect()

        errors = set([e.kind for e in self.collector.errors])
        self.assertSetEqual(errors, set(), msg="No errors are expected")

        # Convert expected into a set of (name, license) tuples
        expected = set()
        for name in expected_names_and_licenses.keys():
            for lic in expected_names_and_licenses[name]:
                expected.add((name, lic))
        # Convert actual into a set of (name, license) tuples
        actual = set()
        for collected_license in self.collector.unique_licenses:
            for _lic in collected_license.license_files:
                actual.add((collected_license.public_name, str(_lic)))
        self.maxDiff = None
        self.assertSetEqual(actual, expected)

    def _collect_and_assert_errors_and_targets(
        self, expected_targets_by_error: dict[CollectorErrorKind, str]
    ) -> None:
        self.collector.collect()

        actual = {}
        for error in self.collector.errors:
            actual[error.kind] = str(error.target_label)
        self.assertDictEqual(
            actual,
            expected_targets_by_error,
            # Adding custom message since assert diff for dicts is hard to read
            msg=f"Actual {actual} and expected {expected_targets_by_error} are different",
        )

    def _collect_and_assert_errors(
        self, expected_error_kinds: list[CollectorErrorKind]
    ) -> None:
        self.collector.collect()

        actual = [e.kind for e in self.collector.errors]
        self.assertSetEqual(set(actual), set(expected_error_kinds))

    def _add_license_metadata(
        self, target: str, name: str, files: list[str]
    ) -> None:
        target_label = GnLabel.from_str(target)
        self.metadata_db.add_license_metadata(
            GnLicenseMetadata(
                target_label=target_label,
                public_package_name=name,
                license_files=tuple(
                    [target_label.create_child_from_str(s) for s in files]
                ),
            )
        )

    def _add_applicable_licenses_metadata(
        self,
        target: str,
        licenses: list[str],
        target_type: str = "action",
        third_party_resources: list[str] | None = None,
    ) -> None:
        target_label = GnLabel.from_str(target)
        if not third_party_resources:
            third_party_resources = []
        self.metadata_db.add_applicable_licenses_metadata(
            application=GnApplicableLicensesMetadata(
                target_label=target_label,
                target_type=target_type,
                license_labels=tuple([GnLabel.from_str(s) for s in licenses]),
                third_party_resources=tuple(
                    [
                        target_label.create_child_from_str(s)
                        for s in third_party_resources
                    ]
                ),
            )
        )

    def _add_files(
        self, file_paths: list[Path | str], base_path: Path | None = None
    ) -> None:
        """Creates fake files in the temp dir"""
        if not base_path:
            base_path = self.temp_dir_path
        else:
            assert base_path.is_relative_to(self.temp_dir_path)
        for path in file_paths:
            path = base_path / path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("")

    ########################### metadata tests:

    def test_target_with_metadata(self) -> None:
        self._add_license_metadata(
            target="//third_party/foo:license",
            name="Foo",
            files=["license.txt"],
        )
        self._add_applicable_licenses_metadata(
            target="//third_party/foo", licenses=["//third_party/foo:license"]
        )

        self._collect_and_assert_licenses(
            {"Foo": ["//third_party/foo/license.txt"]}
        )

    def test_3p_target_with_metadata_but_no_applicable_licenses(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/foo", licenses=[]
        )

        self._collect_and_assert_errors(
            [CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES]
        )

    def test_prebuilt_target_with_metadata_but_no_applicable_licenses(
        self,
    ) -> None:
        self._add_applicable_licenses_metadata(
            target="//prebuilt/foo", licenses=[]
        )

        self._collect_and_assert_errors(
            [CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES]
        )

    def test_target_with_metadata_but_no_such_license_label(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/foo", licenses=["//third_party/foo:license"]
        )

        self._collect_and_assert_errors(
            [CollectorErrorKind.APPLICABLE_LICENSE_REFERENCE_DOES_NOT_EXIST]
        )

    def test_non_third_party_target_without_licenses_does_not_error(
        self,
    ) -> None:
        self._add_applicable_licenses_metadata(target="//foo/bar", licenses=[])

        self._collect_and_assert_licenses({})

    def test_non_third_party_target_with_license(self) -> None:
        self._add_license_metadata(
            target="//foo/bar:license", name="Bar", files=["license.txt"]
        )
        self._add_applicable_licenses_metadata(
            target="//foo/bar", licenses=["//foo/bar:license"]
        )

        self._collect_and_assert_licenses({"Bar": ["//foo/bar/license.txt"]})

    ########################### readme tests:

    def _add_readme_file(
        self, file_path: str, name: str | None, license_files: list[str]
    ) -> None:
        path = self.temp_dir_path / file_path
        content = []
        if name:
            content.append(f"NAME: {name}")
        for l in license_files:
            content.append(f"LICENSE FILE: {l}")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(content))

    def test_target_with_readme(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/foo", licenses=[]
        )
        self._add_files(["third_party/foo/license.txt"])
        self._add_readme_file(
            "third_party/foo/README.fuchsia",
            name="Foo",
            license_files=["license.txt"],
        )
        self._collect_and_assert_licenses(
            {"Foo": ["//third_party/foo/license.txt"]}
        )

    def test_target_with_readme_without_license_specified(self) -> None:
        target = "//third_party/foo"
        self._add_applicable_licenses_metadata(target=target, licenses=[])
        self._add_readme_file(
            "third_party/foo/README.fuchsia", name="Foo", license_files=[]
        )
        self._collect_and_assert_errors(
            [
                CollectorErrorKind.NO_LICENSE_FILE_IN_README,
                CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES,
            ]
        )

    def test_target_with_readme_but_license_file_not_found(self) -> None:
        target = "//third_party/foo"
        self._add_applicable_licenses_metadata(target=target, licenses=[])
        self._add_readme_file(
            "third_party/foo/README.fuchsia",
            name="Foo",
            license_files=["missing_file"],
        )
        self._collect_and_assert_errors(
            [
                CollectorErrorKind.LICENSE_FILE_IN_README_NOT_FOUND,
                CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES,
            ]
        )

    def test_target_with_readme_but_license_name_missing_is_ok(self) -> None:
        target = "//third_party/foo"
        self._add_applicable_licenses_metadata(target=target, licenses=[])
        self._add_files(["third_party/foo/license.txt"])
        self._add_readme_file(
            "third_party/foo/README.fuchsia",
            name=None,
            license_files=["license.txt"],
        )
        self._collect_and_assert_errors(
            [
                CollectorErrorKind.NO_PACKAGE_NAME_IN_README,
                CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES,
            ]
        )

    def test_3p_group_target_with_readme_but_license_name_missing(self) -> None:
        target = "//third_party/foo"
        self._add_applicable_licenses_metadata(
            target=target, licenses=[], target_type="group"
        )
        self._add_files(["third_party/foo/license.txt"])
        self._add_readme_file(
            "third_party/foo/README.fuchsia",
            name=None,
            license_files=["license.txt"],
        )
        self._collect_and_assert_licenses({})

    def test_3p_group_target_with_3p_resource_and_readme_but_license_name_missing(
        self,
    ) -> None:
        target = "//third_party/foo"
        resource = "//third_party/bar/baz"
        self._add_applicable_licenses_metadata(
            target=target,
            licenses=[],
            target_type="group",
            third_party_resources=[resource],
        )
        self._add_files(["third_party/foo/license.txt"])
        self._add_readme_file(
            "third_party/foo/README.fuchsia",
            name=None,
            license_files=["license.txt"],
        )
        self._collect_and_assert_errors_and_targets(
            {
                CollectorErrorKind.NO_PACKAGE_NAME_IN_README: target,
                CollectorErrorKind.THIRD_PARTY_RESOURCE_WITHOUT_LICENSE: resource,
                CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES: target,
            }
        )

    def test_non_3p_group_target_with_3p_resource_and_readme_but_license_name_missing(
        self,
    ) -> None:
        target = "//foo"
        resource = "//third_party/bar/baz"
        self._add_applicable_licenses_metadata(
            target=target,
            licenses=[],
            target_type="group",
            third_party_resources=[resource],
        )
        self._add_files(["third_party/foo/license.txt"])
        self._add_readme_file(
            "third_party/foo/README.fuchsia",
            name=None,
            license_files=["license.txt"],
        )
        self._collect_and_assert_errors_and_targets(
            {
                CollectorErrorKind.THIRD_PARTY_RESOURCE_WITHOUT_LICENSE: resource,
            }
        )

    ########################### resources tests:

    def test_target_with_3p_resource_without_licenses(self) -> None:
        target = "//foo"
        self._add_applicable_licenses_metadata(
            target=target,
            licenses=[],
            third_party_resources=["//third_party/bar/baz"],
        )
        self._add_files(["third_party/bar/license.txt"])
        self._add_readme_file(
            "third_party/bar/README.fuchsia",
            name="bar",
            license_files=["license.txt"],
        )
        self._collect_and_assert_licenses(
            {"bar": ["//third_party/bar/license.txt"]}
        )

    def test_3p_target_with_3p_resource_and_no_license_but_resource_has_license(
        self,
    ) -> None:
        target = "//third_party/foo"
        self._add_applicable_licenses_metadata(
            target=target,
            licenses=[],
            third_party_resources=["//third_party/bar/baz"],
        )
        self._add_files(["third_party/bar/license.txt"])
        self._add_readme_file(
            "third_party/bar/README.fuchsia",
            name="bar",
            license_files=["license.txt"],
        )
        self._collect_and_assert_errors(
            [CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES]
        )

    def test_3p_group_target_with_3p_resource_and_no_license_but_resource_has_license(
        self,
    ) -> None:
        target = "//third_party/foo"
        self._add_applicable_licenses_metadata(
            target=target,
            licenses=[],
            target_type="group",
            third_party_resources=["//third_party/bar/baz"],
        )
        self._add_files(["third_party/bar/license.txt"])
        self._add_readme_file(
            "third_party/bar/README.fuchsia",
            name="bar",
            license_files=["license.txt"],
        )
        self._collect_and_assert_licenses(
            {"bar": ["//third_party/bar/license.txt"]}
        )

    def test_3p_target_with_3p_resource_with_different_licenses(self) -> None:
        target = "//third_party/foo"
        self._add_license_metadata(
            target="//third_party/foo:license",
            name="foo",
            files=["//third_party/foo/license.txt"],
        )
        self._add_applicable_licenses_metadata(
            target=target,
            licenses=["//third_party/foo:license"],
            third_party_resources=["//third_party/bar/baz"],
        )
        self._add_files(["third_party/foo/license.txt"])
        self._add_files(["third_party/bar/license.txt"])
        self._add_readme_file(
            "third_party/bar/README.fuchsia",
            name="bar",
            license_files=["license.txt"],
        )
        self._collect_and_assert_licenses(
            {
                "bar": ["//third_party/bar/license.txt"],
                "foo": ["//third_party/foo/license.txt"],
            }
        )

    def test_3p_group_target_without_resources_or_licenses(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/foo", target_type="group", licenses=[]
        )

        # groupless 3p groups don't need a license: No errors.
        self._collect_and_assert_licenses({})

    ########################### golib tests:

    def _add_golib_vendor_files(self, file_paths: list[Path | str]) -> None:
        self._add_files(file_paths, base_path=self.golibs_vendor_path)

    def test_golib_license_simple(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/golibs:foo/bar", licenses=[]
        )
        self._add_golib_vendor_files(["foo/bar/lib.go", "foo/bar/LICENSE"])
        self._collect_and_assert_licenses(
            {"bar": ["//third_party/golibs/vendor/foo/bar/LICENSE"]}
        )

    def test_golib_license_when_parent_dir_has_the_license(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/golibs:foo/bar", licenses=[]
        )
        self._add_golib_vendor_files(["foo/bar/lib.go", "foo/LICENSE"])
        self._collect_and_assert_licenses(
            {"foo": ["//third_party/golibs/vendor/foo/LICENSE"]}
        )

    def test_golib_license_when_child_dir_has_the_license(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/golibs:foo", licenses=[]
        )
        self._add_golib_vendor_files(["foo/bar/lib.go", "foo/bar/baz/LICENSE"])
        self._collect_and_assert_licenses(
            {"foo": ["//third_party/golibs/vendor/foo/bar/baz/LICENSE"]}
        )

    def test_golib_license_with_different_names_of_license_files(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/golibs:foo", licenses=[]
        )
        self._add_golib_vendor_files(
            [
                "foo/lib.go",  # Not a license
                "foo/license",
                "foo/LICENSE-MIT",
                "foo/LICENSE.txt",
                "foo/COPYRIGHT",
                "foo/NOTICE",
                "foo/COPYING",  # Not a license
            ]
        )
        self._collect_and_assert_licenses(
            {
                "foo": [
                    "//third_party/golibs/vendor/foo/license",
                    "//third_party/golibs/vendor/foo/LICENSE-MIT",
                    "//third_party/golibs/vendor/foo/LICENSE.txt",
                    "//third_party/golibs/vendor/foo/COPYRIGHT",
                    "//third_party/golibs/vendor/foo/NOTICE",
                ]
            }
        )

    def test_golib_without_license(self) -> None:
        target = "//third_party/golibs:foo"
        self._add_applicable_licenses_metadata(target=target, licenses=[])
        self._add_golib_vendor_files(["foo/lib.go"])

        self._collect_and_assert_errors(
            [
                CollectorErrorKind.THIRD_PARTY_GOLIB_WITHOUT_LICENSES,
                CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES,
            ]
        )

    ########################### Default license:

    def test_adds_default_license_for_non_3p_target(self) -> None:
        self._add_files(["default_license.txt"])
        self.collector = dataclasses.replace(
            self.collector,
            default_license_file=GnLabel.from_str("//default_license.txt"),
        )

        self._add_applicable_licenses_metadata(target="//foo", licenses=[])

        self._collect_and_assert_licenses(
            {"Fuchsia": ["//default_license.txt"]}
        )

    def test_does_not_add_default_license_for_3p_target(self) -> None:
        self._add_files(["default_license.txt"])
        self.collector = dataclasses.replace(
            self.collector,
            default_license_file=GnLabel.from_str("//default_license.txt"),
        )

        self._add_applicable_licenses_metadata(
            target="//third_party/foo", licenses=[]
        )

        self._collect_and_assert_errors(
            [CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES]
        )

    ########################### Host targets:

    def _add_host_tool_target_and_licenses(self) -> None:
        self._add_files(["third_party/foo/license.txt"])
        self._add_license_metadata(
            target="//third_party/foo:license",
            name="Foo",
            files=["license.txt"],
        )
        self._add_applicable_licenses_metadata(
            target="//third_party/foo(//host_toolchain)",
            licenses=["//third_party/foo:license"],
        )

    def test_includes_host_tools(self) -> None:
        self._add_host_tool_target_and_licenses()
        self.collector = dataclasses.replace(
            self.collector, include_host_tools=True
        )
        self._collect_and_assert_licenses(
            {"Foo": ["//third_party/foo/license.txt"]}
        )

    def test_excludes_host_tools(self) -> None:
        self._add_host_tool_target_and_licenses()
        self.collector = dataclasses.replace(
            self.collector, include_host_tools=False
        )
        self._collect_and_assert_licenses({})

    #################### Ignored licenses

    def test_ignores_the_no_license_label(self) -> None:
        self._add_applicable_licenses_metadata(
            target="//third_party/foo",
            licenses=["//build/licenses:no_license(//some:toolchain)"],
        )

        self._collect_and_assert_licenses({})


if __name__ == "__main__":
    unittest.main()
