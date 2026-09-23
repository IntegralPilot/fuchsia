#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility that produces OSS licenses compliance materials."""

import argparse
import csv
import os
import shutil
import zipfile
from sys import stderr

from fuchsia.tools.licenses.classification_types import *
from fuchsia.tools.licenses.spdx_types import *

_VERBOSE = True


def _log(*kwargs):
    if _VERBOSE:
        print(*kwargs, file=stderr)


def _dedup(input: list[str]) -> list[str]:
    return sorted(list(set(input)))


def _dependents_string(
    license: SpdxExtractedLicensingInfo, spdx_index: SpdxIndex
) -> str:
    dependents = [
        ">".join([p.name for p in path])
        for path in spdx_index.dependency_chains_for_license(license)
    ]
    return "\n".join(sorted(dependents))


def _packages_homepages(
    license: SpdxExtractedLicensingInfo, spdx_index: SpdxIndex
) -> str:
    return [
        p.homepage
        for p in spdx_index.get_packages_by_license(license)
        if p.homepage
    ]


def _link_string(
    license: SpdxExtractedLicensingInfo, spdx_index: SpdxIndex
) -> str:
    links = license.unique_links()
    links.extend(_packages_homepages(license, spdx_index))
    return "\n".join(_dedup(links))


def _write_summary_csv(
    spdx_doc: SpdxDocument,
    spdx_index: SpdxIndex,
    classifications: LicensesClassifications,
    output_path: str,
):
    """The summary CSV has a row for original license text, aggregates all the identifications and conditions in the text"""
    with open(output_path, "w") as csvfile:
        writer = csv.DictWriter(
            csvfile,
            fieldnames=[
                "name",
                "identifications",
                "conditions",
                "identifications_and_conditions",
                "overriden_conditions",
                "link",
                "tracking_issues",
                "email_subject_lines",
                "comments",
                "public_source_mirrors",
                "is_project_shipped",
                "is_notice_shipped",
                "is_source_code_shipped",
                # The following 'debugging' fields begin with _ so can be easily
                # filtered/hidden/sorted once in a spreadsheet.
                "_spdx_license_id",
                "_dependents",
                "_detailed_identifications",
                "_detailed_overrides",
                "_size_bytes",
                "_size_lines",
                "_unidentified_lines",
            ],
        )
        writer.writeheader()

        for license in spdx_doc.extracted_licenses:
            license_id = license.license_id
            identifications = []
            conditions = []
            identifications_and_conditions = []
            detailed_identifications = []
            detailed_overrides = []
            tracking_issues = []
            email_subject_lines = []
            comments = []
            public_source_mirrors = []

            shipped_info = {}
            identification_stats = {}
            if license_id in classifications.classifications_by_id:
                license_classification = classifications.classifications_by_id[
                    license_id
                ]
                final_conditions = []

                identification_stats.update(
                    {
                        "_size_bytes": license_classification.size_bytes,
                        "_size_lines": license_classification.size_lines,
                        "_unidentified_lines": license_classification.unidentified_lines,
                    }
                )

                shipped_info.update(
                    {
                        "is_project_shipped": license_classification.is_project_shipped,
                        "is_notice_shipped": license_classification.is_notice_shipped,
                        "is_source_code_shipped": license_classification.is_source_code_shipped,
                    }
                )

                for i in license_classification.identifications:
                    identifications.append(i.identified_as)
                    conditions.extend(i.conditions)

                    conditions_str = ",".join(list(i.conditions))
                    identifications_and_conditions.append(
                        f"{i.identified_as} ({conditions_str})"
                    )

                    detailed_identifications.append(
                        f"{i.identified_as} at lines {i.start_line}-{i.end_line}: {conditions_str}"
                    )
                    if i.verified_conditions:
                        final_conditions.extend(i.verified_conditions)
                    if i.overriding_rules:
                        for r in i.overriding_rules:
                            detailed_overrides.append(
                                f"{i.identified_as} ({conditions_str}) at {i.start_line}-{i.end_line} overriden to ({r.override_condition_to}) by {r.rule_file_path}"
                            )
                            tracking_issues.append(r.bug)
                            if r.email_subject_line:
                                email_subject_lines.append(r.email_subject_line)
                            comment = "{matched_identifications} ({matched_conditions}) -> ({overriden_condition})\n{comment_text}".format(
                                matched_identifications=",".join(
                                    r.match_identifications.all_expressions
                                ),
                                matched_conditions=",".join(
                                    r.match_conditions.all_expressions
                                ),
                                overriden_condition=r.override_condition_to,
                                comment_text="\n".join(r.comment),
                            )
                            comments.append(comment)

                    if i.public_source_mirrors:
                        public_source_mirrors.extend(i.public_source_mirrors)

            row = {
                # License review columns
                "name": license.name,
                "link": _link_string(license, spdx_index),
                "identifications": ",\n".join(_dedup(identifications)),
                "conditions": ",\n".join(_dedup(conditions)),
                "identifications_and_conditions": ",\n".join(
                    _dedup(identifications_and_conditions)
                ),
                "overriden_conditions": ",".join(_dedup(final_conditions)),
                "tracking_issues": "\n".join(_dedup(tracking_issues)),
                "email_subject_lines": "\n==============\n".join(
                    _dedup(email_subject_lines)
                ),
                "comments": "\n==============\n".join(_dedup(comments)),
                "public_source_mirrors": "\n".join(
                    _dedup(public_source_mirrors)
                ),
                # Advanced / debugging columns
                "_spdx_license_id": license_id,
                "_dependents": _dependents_string(license, spdx_index),
                "_detailed_identifications": ",\n".join(
                    _dedup(detailed_identifications)
                ),
                "_detailed_overrides": ",\n".join(_dedup(detailed_overrides)),
            }
            row.update(shipped_info)
            row.update(identification_stats)

            writer.writerow(row)


