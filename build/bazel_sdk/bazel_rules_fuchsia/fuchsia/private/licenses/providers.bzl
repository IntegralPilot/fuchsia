# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""License rules providers."""

LicensesCollectionInfo = provider(
    doc = "The output of fuchsia_licenses_provider.",
    fields = {
        "json_file": "File containing the license collection metadata. See _write_licenses_collection_json in fuchsia_licenses_collection.bzl",
        "license_files": "Depset to all license File objects in the collection",
    },
)
