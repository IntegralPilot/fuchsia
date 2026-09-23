#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generates licenses SPDX from GN metadata."""

import dataclasses
import hashlib
import json
import os

from file_access import FileAccess
from gn_label import GnLabel
from spdx_types.spdx_types import (
    SpdxDocument,
    SpdxDocumentBuilder,
    SpdxExtractedLicensingInfo,
    SpdxLicenseExpression,
    SpdxPackage,
)


@dataclasses.dataclass(frozen=False)
class SpdxWriter:
    "SPDX json file writer"

    file_access: FileAccess
    builder: SpdxDocumentBuilder

    @staticmethod
    def create(root_package_name: str, file_access: FileAccess) -> "SpdxWriter":
        builder = SpdxDocumentBuilder.create(
            root_package_name=root_package_name,
            creators=[f"Tool: {os.path.basename(__file__)}"],
        )
        writer = SpdxWriter(
            file_access=file_access,
            builder=builder,
        )
        return writer

    def add_package_with_licenses(
        self,
        public_package_name: str,
        license_labels: tuple[GnLabel, ...],
        collection_hints: list[str] | None,
    ) -> None:
        license_ids: list[str] = []
        nested_doc_paths: list[GnLabel] = []

        for license_label in license_labels:
            if license_label.is_spdx_json_document():
                nested_doc_paths.append(license_label)
                continue

            license_text = self.file_access.read_text(license_label)

            license = SpdxExtractedLicensingInfo(
                license_id=SpdxExtractedLicensingInfo.content_based_license_id(
                    public_package_name, license_text
                ),
                name=public_package_name,
                extracted_text=license_text,
                cross_refs=[license_label.url()],
                debug_hint=collection_hints,
            )
            self.builder.add_license(license)

            license_ids.append(license.license_id)

        package = SpdxPackage(
            spdx_id=self.builder.next_package_id(),
            name=public_package_name,
            license_concluded=SpdxLicenseExpression.create(
                " AND ".join(sorted(license_ids))
            )
            if license_ids
            else None,
            debug_hint=collection_hints,
        )
        if not self.builder.has_package(package):
            self.builder.add_package(package)
            self.builder.add_contained_by_root_package_relationship(
                package.spdx_id
            )

        for nested_doc_path in nested_doc_paths:
            nested_doc = SpdxDocument.from_json_dict(
                nested_doc_path.path_str,
                self.file_access.read_json(nested_doc_path),
            )
            self.builder.add_document(parent_package=package, doc=nested_doc)

    def save(self, file_path: str) -> None:
        with open(file_path, "w") as f:
            json.dump(self.builder.build().to_json_dict(), f, indent=4)

    def save_to_string(self) -> str:
        return json.dumps(self.builder.build().to_json_dict(), indent=4)

    def _spdx_package_id(
        self, public_package_name: str, license_labels: tuple[GnLabel, ...]
    ) -> str:
        md5 = hashlib.md5()
        md5.update(public_package_name.strip().encode("utf-8"))
        for ll in license_labels:
            md5.update(str(ll.path_str).encode("utf-8"))
        digest = md5.hexdigest()
        return f"SPDXRef-Package-{digest}"

    def _spdx_license_ref(
        self, public_package_name: str, license_label: GnLabel
    ) -> str:
        md5 = hashlib.md5()
        md5.update(public_package_name.strip().encode("utf-8"))
        md5.update(str(license_label.path_str).encode("utf-8"))
        digest = md5.hexdigest()
        return f"LicenseRef-{digest}"
