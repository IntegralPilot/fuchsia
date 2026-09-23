#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for GnLicenseMetadataDB and friends."""

import unittest

from gn_label import GnLabel
from gn_license_metadata import GnLicenseMetadata, GnLicenseMetadataDB


class GnLicenseMetadataDBTest(unittest.TestCase):
    def test_load_from_list(self) -> None:
        fuchsia_source_path = "/absolute/path/to/fuchsia/dir"

        input = [
            {  # l1
                "target_label": "//foo:license(//toolchain)",
                "public_package_name": "l1",
                "license_files": [
                    "//license1",
                    "license2",
                    "bar/license3",
                    fuchsia_source_path + "/license4",
                ],
            },
            {
                # l2
                "target_label": "//bar:lic(//toolchain)",
                "public_package_name": "l2",
                "license_files": ["license3"],  # Relative
            },
            {
                # al1
                "target_label": "//foo:target(//toolchain)",
                "license_labels": ["//foo:license(//toolchain)"],
            },
            {
                # al2
                "target_label": "//bar:bar(//toolchain)",
                "license_labels": ["//bar:lic(//toolchain)"],
                "third_party_resources": [
                    "r1.txt",
                    "r2/r2.txt",
                    "//r3.txt",
                    fuchsia_source_path + "/r4.txt",
                ],
            },
        ]

        db = GnLicenseMetadataDB.from_json_list(
            input, fuchsia_source_path=fuchsia_source_path
        )

        l1 = db.licenses_by_label[
            GnLabel.from_str("//foo:license(//toolchain)")
        ]
        self.assertEqual(
            l1,
            GnLicenseMetadata(
                target_label=GnLabel.from_str("//foo:license(//toolchain)"),
                public_package_name="l1",
                license_files=(
                    GnLabel.from_str("//license1"),
                    GnLabel.from_str("//foo/license2"),
                    GnLabel.from_str("//foo/bar/license3"),
                    GnLabel.from_str("//license4"),
                ),
            ),
        )

        l2 = db.licenses_by_label[GnLabel.from_str("//bar:lic(//toolchain)")]
        self.assertEqual(
            l2,
            GnLicenseMetadata(
                target_label=GnLabel.from_str("//bar:lic(//toolchain)"),
                public_package_name="l2",
                license_files=(GnLabel.from_str("//bar/license3"),),
            ),
        )

        al1 = db.applicable_licenses_by_target[
            GnLabel.from_str("//foo:target(//toolchain)")
        ]
        self.assertEqual(
            al1.target_label, GnLabel.from_str("//foo:target(//toolchain)")
        )
        self.assertEqual(
            al1.license_labels,
            (GnLabel.from_str("//foo:license(//toolchain)"),),
        )

        al2 = db.applicable_licenses_by_target[
            GnLabel.from_str("//bar:bar(//toolchain)")
        ]
        self.assertEqual(
            al2.target_label, GnLabel.from_str("//bar:bar(//toolchain)")
        )
        self.assertEqual(
            al2.license_labels, (GnLabel.from_str("//bar:lic(//toolchain)"),)
        )
        self.assertEqual(
            al2.third_party_resources,
            tuple(
                [
                    GnLabel.from_str("//bar/r1.txt"),
                    GnLabel.from_str("//bar/r2/r2.txt"),
                    GnLabel.from_str("//r3.txt"),
                    GnLabel.from_str("//r4.txt"),
                ]
            ),
        )


if __name__ == "__main__":
    unittest.main()
