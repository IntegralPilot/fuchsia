#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import dataclasses
import logging
from typing import ClassVar

from file_access import FileAccess
from gn_label import GnLabel


@dataclasses.dataclass
class Readme:
    readme_label: GnLabel
    package_name: str | None
    license_files: tuple[GnLabel, ...]

    @staticmethod
    def from_text(
        readme_label: GnLabel, applicable_target: GnLabel, file_text: str
    ) -> "Readme":
        package_name = None
        last_license_file_path = ""
        last_license_file_url = ""
        license_files: list[GnLabel] = []
        lines = file_text.split("\n")
        for l in lines:
            if ":" in l:
                splitted_line = [s.strip() for s in l.split(":", 1)]
                key = splitted_line[0].upper()
                value = splitted_line[1]
                if not value:
                    continue
                if key == "NAME":
                    package_name = value
                elif key == "LICENSE FILE":
                    if last_license_file_path:
                        license_file = applicable_target.create_child_from_str(
                            last_license_file_path, url=last_license_file_url
                        )
                        license_files.append(license_file)
                        last_license_file_path = ""
                        last_license_file_url = ""
                    last_license_file_path = value
                elif key == "LICENSE FILE URL" and last_license_file_path:
                    last_license_file_url = value

        if last_license_file_path:
            license_file = applicable_target.create_child_from_str(
                last_license_file_path, url=last_license_file_url
            )
            license_files.append(license_file)

        return Readme(
            readme_label=readme_label,
            package_name=package_name,
            license_files=tuple(license_files),
        )


@dataclasses.dataclass
class ReadmesDB:
    file_access: FileAccess
    cache: dict[GnLabel, Readme | None] = dataclasses.field(
        default_factory=dict
    )

    _barrier_dir_names: ClassVar[set[str]] = set(
        [
            "third_party",
            "thirdparty",
            "prebuilt",
            "prebuilts",
            # "contrib", TODO(134885): Also add contrib as a boundary name.
        ]
    )

    def find_readme_for_label(self, target_label: GnLabel) -> Readme | None:
        GnLabel.check_type(target_label)

        logging.debug("Finding readme for %s", target_label)
        if target_label.toolchain:
            target_label = target_label.without_toolchain()
        if target_label.is_local_name:
            target_label = target_label.parent_label()
        assert not target_label.is_local_name

        value = self.cache.get(target_label, False)
        if value != False:
            logging.debug("Found %s in cache", value)
            assert not isinstance(value, bool)  # quiet mypy false positive.
            return value

        if target_label.path_str:
            readme_fuchsia_suffix = f"{target_label.path_str}/README.fuchsia"
        else:
            readme_fuchsia_suffix = "README.fuchsia"

        potential_readme_files = [
            f"vendor/google/tools/check-licenses/assets/readmes/{readme_fuchsia_suffix}",
            f"tools/check-licenses/assets/readmes/{readme_fuchsia_suffix}",
            readme_fuchsia_suffix,
        ]
        for readme_source_path in potential_readme_files:
            readme_label = GnLabel.from_path(readme_source_path)

            if self.file_access.file_exists(readme_label):
                readme = Readme.from_text(
                    readme_label,
                    applicable_target=target_label,
                    file_text=self.file_access.read_text(readme_label),
                )
                logging.debug(
                    "%s found with name=%s files=%s",
                    readme_label,
                    readme.package_name,
                    readme.license_files,
                )
                self.cache[target_label] = readme
                return readme

            logging.debug("%s does not exist", readme_label)

        if target_label.name in ReadmesDB._barrier_dir_names:
            logging.debug("%s is a barrier path", target_label)
            self.cache[target_label] = None
            return None

        if target_label.has_parent_label():
            parent = target_label.parent_label()
            logging.debug(
                "Trying with parent of %s: %s...", target_label, parent
            )

            parent_result = self.find_readme_for_label(parent)
            logging.debug(
                "Parent of %s readme is %s", target_label, parent_result
            )
            self.cache[target_label] = parent_result
            return parent_result

        logging.debug(f"No readme found for %s", target_label)
        self.cache[target_label] = None
        return None
