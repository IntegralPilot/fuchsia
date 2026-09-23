#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility that classifies the licenses in an SPDX file."""

import argparse
import os
import subprocess
import sys
from pathlib import Path

from fuchsia.tools.licenses.classification_types import *
from fuchsia.tools.licenses.spdx_types import *

_VERBOSE = True


def _log(*kwargs):
    if _VERBOSE:
        print(*kwargs, file=sys.stderr)


def _prepare_license_files(
    license_files_dir: str, spdx_doc: SpdxDocument
) -> dict[str, str]:
    """Extract license texts in the spdx_doc into separate files"""

    # Reuse files with duplicate license texts to speed up classification
    file_by_unique_text: dict[str, str] = {}

    license_files_by_id = {}

    for license in spdx_doc.extracted_licenses:
        id = license.license_id
        text = license.extracted_text
        if text in file_by_unique_text:
            file_path = file_by_unique_text[text]
        else:
            file_path = os.path.join(license_files_dir, id + ".txt")
            file_by_unique_text[text] = file_path
            Path(file_path).write_text(text)

        license_files_by_id[id] = file_path

    _log(
        f"Found {len(file_by_unique_text.keys())} unique license texts in"
        f" {len(license_files_by_id.keys())} extracted licenses."
    )

    return license_files_by_id


def _invoke_identify_license(
    identify_license_path: str,
    identify_license_output_path: str,
    license_files_dir: str,
    license_files_by_id: dict[str, str],
) -> LicensesClassifications:
    """Invokes identify_license tool, returning an LicensesClassifications."""

    license_paths = sorted(list(set(license_files_by_id.values())))

    for path in [identify_license_path, license_files_dir] + license_paths:
        assert os.path.exists(path), f"{path} doesn't exist"

    _log(
        f"Producing {identify_license_output_path} using {identify_license_path}"
    )

    command = [
        identify_license_path,
        "-headers",
        f"-json={identify_license_output_path}",
        "-include_text=true",
        "-ignorable=true",
        "-copyright=true",
        license_files_dir,
    ]

    _log(f"identify_license invocation = {command}")

    # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
    # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
    # inheritance which can confuse child Python processes.
    env = dict(os.environ)
    env.pop("RUNFILES_DIR", None)
    env.pop("RUNFILES_MANIFEST_FILE", None)

    result = subprocess.run(command, text=True, capture_output=True, env=env)
    if result.returncode != 0:
        raise RuntimeError(
            f"""Failed to invoke {command}
Returncode={result.returncode}.
Output=`{result.stdout}`
Error=`{result.stderr}`"""
        )

    assert os.path.exists(
        identify_license_output_path
    ), f"{identify_license_output_path} doesn't exist"

    classifications = LicensesClassifications.from_identify_license_output_json(
        identify_license_output_path,
        license_files_by_id,
    )

    _log(
        f"Found {classifications.identifications_count()} identifications for {classifications.licenses_count()} licenses"
    )

    return classifications


def _get_failing_license_files(
    classification: LicensesClassifications,
    license_files_by_id: dict[str, str],
) -> list[str]:
    """Returns a sorted list of unique file paths for licenses that failed verification or were unidentified."""
    failing_files = set()
    for license_id, lic_class in classification.classifications_by_id.items():
        for snippet in lic_class.identifications:
            if (
                snippet.identified_as
                == IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION
                or not snippet.verified
            ):
                if license_id in license_files_by_id:
                    failing_files.add(license_files_by_id[license_id])
                break
    return sorted(failing_files)


def _invoke_identify_license_single_file(
    identify_license_path: str,
    file_path: str,
    output_json_path: str,
) -> list[IdentifiedSnippet]:
    """Invokes identify_license on a single file and returns its IdentifiedSnippets."""
    command = [
        identify_license_path,
        "-headers",
        f"-json={output_json_path}",
        "-include_text=true",
        "-ignorable=true",
        "-copyright=true",
        file_path,
    ]

    env = dict(os.environ)
    env.pop("RUNFILES_DIR", None)
    env.pop("RUNFILES_MANIFEST_FILE", None)

    result = subprocess.run(command, text=True, capture_output=True, env=env)
    snippets = []
    if result.returncode == 0 and os.path.exists(output_json_path):
        try:
            with open(output_json_path, "r") as f:
                json_output = json.load(f)
        except Exception:
            json_output = []
        if isinstance(json_output, list):
            for one_output in json_output:
                if (
                    os.path.normpath(one_output.get("Filepath", ""))
                    == os.path.normpath(file_path)
                    or len(json_output) == 1
                ):
                    for match_json in one_output.get("Classifications") or []:
                        snippets.append(
                            IdentifiedSnippet.from_identify_license_dict(
                                dictionary=match_json,
                                location=output_json_path,
                            )
                        )

    if not snippets:
        num_lines = 1
        if os.path.exists(file_path):
            with open(file_path, "r") as f:
                num_lines = len(f.readlines())
        snippets.append(
            IdentifiedSnippet(
                identified_as=IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
                confidence=0.0,
                start_line=1,
                end_line=max(1, num_lines),
                conditions=set(["unidentified"]),
            )
        )
    return snippets


