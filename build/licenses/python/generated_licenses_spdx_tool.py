#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generates licenses SPDX from GN metadata."""

import argparse
import logging
import os
import sys
from pathlib import Path

from collector import Collector
from file_access import FileAccess
from gn_label import GnLabel
from gn_license_metadata import GnLicenseMetadataDB
from readme_fuchsia import ReadmesDB
from spdx_comparator import SpdxComparator
from spdx_writer import SpdxWriter


def main() -> int:
    """
    Generates licenses SPDX json file from GN license metadata.
    """
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--generated-license-metadata",
        type=str,
        required=True,
        help="Path to the license GN metadata file, generated via the GN genereated_file action",
    )
    parser.add_argument(
        "--fuchsia-source-path",
        type=str,
        required=True,
        help="The path to the root of the Fuchsia source tree",
    )
    parser.add_argument(
        "--spdx-root-package-name",
        type=str,
        required=True,
        help="The name of the SPDX root package",
    )
    parser.add_argument(
        "--spdx-output",
        type=str,
        required=True,
        help="Path to the output SPDX file",
    )
    parser.add_argument(
        "--dep-file",
        type=str,
        required=True,
        help="Path to the generated dep file",
    )
    parser.add_argument(
        "--ignore-collection-errors",
        action="store_true",
        default=False,
        help="Tool will fail when encountering license collection errors",
    )
    parser.add_argument(
        "--include-host-tools",
        action="store_true",
        default=False,
        help="Whether to include licenses of targets that are host tools",
    )
    parser.add_argument(
        "--debug-hints",
        action="store_true",
        default=False,
        help="Embeds '_hint' keys in the output SPDX for debugging purposes",
    )
    # TODO(132725): Remove once migration completes.
    parser.add_argument(
        "--compare-with-legacy-spdx",
        type=str,
        default=None,
        help="The tool will compare the contents of the actual spdx output with the given legacy file",
    )
    parser.add_argument(
        "--ignore-comparison-errors",
        action="store_true",
        default=False,
        help="Tool will fail when comparison with the legacy spdx finds missing licenses",
    )

    parser.add_argument(
        "--log-level",
        type=str,
        default="WARNING",
        help="Python logging level",
    )

    args = parser.parse_args()

    log_level = args.log_level.upper()

    logging.basicConfig(
        level=log_level, force=True, format="%(levelname)s:%(message)s"
    )

    fuchsia_source_path = Path(args.fuchsia_source_path).expanduser()
    assert fuchsia_source_path.exists()
    logging.debug("fuchsia_source_path=%s", fuchsia_source_path)

    fuchsia_source_path_str = str(fuchsia_source_path)
    file_access = FileAccess(fuchsia_source_path_str=fuchsia_source_path_str)

    readmes_db = ReadmesDB(file_access=file_access)

    metadata_db = GnLicenseMetadataDB.from_file(
        file_path=args.generated_license_metadata,
        fuchsia_source_path=fuchsia_source_path_str,
    )

    # Collect licenses information
    collector = Collector(
        file_access=file_access,
        metadata_db=metadata_db,
        readmes_db=readmes_db,
        include_host_tools=args.include_host_tools,
        default_license_file=GnLabel.from_str("//LICENSE"),
        generate_debug_hints=bool(args.debug_hints),
    )

    collector.collect()

    if collector.errors:
        if args.ignore_collection_errors:
            collector.log_errors(log_level=logging.WARN, is_full_report=False)
            logging.warning(
                "Errors are ignored because --ignore_collection_errors is True. "
            )
        else:
            collector.log_errors(log_level=logging.ERROR, is_full_report=True)
            return -1

    logging.info(f"Collection stats: {collector.stats}")

    # Generate an SPDX file:
    spdx_writer = SpdxWriter.create(
        root_package_name=args.spdx_root_package_name,
        file_access=file_access,
    )

    sorted_licenses = sorted(collector.unique_licenses)
    for collected_license in sorted_licenses:
        spdx_writer.add_package_with_licenses(
            public_package_name=collected_license.public_name,
            license_labels=collected_license.license_files,
            collection_hints=sorted(
                collector.license_debug_hints.get(collected_license, [])
            ),
        )

    spdx_writer.save(args.spdx_output)
    logging.info(
        f"Wrote spdx {args.spdx_output} (licenses={len(collector.unique_licenses)} size={os.stat(args.spdx_output).st_size})"
    )

    if args.compare_with_legacy_spdx:
        # Compare with legacy spdx file
        comparator = SpdxComparator(
            current_file=args.spdx_output,
            legacy_file=Path(args.compare_with_legacy_spdx),
        )
        comparator.compare()
        if comparator.found_differences():
            if args.ignore_comparison_errors:
                comparator.log_differences(logging.WARN, is_full_report=False)
                logging.warning(
                    "Differences are ignored because --ignore_comparison_errors is True. "
                )
            else:
                comparator.log_differences(logging.ERROR, is_full_report=True)
                return -1

    # Generate a GN depfile
    logging.info(f"writing depfile {args.dep_file}")
    file_access.write_depfile(args.dep_file, main_entry=args.spdx_output)

    return 0


if __name__ == "__main__":
    sys.exit(main())
