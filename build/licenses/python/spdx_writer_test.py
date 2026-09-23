#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for SpdxWriter."""


import json
import unittest
from unittest import mock

from file_access import FileAccess
from gn_label import GnLabel
from spdx_writer import SpdxWriter


class SpdxWriterTest(unittest.TestCase):
    writer: SpdxWriter

    def setUp(self) -> None:
        self.maxDiff = None

        self.mock_file_access = mock.MagicMock(spec=FileAccess)
        self.writer = SpdxWriter.create(
            root_package_name="root pkg",
            file_access=self.mock_file_access,
        )

        return super().setUp()

    def test_empty_doc(self) -> None:
        self.writer.save_to_string()
        self.assertEqual(
            self.writer.save_to_string(),
            """{
    "spdxVersion": "SPDX-2.2",
    "SPDXID": "SPDXRef-DOCUMENT",
    "name": "root pkg",
    "documentNamespace": "",
    "creationInfo": {
        "creators": [
            "Tool: spdx_writer.py"
        ]
    },
    "dataLicense": "CC0-1.0",
    "documentDescribes": [
        "SPDXRef-Package-0"
    ],
    "packages": [
        {
            "SPDXID": "SPDXRef-Package-0",
            "name": "root pkg"
        }
    ],
    "relationships": [],
    "hasExtractedLicensingInfos": []
}""",
        )

    def test_add_package_with_licenses(self) -> None:
        self.mock_file_access.read_text.side_effect = (
            lambda label: f"TEXT FROM {label.path_str}"
        )

        self.writer.add_package_with_licenses(
            public_package_name="Foo Pkg",
            license_labels=(GnLabel.from_str("//foo/license"),),
            collection_hints=["unit test"],
        )
        self.writer.add_package_with_licenses(
            public_package_name="Bar Pkg",
            license_labels=(
                GnLabel.from_str("//bar/license"),
                GnLabel.from_str("//bar/license2"),
            ),
            collection_hints=["unit test"],
        )
        # Add again - should have no effect
        self.writer.add_package_with_licenses(
            public_package_name="Foo Pkg",
            license_labels=(GnLabel.from_str("//foo/license"),),
            collection_hints=["unit test"],
        )

        self.assertEqual(
            self.writer.save_to_string(),
            """{
    "spdxVersion": "SPDX-2.2",
    "SPDXID": "SPDXRef-DOCUMENT",
    "name": "root pkg",
    "documentNamespace": "",
    "creationInfo": {
        "creators": [
            "Tool: spdx_writer.py"
        ]
    },
    "dataLicense": "CC0-1.0",
    "documentDescribes": [
        "SPDXRef-Package-0",
        "SPDXRef-Package-1",
        "SPDXRef-Package-2",
        "SPDXRef-Package-3"
    ],
    "packages": [
        {
            "SPDXID": "SPDXRef-Package-2",
            "name": "Bar Pkg",
            "licenseConcluded": "LicenseRef-0dcba69b66f45540d511c563444945d3 AND LicenseRef-739e71ca1f5d900c23c830d2cf935551",
            "_hint": [
                "unit test"
            ]
        },
        {
            "SPDXID": "SPDXRef-Package-1",
            "name": "Foo Pkg",
            "licenseConcluded": "LicenseRef-4d3799f5b92cbb6482396246f83fac64",
            "_hint": [
                "unit test"
            ]
        },
        {
            "SPDXID": "SPDXRef-Package-3",
            "name": "Foo Pkg",
            "licenseConcluded": "LicenseRef-4d3799f5b92cbb6482396246f83fac64",
            "_hint": [
                "unit test"
            ]
        },
        {
            "SPDXID": "SPDXRef-Package-0",
            "name": "root pkg"
        }
    ],
    "relationships": [
        {
            "spdxElementId": "SPDXRef-Package-0",
            "relatedSpdxElement": "SPDXRef-Package-1",
            "relationshipType": "CONTAINS"
        },
        {
            "spdxElementId": "SPDXRef-Package-0",
            "relatedSpdxElement": "SPDXRef-Package-2",
            "relationshipType": "CONTAINS"
        },
        {
            "spdxElementId": "SPDXRef-Package-0",
            "relatedSpdxElement": "SPDXRef-Package-3",
            "relationshipType": "CONTAINS"
        }
    ],
    "hasExtractedLicensingInfos": [
        {
            "name": "Bar Pkg",
            "licenseId": "LicenseRef-0dcba69b66f45540d511c563444945d3",
            "extractedText": "TEXT FROM bar/license2",
            "crossRefs": [
                {
                    "url": "https://cs.opensource.google/fuchsia/fuchsia/+/main:bar/license2"
                }
            ],
            "_hint": [
                "unit test"
            ]
        },
        {
            "name": "Bar Pkg",
            "licenseId": "LicenseRef-739e71ca1f5d900c23c830d2cf935551",
            "extractedText": "TEXT FROM bar/license",
            "crossRefs": [
                {
                    "url": "https://cs.opensource.google/fuchsia/fuchsia/+/main:bar/license"
                }
            ],
            "_hint": [
                "unit test"
            ]
        },
        {
            "name": "Foo Pkg",
            "licenseId": "LicenseRef-4d3799f5b92cbb6482396246f83fac64",
            "extractedText": "TEXT FROM foo/license",
            "crossRefs": [
                {
                    "url": "https://cs.opensource.google/fuchsia/fuchsia/+/main:foo/license"
                }
            ],
            "_hint": [
                "unit test"
            ]
        }
    ]
}""",
        )

    def test_nested_spdx_document(self) -> None:
        nested_doc_spdx_json = json.loads(
            """{
    "spdxVersion": "SPDX-2.2",
    "SPDXID": "SPDXRef-DOCUMENT",
    "name": "some project",
    "documentNamespace": "",
    "creationInfo": {
        "creators": [ "unknown" ]
    },
    "documentDescribes": [
        "SPDXRef-Package-A",
        "SPDXRef-Package-B"
    ],
    "packages": [
        {
            "SPDXID": "SPDXRef-Package-A",
            "name": "Pkg A",
            "licenseConcluded": "LicenseRef-1"
        },
        {
            "SPDXID": "SPDXRef-Package-B",
            "name": "Pkg B",
            "licenseConcluded": "LicenseRef-2"
        }
    ],
    "relationships": [
        {
            "spdxElementId": "SPDXRef-Package-A",
            "relatedSpdxElement": "SPDXRef-Package-B",
            "relationshipType": "CONTAINS"
        }
    ],
    "hasExtractedLicensingInfos": [
        {
            "name": "License A",
            "licenseId": "LicenseRef-1",
            "extractedText": "License text 1"
        },
        {
            "name": "License B",
            "licenseId": "LicenseRef-2",
            "extractedText": "License text 2"
        }
    ]
}"""
        )
        self.mock_file_access.read_json.return_value = nested_doc_spdx_json

        self.writer.add_package_with_licenses(
            public_package_name="Sub Project",
            license_labels=(
                GnLabel.from_str("//some/path/to/license.spdx.json"),
            ),
            collection_hints=["unit test"],
        )

        self.assertEqual(
            self.writer.save_to_string(),
            """{
    "spdxVersion": "SPDX-2.2",
    "SPDXID": "SPDXRef-DOCUMENT",
    "name": "root pkg",
    "documentNamespace": "",
    "creationInfo": {
        "creators": [
            "Tool: spdx_writer.py"
        ]
    },
    "dataLicense": "CC0-1.0",
    "documentDescribes": [
        "SPDXRef-Package-0",
        "SPDXRef-Package-1",
        "SPDXRef-Package-2",
        "SPDXRef-Package-3"
    ],
    "packages": [
        {
            "SPDXID": "SPDXRef-Package-2",
            "name": "Pkg A",
            "licenseConcluded": "LicenseRef-767eeeb3caf0024a6910b45885880f56"
        },
        {
            "SPDXID": "SPDXRef-Package-3",
            "name": "Pkg B",
            "licenseConcluded": "LicenseRef-a76019994a4981e5870988c499093775"
        },
        {
            "SPDXID": "SPDXRef-Package-1",
            "name": "Sub Project",
            "_hint": [
                "unit test"
            ]
        },
        {
            "SPDXID": "SPDXRef-Package-0",
            "name": "root pkg"
        }
    ],
    "relationships": [
        {
            "spdxElementId": "SPDXRef-Package-0",
            "relatedSpdxElement": "SPDXRef-Package-1",
            "relationshipType": "CONTAINS"
        },
        {
            "spdxElementId": "SPDXRef-Package-1",
            "relatedSpdxElement": "SPDXRef-Package-2",
            "relationshipType": "CONTAINS"
        },
        {
            "spdxElementId": "SPDXRef-Package-2",
            "relatedSpdxElement": "SPDXRef-Package-3",
            "relationshipType": "CONTAINS"
        }
    ],
    "hasExtractedLicensingInfos": [
        {
            "name": "License A",
            "licenseId": "LicenseRef-767eeeb3caf0024a6910b45885880f56",
            "extractedText": "License text 1"
        },
        {
            "name": "License B",
            "licenseId": "LicenseRef-a76019994a4981e5870988c499093775",
            "extractedText": "License text 2"
        }
    ]
}""",
        )


if __name__ == "__main__":
    unittest.main()