def _retry_failing_files(
    raw_classification: LicensesClassifications,
    failing_files: list[str],
    license_files_by_id: dict[str, str],
    identify_license_path: str,
    identify_license_output_path: str,
    num_retries: int = 3,
) -> LicensesClassifications:
    """Re-runs identify_license sequentially num_retries times on each failing file and applies the majority result."""
    license_ids_by_file = defaultdict(list)
    for lid, fpath in license_files_by_id.items():
        license_ids_by_file[fpath].append(lid)

    replacements = []
    for file_idx, file_path in enumerate(failing_files):
        runs = []
        for attempt in range(1, num_retries + 1):
            retry_output_path = f"{identify_license_output_path}.retry_{file_idx}_{attempt}.json"
            snippets = _invoke_identify_license_single_file(
                identify_license_path=identify_license_path,
                file_path=file_path,
                output_json_path=retry_output_path,
            )
            runs.append(snippets)

        (
            winning_snippets,
            vote_count,
        ) = LicensesClassifications.select_majority_identifications(runs)
        _log(
            f"Retry for {file_path}: majority vote ({vote_count}/{num_retries}) selected "
            f"{[s.identified_as for s in winning_snippets]}"
        )
        for lid in license_ids_by_file[file_path]:
            replacements.append(
                LicenseClassification(
                    license_id=lid, identifications=winning_snippets
                )
            )

    return raw_classification.replace_classifications(replacements)


def _check_for_missing_identifications(
    spdx_doc: SpdxDocument,
    spdx_index: SpdxIndex,
    classifications: LicensesClassifications,
) -> LicensesClassifications:
    extra_classifications = []
    unclassified_licenses = []
    for l in spdx_doc.extracted_licenses:
        if l.license_id not in classifications.license_ids():
            unclassified_licenses.append(l.license_id)

    if unclassified_licenses:
        error_details = []
        for license_id in sorted(unclassified_licenses):
            spdx_license = spdx_index.get_license_by_id(license_id)
            text = spdx_license.extracted_text
            if not text.strip():
                preview = "[EMPTY OR WHITESPACE ONLY]"
            else:
                # Log the first 100 characters to help debugging why the classifier skipped it.
                preview = repr(text[:100])
                if len(text) > 100:
                    preview += "..."

            detail = (
                f"  - ID: {license_id}\n"
                f"    Name: {spdx_license.name}\n"
                f"    Text: {preview}"
            )
            if spdx_license.debug_hint:
                detail += f"\n    Hint: {spdx_license.debug_hint}"

            chains = spdx_index.dependency_chains_for_license(spdx_license)
            dependents = [">".join([p.name for p in chain]) for chain in chains]
            dependents = sorted(set(dependents))
            if dependents:
                dependents_str = "\n".join([f"      {d}" for d in dependents])
                detail += f"\n    Dependents:\n{dependents_str}"

            error_details.append(detail)

        raise RuntimeError(
            """
License files without any identification:
{license_names}

Details:
{details}
""".format(
                license_names="\n".join(sorted(unclassified_licenses)),
                details="\n".join(error_details),
            )
        )

    return classifications.add_classifications(extra_classifications)


def _load_override_rules(rule_paths: list[str]) -> ConditionOverrideRuleSet:
    rules = []
    for p in rule_paths:
        rule_set = ConditionOverrideRuleSet.from_json(p)
        rules.extend(rule_set.rules)
    return ConditionOverrideRuleSet(rules)


def _apply_policy_and_overrides(
    classification: LicensesClassifications,
    policy_override_rules_file_paths: list[str],
    allowed_conditions: list[str],
) -> LicensesClassifications:
    if policy_override_rules_file_paths:
        override_rules = _load_override_rules(policy_override_rules_file_paths)
        classification = classification.override_conditions(override_rules)

    classification = classification.verify_conditions(set(allowed_conditions))

    _log(
        f"{classification.failed_verifications_count()} of {classification.identifications_count()} identification failed verification"
    )

    return classification