def _write_detailed_csv(
    spdx_doc: SpdxDocument,
    spdx_index: SpdxIndex,
    classifications: LicensesClassifications,
    output_path: str,
):
    """The detailed CSV has a row for every identified license snippet, and includes the snippet text"""
    with open(output_path, "w") as csvfile:
        writer = csv.DictWriter(
            csvfile,
            fieldnames=[
                "spdx_license_id",
                "license_name",
                "dependents",
                "link",
                "identification",
                "start_line",
                "end_line",
                "total_lines",
                "conditions",
                "verified_conditions",
                "overriden_conditions",
                "overriding_rules",
                "tracking_issues",
                "email_subject_lines",
                "comments",
                "public_source_mirrors",
                "snippet_checksum",
                "snippet_text",
            ],
        )
        writer.writeheader()

        for license in spdx_doc.extracted_licenses:
            license_id = license.license_id

            if license_id in classifications.classifications_by_id:
                for identification in classifications.classifications_by_id[
                    license_id
                ].identifications:
                    row = {
                        "spdx_license_id": license_id,
                        "license_name": license.name,
                        "dependents": _dependents_string(license, spdx_index),
                        "link": _link_string(license, spdx_index),
                        "identification": identification.identified_as,
                        "start_line": identification.start_line,
                        "end_line": identification.end_line,
                        "total_lines": identification.number_of_lines(),
                        "conditions": "\n".join(
                            sorted(list(identification.conditions))
                        ),
                        "verified_conditions": "\n".join(
                            sorted(list(identification.verified_conditions))
                        ),
                        "snippet_checksum": identification.snippet_checksum,
                        "snippet_text": identification.snippet_text,
                    }

                    if identification.overriden_conditions:
                        row["overriden_conditions"] = "\n".join(
                            sorted(list(identification.overriden_conditions))
                        )

                    if identification.overriding_rules:
                        row["overriding_rules"] = "\n".join(
                            [
                                r.rule_file_path
                                for r in identification.overriding_rules
                            ]
                        )
                        row["email_subject_lines"] = "\n=======\n".join(
                            _dedup(
                                [
                                    "\n".join(r.email_subject_line)
                                    for r in identification.overriding_rules
                                    if r.email_subject_line
                                ]
                            )
                        )
                        row["comments"] = "\n=======\n".join(
                            _dedup(
                                [
                                    "\n".join(r.comment)
                                    for r in identification.overriding_rules
                                    if r.comment
                                ]
                            )
                        )
                        row["tracking_issues"] = "\n".join(
                            [
                                r.bug
                                for r in identification.overriding_rules
                                if r.bug
                            ]
                        )
                    if identification.public_source_mirrors:
                        row["public_source_mirrors"] = "\n".join(
                            _dedup(identification.public_source_mirrors)
                        )

                    writer.writerow(row)


def _zip_everything(output_dir_path: str, output_zip_path: str):
    _log("zipping license review material")
    with zipfile.ZipFile(output_zip_path, mode="w") as archive:
        for root, _, files in os.walk(output_dir_path):
            for file in files:
                archive.write(
                    os.path.join(root, file),
                    os.path.relpath(os.path.join(root, file), output_dir_path),
                )


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
        "--classification_input",
        help="A json file containing the results of"
        " @fuchsia_sdk `fuchsia_licenses_classification`",
        required=False,
    )
    parser.add_argument(
        "--extra_files",
        help="Additional files to add to the output archive.",
        type=str,
        nargs="+",
        required=False,
    )
    parser.add_argument(
        "--output_file",
        help="Where to write the archive containing all the output files.",
        required=True,
    )
    parser.add_argument(
        "--output_dir",
        help="Where to write all the output files.",
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

    spdx_input_path = args.spdx_input
    output_dir = args.output_dir
    classification_input_path = args.classification_input

    _log(f"Reading license info from {spdx_input_path}!")
    spdx_doc = SpdxDocument.from_json(spdx_input_path)
    spdx_index = SpdxIndex.create(spdx_doc)

    _log(f"Outputing all the files into {output_dir}!")

    spdx_doc.to_json(os.path.join(output_dir, "licenses.spdx.json"))

    if classification_input_path:
        shutil.copy(
            classification_input_path,
            os.path.join(output_dir, "classification.json"),
        )
        classifications = LicensesClassifications.from_json(
            classification_input_path
        )
    else:
        classifications = LicensesClassifications.create_empty()

    extracted_licenses_dir = os.path.join(output_dir, "extracted_licenses")
    os.mkdir(extracted_licenses_dir)
    for license in spdx_doc.extracted_licenses:
        with open(
            os.path.join(extracted_licenses_dir, f"{license.license_id}.txt"),
            "w",
        ) as license_file:
            license_file.write(license.extracted_text)

    _write_summary_csv(
        spdx_doc,
        spdx_index,
        classifications,
        os.path.join(output_dir, "summary.csv"),
    )

    _write_detailed_csv(
        spdx_doc,
        spdx_index,
        classifications,
        os.path.join(output_dir, "detailed.csv"),
    )

    if args.extra_files:
        extra_files_dir = os.path.join(output_dir, "extra_files")
        os.mkdir(extra_files_dir)
        for source in args.extra_files:
            destination = os.path.join(
                extra_files_dir, os.path.basename(source)
            )
            shutil.copy(source, destination)

    output_file_path = args.output_file
    _log(f"Saving all the files into {output_file_path}!")
    _zip_everything(output_dir, output_file_path)


if __name__ == "__main__":
    main()
