#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Collects licenses from GN metadata."""

import dataclasses
import enum
import logging
import os
from collections import defaultdict

from file_access import FileAccess
from gn_label import GnLabel
from gn_license_metadata import (
    GnApplicableLicensesMetadata,
    GnLicenseMetadataDB,
)
from readme_fuchsia import ReadmesDB


@dataclasses.dataclass(frozen=True, order=True)
class CollectedLicense:
    """Data about a a single collected license (may have multiple files)."""

    public_name: str
    license_files: tuple[GnLabel, ...]

    @staticmethod
    def create(
        public_name: str,
        license_files: tuple[GnLabel, ...],
    ) -> "CollectedLicense":
        assert type(public_name) is str
        assert type(license_files) is tuple
        GnLabel.check_types_in_list(list(license_files))
        return CollectedLicense(public_name, tuple(sorted(license_files)))

    def __str__(self) -> str:
        return """License(name='{name}', files=[{files}])""".format(
            name=self.public_name,
            files=", ".join([str(f) for f in self.license_files]),
        )


_readme_fallback_note = """Note:
   This error might be a red-herring if the README.fuchsia file is
   used because of a faulty 'applicable_licenses' GN setup. You would
   see other errors above/below, such as `THIRD_PARTY_RESOURCE_WITHOUT_LICENSE`
   or `THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES` errors. When
   these errors happen, we fallback to reading the README.fuchsia file."""


class CollectorErrorKind(enum.Enum):
    """The type of a CollectionError"""

    THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES = 1

    LICENSE_FILE_IN_README_NOT_FOUND = 2
    NO_LICENSE_FILE_IN_README = 3
    NO_PACKAGE_NAME_IN_README = 4

    THIRD_PARTY_GOLIB_WITHOUT_LICENSES = 5

    APPLICABLE_LICENSE_REFERENCE_DOES_NOT_EXIST = 6

    THIRD_PARTY_RESOURCE_WITHOUT_LICENSE = 7

    def explanation(self) -> str:
        messages = {
            CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES: """
The target has no `applicable_licenses` argument, nor associated valid README.fuchsia file.
It should have at lease one of these set up.

To add applicable_licenses argument, see //build/licenses/license.gni.

To add README.fuchsia file, see:
https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata.

Notes:
1. If you still seeing this error after adding applicable_licenses argument,
   it might be due incomplete forwarding of the `applicable_licenses` argument
   through custom GN action templates, downwards the to that basic actions. Make
   sure `forward_variables_from(invoker, ["applicable_licenses"])` is called
   in all GN paths.
2. If a README.fuchsia file exists but you still see this error, it could
   be due to other errors, which should be printed above/below.
""",
            CollectorErrorKind.LICENSE_FILE_IN_README_NOT_FOUND: """
The `License File: ...` specified in the README.fuchsia file does not exist.

"""
            + _readme_fallback_note,
            CollectorErrorKind.NO_LICENSE_FILE_IN_README: """
The README.fuchsia file of the target is missing a `License File: ...` value.
"""
            + _readme_fallback_note,
            CollectorErrorKind.NO_PACKAGE_NAME_IN_README: """
The README.fuchsia file of the target is missing a `Name: [package name]` value.
"""
            + _readme_fallback_note,
            CollectorErrorKind.THIRD_PARTY_GOLIB_WITHOUT_LICENSES: """
No license found in the golib source folder. We look for files named
LICENSE*, COPYRIGHT* and NOTICE*.
""",
            CollectorErrorKind.APPLICABLE_LICENSE_REFERENCE_DOES_NOT_EXIST: """
`applicable_licenses` refers to a label that does not exist or is not
a `build/licenses/license.gni` target.
""",
            CollectorErrorKind.THIRD_PARTY_RESOURCE_WITHOUT_LICENSE: """
Could not find a licenes that applies to the 3rd party resource.
The resource is referenced directly by a target but the target has
no `applicable_licenses` argument, nor associated README.fuchsia file.

To add `applicable_licenses` argument, see //build/licenses/license.gni.

To add README.fuchsia file, see:
https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata.

Notes:
1. If you still seeing this error after adding applicable_licenses argument,
   it might be due incomplete forwarding of the `applicable_licenses` argument
   through custom GN action templates, downwards the to that basic actions. Make
   sure `forward_variables_from(invoker, ["applicable_licenses"])` is called
   in all GN paths.
2. If a README.fuchsia file exists but you still see this error, it could
   be due to other errors, which should be printed above/below.
""",
        }

        return messages[self]


