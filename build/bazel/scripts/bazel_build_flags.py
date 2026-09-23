# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import dataclasses
import json
from pathlib import Path


# LINT.IfChange(DefaultBuildFlagsSet)
@dataclasses.dataclass(frozen=True)
class DefaultBuildFlagsSet:
    """Models a set of default build_flags() labels to be applied to different target types.

    cxx_common_build_flags: A list of build_flags() labels whose flags will be used by
       actions of all C/C++ target types.

    cxx_executable_build_flags: A list of extra build_flags() labels, used for actions of
        C/C++ executable targets only.

    cxx_shared_library_build_flags: A list of extra build_flags() labels, used for actions
        of C/C++ shared library targets only.

    rust_common_build_flags: A list of build_flags() labels whose flags will be used by
       actions of all Rust target types.

    rust_executable_build_flags: A list of extra build_flags() labels, used for actions of
        Rust executable targets only.

    rust_shared_library_build_flags: A list of extra build_flags() labels, used for actions
        of Rust shared library targets only.

    target_compatible_with: A string for a Bazel expression listing Bazel constraint labels
        to restrict these definitions to artifacts built in specific build configurations.
    """

    cxx_common_build_flags: list[str]
    cxx_executable_build_flags: list[str]
    cxx_shared_library_build_flags: list[str]
    rust_common_build_flags: list[str]
    rust_executable_build_flags: list[str]
    rust_shared_library_build_flags: list[str]
    target_compatible_with: str


# LINT.ThenChange(//build/bazel_sdk/fuchsia_rules_common/build_flags/providers.bzl:DefaultBuildFlagsSetInfo)


class DefaultBuildFlagsMap(dict[str, DefaultBuildFlagsSet]):
    """A { name -> DefaultBuildFlagsSet } map."""

    @staticmethod
    def new_from_gn_config(build_dir: Path) -> "DefaultBuildFlagsMap":
        """Return a new instance matching the current GN build configuration.

        Args:
            build_dir: Ninja build directory.
        Returns:
            A new DefaultBuildFlagsMap instance.
        """
        # TODO(digit): Actually sync this with the GN build configuration.
        # Doing this will require BUILDCONFIG.gn to write a generated_file() output
        # that contains information that could be parsed and processed here.

        # Until then, build_dir is currently unused, but make mypy happy.
        assert build_dir

        # NOTE: While the current default only distinguishes between Fuchsia and host
        # OS values, it may be possible to provide different definitions based on
        # additional constraints, such as CPU architecture, API level, or the
        # platform/SDK split.

        return DefaultBuildFlagsMap(
            {
                "fuchsia": DefaultBuildFlagsSet(
                    cxx_common_build_flags=[],
                    cxx_executable_build_flags=[],
                    cxx_shared_library_build_flags=[],
                    rust_common_build_flags=[],
                    rust_executable_build_flags=[],
                    rust_shared_library_build_flags=[],
                    # NOTE: The same default build flags will be used for both
                    # fuchsia_platform and fuchsia_sdk artifacts produced in-tree.
                    target_compatible_with='["@platforms//os:fuchsia"]',
                ),
                "host": DefaultBuildFlagsSet(
                    cxx_common_build_flags=[],
                    cxx_executable_build_flags=[],
                    cxx_shared_library_build_flags=[],
                    rust_common_build_flags=[],
                    rust_executable_build_flags=[],
                    rust_shared_library_build_flags=[],
                    target_compatible_with="HOST_OS_CONSTRAINTS",
                ),
            }
        )

    def generate_bazel_toolchain_definitions(self) -> str:
        """Generate a BUILD.bazel fragment that defines Bazel toolchain() targets.

        These toolchains carry the default build_flags() labels describing
        the default compiler and linker flags to be used when building different
        types of C++ and Rust artifacts.

        The top-level MODULE.bazel file should call register_toolchains()
        with a filegroup() label pointing to them (or use the magic ":all" target
        name which picks up all toolchain() targets from a package).

        Returns:
            A BUILD.bazel text fragment.
        """
        content = """# Default build_flags() definition for C++ and Rust toolchains.

load("@fuchsia_rules_common//build_flags:toolchain.bzl", "build_flags_toolchain_instance")
load("@@//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")
"""
        for name, info in self.items():
            content += """
build_flags_toolchain_instance(
    name = "{name}_default_build_flags",
    cxx_common_build_flags = {cxx_common_build_flags},
    cxx_executable_build_flags = {cxx_executable_build_flags},
    cxx_shared_library_build_flags = {cxx_shared_library_build_flags},
    rust_common_build_flags = {rust_common_build_flags},
    rust_executable_build_flags = {rust_executable_build_flags},
    rust_shared_library_build_flags = {rust_shared_library_build_flags},
)

toolchain(
    name = "{name}_toolchain",
    target_compatible_with = {target_compatible_with},
    toolchain = ":{name}_default_build_flags",
    toolchain_type = "@fuchsia_rules_common//build_flags:toolchain_type",
)
""".format(
                name=name,
                # Use json.dumps() to use double-quote formatting.
                cxx_common_build_flags=json.dumps(info.cxx_common_build_flags),
                cxx_executable_build_flags=json.dumps(
                    info.cxx_executable_build_flags
                ),
                cxx_shared_library_build_flags=json.dumps(
                    info.cxx_shared_library_build_flags
                ),
                rust_common_build_flags=json.dumps(
                    info.rust_common_build_flags
                ),
                rust_executable_build_flags=json.dumps(
                    info.rust_executable_build_flags
                ),
                rust_shared_library_build_flags=json.dumps(
                    info.rust_shared_library_build_flags
                ),
                target_compatible_with=info.target_compatible_with,
            )

        return content
