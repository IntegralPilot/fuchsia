#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility that produces OSS licenses notice text file."""

import argparse
from sys import stderr

from fuchsia.tools.licenses.classification_types import *
from fuchsia.tools.licenses.spdx_types import *

_VERBOSE = True


def _log(*kwargs):
    if _VERBOSE:
        print(*kwargs, file=stderr)


def main():
    """Parses arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--spdx_input",
        help="An SPDX json file containing all licenses to process."
        " The output of @fuchsia_sdk `fuchsia_licenses_spdx`",
        required=True,
    )
    parser.add_argument(
        "--classifications",
        help="A json file containing the classification output of"
        " the `fuchsia_licenses_classification` rule. When present,"
        " post-classification information such as public source mirrors"
        " are added to the generated notice.",
        required=False,
    )
    parser.add_argument(
        "--output_file",
        help="Where to write the notice text file.",
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

    spdx_input = args.spdx_input

    _log(f"Reading license info from {spdx_input}!")
    spdx_doc = SpdxDocument.from_json(spdx_input)
    spdx_index = SpdxIndex.create(spdx_doc)

    if args.classifications:
        classifications = LicensesClassifications.from_json(
            args.classifications
        )
    else:
        classifications = LicensesClassifications.create_empty()

    output_path = args.output_file
    _log(f"Writing {output_path}...")

    licenses_by_unique_stripped_text = defaultdict(list)
    for license in spdx_doc.extracted_licenses:
        licenses_by_unique_stripped_text[license.extracted_text.strip()].append(
            license
        )

    packages_by_unique_copyright = defaultdict(list)
    for package in spdx_doc.packages:
        if package.copyright_text:
            packages_by_unique_copyright[package.copyright_text].append(package)

    with open(output_path, "w") as notice:

        def write(text):
            notice.write(text)

        def write_delimiter():
            write(
                "================================================================================\n"
            )

        for grouped_licenses in licenses_by_unique_stripped_text.values():
            is_notice_shipped = False
            unique_package_names = set()
            unique_public_source_mirrors = set()

            for license in grouped_licenses:
                for package in spdx_index.get_packages_by_license(license):
                    unique_package_names.add(package.name)

                if license.license_id in classifications.classifications_by_id:
                    classification = classifications.classifications_by_id[
                        license.license_id
                    ]
                    unique_public_source_mirrors.update(
                        classification.all_public_source_mirrors()
                    )

                    is_notice_shipped = (
                        is_notice_shipped
                        or classification.is_notice_shipped is True
                    )
                else:
                    # Absent classifications, fallback to true
                    is_notice_shipped = True

            if not is_notice_shipped:
                continue

            write_delimiter()
            write("The following license text(s) applies to these projects:\n")
            for package_name in sorted(list(unique_package_names)):
                write(f" • {package_name}\n")
            if unique_public_source_mirrors:
                write("\nThe projects' source code is made available at:\n")
                for mirror in sorted(list(unique_public_source_mirrors)):
                    write(f" • {mirror}\n")

            write("\n")
            write(grouped_licenses[0].extracted_text)
            write("\n")

        for unique_text, packages in packages_by_unique_copyright.items():
            write_delimiter()

            write("The following copyright(s) applies to these projects:\n")
            unique_package_names = set([p.name for p in packages])
            for package_name in sorted(list(unique_package_names)):
                write(f" • {package_name}\n")

            write("\n")
            write(unique_text)
            write("\n")


if __name__ == "__main__":
    main()