def _verification_error_message(
    classifications: LicensesClassifications, preamble_file_path
) -> str:
    message: list[str] = [
        "ERROR: Licenses verification failed. See following details."
    ]

    def p(s: str) -> None:
        message.append(s)

    if preamble_file_path:
        with open(preamble_file_path, "r") as preamble_file:
            preamble_text = preamble_file.read()
            p("=====================")
            p(preamble_text)
            p("=====================")

    verification_messages = classifications.verification_errors()

    message_count = len(verification_messages)
    max_verification_errors = 100

    if message_count > max_verification_errors:
        verification_messages = verification_messages[0:max_verification_errors]

    for i in range(0, len(verification_messages)):
        p(f"==========================")
        p(f"VERIFICATION MESSAGE {i+1}/{message_count}:")
        p(f"==========================")
        p(verification_messages[i])

    if message_count > max_verification_errors:
        p(
            f"WARNING: Too many verification errors. Only showing the first {max_verification_errors} of {message_count} errors."
        )

    return "\n".join(message)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--spdx_input",
        help="An SPDX json file containing all licenses to process."
        "The output of @fuchsia_sdk `fuchsia_licenses_spdx`",
        required=True,
    )
    parser.add_argument(
        "--identify_license_bin",
        help="Path to the identify_license binary. "
        "Expecting a binary with the same I/O as "
        "https://github.com/google/licenseidentify_license/tree/main/tools/identify_license",
        required=True,
    )
    parser.add_argument(
        "--identify_license_output",
        help="Path to json file output by running identify_license binary.",
        required=True,
    )
    parser.add_argument(
        "--policy_override_rules",
        help="Condition override rule files (JSON files)",
        nargs="*",
        required=True,
        default=[],
    )
    parser.add_argument(
        "--default_is_project_shipped",
        help="Default value for whether OSS projects are shipped",
        type=bool,
        required=False,
        default=False,
    )
    parser.add_argument(
        "--default_is_notice_shipped",
        help="Default value for whether OSS notice files are shipped",
        type=bool,
        required=False,
        default=False,
    )
    parser.add_argument(
        "--default_is_source_code_shipped",
        help="Default value for whether OSS source code is shipped",
        type=bool,
        required=False,
        default=False,
    )
    parser.add_argument(
        "--allowed_conditions",
        help="Conditions that are allowed",
        nargs="*",
        required=False,
        default=[],
    )

    parser.add_argument(
        "--conditions_requiring_shipped_notice",
        help="""Only licenses with at least one identification with the given conditions
will be shipped as a notice text. If empty, all licenses will be shipped as notice text.
""",
        nargs="*",
        required=False,
        default=[],
    )

    parser.add_argument(
        "--fail_on_disallowed_conditions",
        help="The tool will fail when classifications map to conditions not in the allowed list",
        type=bool,
        required=False,
        default=False,
    )

    parser.add_argument(
        "--failure_message_preamble",
        help="""Path to a text file that contains a failure message preamble.
The message will be pre-pended to the standard generated failure message,
allowing downstream customers to provide project specific instructions.
""",
        required=False,
    )

    parser.add_argument(
        "--output_file",
        help="Where to write the output json",
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

    spdx_input = args.spdx_input

    _log(f"Reading license info from {spdx_input}!")
    spdx_doc = SpdxDocument.from_json(spdx_input)
    spdx_index = SpdxIndex.create(spdx_doc)

    licenses_dir = "input_licenses"
    os.mkdir(licenses_dir)

    license_files_by_id = _prepare_license_files(licenses_dir, spdx_doc)

    raw_classification = _invoke_identify_license(
        identify_license_path=args.identify_license_bin,
        identify_license_output_path=args.identify_license_output,
        license_files_dir=licenses_dir,
        license_files_by_id=license_files_by_id,
    )

    classification = _process_classifications(
        raw_classification, spdx_doc, spdx_index, args
    )

    if args.fail_on_disallowed_conditions:
        failing_files = _get_failing_license_files(
            classification, license_files_by_id
        )
        if failing_files:
            _log(
                f"Found {len(failing_files)} failing/unidentified license files. "
                "Re-running identify_license sequentially 3x on each failing file..."
            )
            raw_classification = _retry_failing_files(
                raw_classification=raw_classification,
                failing_files=failing_files,
                license_files_by_id=license_files_by_id,
                identify_license_path=args.identify_license_bin,
                identify_license_output_path=args.identify_license_output,
                num_retries=3,
            )
            classification = _process_classifications(
                raw_classification, spdx_doc, spdx_index, args
            )

    output_json_path = args.output_file
    _log(f"Writing classification into {output_json_path}!")
    classification.to_json(output_json_path)

    if args.fail_on_disallowed_conditions:
        if classification.failed_verifications_count() > 0:
            _log("ERROR: Licenses verification failed.")
            raise RuntimeError(
                _verification_error_message(
                    classification,
                    preamble_file_path=args.failure_message_preamble,
                )
            )


def _process_classifications(
    classification: LicensesClassifications,
    spdx_doc: SpdxDocument,
    spdx_index: SpdxIndex,
    args: argparse.Namespace,
) -> LicensesClassifications:
    classification = _check_for_missing_identifications(
        spdx_doc,
        spdx_index,
        classification,
    )
    classification = classification.set_is_shipped_defaults(
        is_project_shipped=args.default_is_project_shipped,
        is_notice_shipped=args.default_is_notice_shipped,
        is_source_code_shipped=args.default_is_source_code_shipped,
    )

    classification = classification.compute_identification_stats(spdx_index)
    classification = classification.add_licenses_information(spdx_index)
    classification = _apply_policy_and_overrides(
        classification,
        policy_override_rules_file_paths=args.policy_override_rules,
        allowed_conditions=args.allowed_conditions,
    )

    if args.conditions_requiring_shipped_notice:
        classification = classification.determine_is_notice_shipped(
            conditions_requiring_shipped_notice=args.conditions_requiring_shipped_notice
        )
    return classification


if __name__ == "__main__":
    main()
