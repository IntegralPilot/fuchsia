#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility that produces an SPDX file containing all licenses used in a project"""

import argparse
import json
from pathlib import Path
from sys import stderr

from fuchsia.tools.licenses.common_types import *
from fuchsia.tools.licenses.spdx_types import *

_VERBOSE = False


def _log(*kwargs):
    if _VERBOSE:
        print(*kwargs, file=stderr)


def _create_doc_from_licenses_used_json(
    licenses_used_path: str,
    root_package_name: str,
    root_package_homepage: str,
    document_namespace: str,
    licenses_cross_refs_base_url: str,
) -> SpdxDocument:
    """
    Populates the SPDX docuemnt with bazel license rules information.

    Populates the given SPDX document with information loaded
    from a JSON dict corresponding to a
    @rules_license//rules:gather_licenses_info.bzl JSON output.
    """

    _log(f"Reading {licenses_used_path}!")
    licenses_used_json = json.load(open(licenses_used_path, "r"))
    if isinstance(licenses_used_json, list):
        assert len(licenses_used_json) == 1
        licenses_used_dict = licenses_used_json[0]
    else:
        licenses_used_dict = licenses_used_json
    assert isinstance(licenses_used_dict, dict)
    assert "licenses" in licenses_used_dict
    json_list = licenses_used_dict["licenses"]

    # Sort the licenses list to ensure deterministic output.
    json_list = sorted(
        json_list,
        key=lambda d: (
            d.get("package_name", None),
            d.get("license_text", None),
            d.get("package_url", None),
        ),
    )

    doc_builder: SpdxDocumentBuilder = SpdxDocumentBuilder.create(
        root_package_name=root_package_name,
        root_package_homepage=root_package_homepage,
        creators=[f"Tool: {Path(__file__).name}"],
    )

    for json_dict in json_list:
        dict_reader = DictReader(json_dict, location=licenses_used_path)
        bazel_package_name: str = dict_reader.get("package_name")
        homepage: str | None = dict_reader.get_or("package_url", None)
        copyright_notice: str = dict_reader.get("copyright_notice")
        license_text_file_path: str = dict_reader.get("license_text")

        license_id: str = None
        nested_doc: SpdxDocument = None

        if license_text_file_path.endswith(".spdx.json"):
            _log(f"Adding {license_text_file_path} spdx as a nested document!")
            nested_doc = SpdxDocument.from_json(license_text_file_path)
        else:
            _log(f"Adding {license_text_file_path}!")
            with open(license_text_file_path, "r") as text_file:
                _log(f"Adding license {license_text_file_path}!")
                cross_ref = (
                    licenses_cross_refs_base_url + license_text_file_path
                )
                license_text = text_file.read()
                license = SpdxExtractedLicensingInfo(
                    license_id=SpdxExtractedLicensingInfo.content_based_license_id(
                        bazel_package_name, license_text
                    ),
                    name=bazel_package_name,
                    extracted_text=license_text,
                    cross_refs=[cross_ref],
                )
                license_id = license.license_id
                doc_builder.add_license(license)

        package = SpdxPackage(
            spdx_id=doc_builder.next_package_id(),
            name=bazel_package_name,
            copyright_text=copyright_notice,
            license_concluded=SpdxLicenseExpression.create(license_id)
            if license_id
            else None,
            homepage=homepage,
        )
        doc_builder.add_package(package)
        doc_builder.add_contained_by_root_package_relationship(package.spdx_id)

        if nested_doc:
            doc_builder.add_document(parent_package=package, doc=nested_doc)
            _log(
                f"Merged {license_text_file_path}: packages={len(nested_doc.packages)} licenses={len(nested_doc.extracted_licenses)}"
            )

    return doc_builder.build()


def main():
    """Parses arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--licenses_used",
        help="JSON file containing all licenses to analyze."
        " The output of @rules_license//rules:gather_licenses_info.bzl",
        required=True,
    )
    parser.add_argument(
        "--spdx_output",
        help="Where to write the output spdx file.",
        required=True,
    )
    parser.add_argument(
        "--document_namespace",
        help="A unique namespace url for the SPDX references in the doc.",
        required=True,
    )
    parser.add_argument(
        "--root_package_name",
        help="The name of the root package in the SDPX doc.",
        required=True,
    )
    parser.add_argument(
        "--root_package_homepage",
        help="The homepage of the root package in the SDPX doc.",
        required=True,
    )
    parser.add_argument(
        "--licenses_cross_refs_base_url",
        help="Base URL for license paths that are local files.",
        required=True,
    )
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="Decrease verbosity.",
    )

    args = parser.parse_args()

    if args.quiet:
        global _VERBOSE
        _VERBOSE = False

    _log(f"Got these args {args}!")

    output_path = args.spdx_output

    document = _create_doc_from_licenses_used_json(
        licenses_used_path=args.licenses_used,
        root_package_name=args.root_package_name,
        root_package_homepage=args.root_package_homepage,
        document_namespace=args.document_namespace,
        licenses_cross_refs_base_url=args.licenses_cross_refs_base_url,
    )

    _log(f"Writing {output_path}!")
    document.to_json(output_path)


if __name__ == "__main__":
    main()