@dataclasses.dataclass(frozen=True)
class CollectorError:
    """Describe collection error encountered by Collector"""

    kind: CollectorErrorKind
    target_label: GnLabel
    debug_hint: str = dataclasses.field(hash=False, compare=False, default="")


@dataclasses.dataclass(frozen=False)
class CollectorStats:
    """Collector execution stats"""

    targets_loaded: int = 0
    targets_scanned: int = 0
    targets_with_licenses: int = 0
    unique_labels: int = 0
    unique_licenses: int = 0
    unique_resources: int = 0
    licenses_from_readmes: int = 0
    licenses_from_metadata: int = 0
    licenses_from_golibs: int = 0


_ignored_license_labels = set(
    [
        GnLabel.from_str("//build/licenses:no_license"),
    ]
)


@dataclasses.dataclass
class Collector:
    """Collects licenses from build information"""

    file_access: FileAccess
    metadata_db: GnLicenseMetadataDB
    readmes_db: ReadmesDB
    include_host_tools: bool
    default_license_file: GnLabel | None = None
    generate_debug_hints: bool = False
    scan_result_by_label: dict[GnLabel, bool] = dataclasses.field(
        default_factory=dict
    )
    unique_licenses: set[CollectedLicense] = dataclasses.field(
        default_factory=set
    )
    license_debug_hints: dict[CollectedLicense, list[str]] = dataclasses.field(
        default_factory=dict
    )
    unique_resources: set[GnLabel] = dataclasses.field(default_factory=set)
    errors: list[CollectorError] = dataclasses.field(default_factory=list)
    stats: CollectorStats = dataclasses.field(default_factory=CollectorStats)

    def _add_error(self, error: CollectorError) -> None:
        logging.debug(
            "Collection error %s for %s: %s",
            error.kind.name,
            error.target_label,
            error.debug_hint,
        )
        self.errors.append(error)

    def _add_license(
        self,
        public_name: str,
        license_files: tuple[GnLabel, ...],
        collection_hint: str,
    ) -> None:
        logging.debug(
            "Collected license %s %s: %s",
            public_name,
            license_files,
            collection_hint,
        )
        assert isinstance(collection_hint, str)
        lic = CollectedLicense.create(public_name, license_files)
        if lic not in self.unique_licenses:
            self.unique_licenses.add(lic)
            self.stats.unique_licenses += 1

        if self.generate_debug_hints:
            cur_hints = self.license_debug_hints.setdefault(lic, [])
            cur_hints.append(collection_hint)

    def _add_license_from_metadata(
        self, license_metadata_label: GnLabel, used_by_target: GnLabel
    ) -> None:
        if (
            license_metadata_label.without_toolchain()
            in _ignored_license_labels
        ):
            logging.debug(
                "Ignoring %s used by %s", license_metadata_label, used_by_target
            )
            return

        if license_metadata_label not in self.metadata_db.licenses_by_label:
            self._add_error(
                CollectorError(
                    kind=CollectorErrorKind.APPLICABLE_LICENSE_REFERENCE_DOES_NOT_EXIST,
                    target_label=used_by_target,
                    debug_hint=f"{license_metadata_label} referenced by {used_by_target} applicable_licenses does not exist",
                )
            )
            return

        license_metadata = self.metadata_db.licenses_by_label[
            license_metadata_label
        ]

        self._add_license(
            public_name=license_metadata.public_package_name,
            license_files=license_metadata.license_files,
            collection_hint=f"From {license_metadata.target_label} used by {used_by_target}",
        )

    def _add_license_from_readme(
        self,
        label: GnLabel,
        resource_of: GnLabel | None,
        is_empty_readme_ok: bool,
    ) -> bool:
        readme = self.readmes_db.find_readme_for_label(label)
        if not readme:
            return False

        due_to = str(label)
        if resource_of:
            due_to = f"{label} resource of {resource_of}"

        if not readme.package_name:
            if not is_empty_readme_ok:
                self._add_error(
                    CollectorError(
                        kind=CollectorErrorKind.NO_PACKAGE_NAME_IN_README,
                        target_label=label,
                        debug_hint=f"In {readme.readme_label}, needed for {due_to}",
                    )
                )
            return False

        if not readme.license_files:
            if not is_empty_readme_ok:
                self._add_error(
                    CollectorError(
                        kind=CollectorErrorKind.NO_LICENSE_FILE_IN_README,
                        target_label=label,
                        debug_hint=f"In {readme.readme_label}, needed for {due_to}",
                    )
                )
            return False

        # Ensure files actually exist:
        for lic_file in readme.license_files:
            if not self.file_access.file_exists(lic_file):
                self._add_error(
                    CollectorError(
                        kind=CollectorErrorKind.LICENSE_FILE_IN_README_NOT_FOUND,
                        target_label=label,
                        debug_hint=f"Looking for {lic_file} specified in {readme.readme_label}, needed for {due_to}",
                    )
                )
                return False

        self.stats.licenses_from_readmes += len(readme.license_files)
        self._add_license(
            public_name=readme.package_name,
            license_files=tuple(readme.license_files),
            collection_hint=f"For {due_to} via {readme.readme_label}",
        )
        return True

    def _add_licenses_for_golib(
        self, label: GnLabel, original_label: GnLabel | None = None
    ) -> bool:
        assert label.is_3p_golib()

        logging.debug("Custom golib handling for %s", label)

        if not original_label:
            original_label = label

        source_dir = GnLabel.from_str(
            "//third_party/golibs/vendor/" + label.name
        )

        license_files: list[GnLabel] = []

        public_package_name = label.name.split("/")[-1]

        def license_file_predicate(path: str) -> bool:
            file_name_upper = os.path.basename(path).upper()
            return file_name_upper in (
                "LICENSE",
                "COPYRIGHT",
                "NOTICE",
            ) or file_name_upper.startswith(("LICENSE.", "LICENSE-", "NOTICE."))

        license_files = self.file_access.search_directory(
            source_dir, path_predicate=license_file_predicate
        )

        if license_files:
            self.stats.licenses_from_golibs += len(license_files)
            self._add_license(
                public_name=public_package_name,
                license_files=tuple(license_files),
                collection_hint=f"Used by {label} (via custom license extraction for 3p golibs)",
            )
            return True
        else:
            split_name = label.name.split("/")
            if len(split_name) == 1:
                self._add_error(
                    CollectorError(
                        kind=CollectorErrorKind.THIRD_PARTY_GOLIB_WITHOUT_LICENSES,
                        target_label=label,
                        debug_hint=f"Original label={original_label}",
                    )
                )
                return False
            else:
                parent_lib = label.parent_label().create_child_from_str(
                    ":" + "/".join(split_name[0:-1])
                )
                return self._add_licenses_for_golib(
                    parent_lib, original_label=original_label
                )

    def _add_default_license(self, label: GnLabel) -> bool:
        if self.default_license_file:
            self._add_license(
                public_name="Fuchsia",
                license_files=(self.default_license_file,),
                collection_hint="Default license for non 3p targets",
            )
            return True
        else:
            return False

    def _label_requires_licenses(self, label: GnLabel) -> bool:
        # The labels in //bundles/assembly/bazel_inputs are all forwarding
        # labels to elsewhere in the tree, and have no licenses, but they
        # can trigger false-positives when they export packages from
        # the third_party folders.
        if label.gn_str.startswith("//bundles/assembly/bazel_inputs/"):
            return False
        return label.is_3rd_party() or label.is_prebuilt()

    def _scan_label(
        self,
        label: GnLabel,
        resource_of: GnLabel | None = None,
        is_resource_of_target_with_licenses: bool = False,
    ) -> bool:
        result = self.scan_result_by_label.get(label, None)
        if result is not None:
            return result
        elif not self.include_host_tools and label.is_host_target():
            logging.debug(
                "Skipping host target %s since include_host_tools=False", label
            )
            return False

        license_found = False
        is_group_target = False
        resources = None

        target_metadata: GnApplicableLicensesMetadata | None = (
            self.metadata_db.applicable_licenses_by_target.get(label, None)
        )
        if target_metadata is not None:
            self.stats.targets_scanned += 1
            is_group_target = target_metadata.is_group()
            resources = target_metadata.third_party_resources

            license_labels = target_metadata.license_labels
            if license_labels:
                logging.debug(
                    "Found applicable_licenses labels %s for %s",
                    license_labels,
                    label,
                )

                self.stats.targets_with_licenses += 1
                license_found = True
                for license_label in license_labels:
                    self.stats.licenses_from_metadata += 1
                    self._add_license_from_metadata(
                        license_metadata_label=license_label,
                        used_by_target=label,
                    )

        if not license_found and label.is_3p_golib():
            license_found = self._add_licenses_for_golib(label)

        if not license_found:
            # Group targets are common in projects with empty README.fuchsia.
            # That is ok as long as the group target has no 3p resources.
            # Also, resources (especially prebuilts) tend to also have partial README.fuchsia files, which is ok
            # if the target that references them has its own license.
            is_empty_readme_ok = (
                is_group_target and not resources
            ) or is_resource_of_target_with_licenses

            logging.debug(
                "No licenses found for %s so looking at README.fuchsia files instead (empty_ok == %s)...",
                label,
                is_empty_readme_ok,
            )
            license_found = self._add_license_from_readme(
                label=label,
                resource_of=resource_of,
                is_empty_readme_ok=is_empty_readme_ok,
            )

        # Handle 3p resources:
        resources_have_licenses = False
        if resources:
            resources_have_licenses = True
            logging.debug("Target %s uses 3p resources: %s", label, resources)
            for resource_label in resources:
                if not self._scan_label(
                    resource_label,
                    resource_of=label,
                    is_resource_of_target_with_licenses=license_found,
                ):
                    resources_have_licenses = False

        if not license_found:
            if not self._label_requires_licenses(label):
                license_found = self._add_default_license(label)
            elif resource_of:
                # 3p resources require licenses too, unless the target that references them
                # has a license.
                if not is_resource_of_target_with_licenses:
                    self._add_error(
                        CollectorError(
                            kind=CollectorErrorKind.THIRD_PARTY_RESOURCE_WITHOUT_LICENSE,
                            target_label=label,
                            debug_hint=f"A resource of {resource_of}",
                        )
                    )
            elif is_group_target and (not resources or resources_have_licenses):
                # 3P Targets are required a license, unless they are group without
                # 3p resources and these resources don't have their own license.
                logging.debug(
                    "3p group target %s doesn't need licenses, as it has no unlicensed resources.",
                    label,
                )
            else:
                self._add_error(
                    CollectorError(
                        kind=CollectorErrorKind.THIRD_PARTY_TARGET_WITHOUT_APPLICABLE_LICENSES,
                        target_label=label,
                        debug_hint="",
                    )
                )

        self.scan_result_by_label[label] = license_found
        self.stats.unique_labels += 1

        return license_found

    def collect(self) -> None:
        # Reset
        self.stats = CollectorStats()
        self.errors.clear()
        self.unique_licenses.clear()
        self.unique_resources

        for (
            target_metadata
        ) in self.metadata_db.applicable_licenses_by_target.values():
            self._scan_label(target_metadata.target_label)

    def log_errors(self, log_level: int, is_full_report: bool) -> None:
        errors_by_kind: dict[
            CollectorErrorKind, list[CollectorError]
        ] = defaultdict(list)

        for error in self.errors:
            errors_by_kind[error.kind].append(error)

        message_lines = []

        message_lines.append(
            f"Encountered {len(self.errors)} license collection errors."
        )

        for kind in errors_by_kind.keys():
            sub_errors = errors_by_kind[kind]
            count = len(sub_errors)
            message_lines.append(
                f"There are {count} targets or resources with the {kind.name} error."
            )

            if not is_full_report:
                continue

            explanation = kind.explanation()
            for explanation_line in explanation.split("\n"):
                message_lines.append(f"  {explanation_line}")

            message_lines.append(
                f"  Here are the {count} targets or resources:"
            )

            sub_message_lines = []
            for e in sub_errors:
                sub_message_lines.append(
                    f"  {e.target_label}  hint={e.debug_hint}"
                )
            sub_message_lines.sort()

            max_lines = 100
            if max_lines < len(sub_message_lines):
                trim_message = (
                    f"  (and {len(sub_message_lines) - max_lines} more)"
                )
                sub_message_lines = sub_message_lines[0:max_lines]
                sub_message_lines.append(trim_message)
            message_lines.extend(sub_message_lines)
            message_lines.append("\n")

        logging.log(log_level, "\n".join(message_lines))
