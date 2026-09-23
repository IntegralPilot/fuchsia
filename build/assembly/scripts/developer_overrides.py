#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import shutil
import sys
from dataclasses import dataclass, field
from typing import cast

from assembly import (
    FilePath,
    PackageCopier,
    PackagedDriverDetails,
    PackageDetails,
    fast_copy_makedirs,
)
from assembly.assembly_input_bundle import (
    CompiledComponentDefinition,
    CompiledPackageDefinition,
    DepSet,
)
from depfile import DepFile
from serialization import JSONValue, instance_from_dict, json_dump, json_load

# See //src/lib/assembly/config_schema/src/developer_overrides.rs for documentation.
# These must be kept in sync with that file.


@dataclass
class ForensicsOptions:
    build_type_override: str | None = None


@dataclass
class DeveloperOnlyOptions:
    all_packages_in_base: bool = False
    netboot_mode: bool = False
    forensics_options: ForensicsOptions = field(
        default_factory=ForensicsOptions
    )


@dataclass
class KernelOptions:
    command_line_args: list[str] = field(default_factory=list)


@dataclass
class ShellCommandEntryFromGN:
    package: str
    components: list[str]


@dataclass
class DeveloperOverridesFromGN:
    """This is the schema used to parse the developer overrides that are written by the GN template."""

    target_name: str | None

    # The following are opaque dictionaries to this script, and don't need to be specified in any
    # further detail, because they are written out just as they are read in.
    developer_only_options: DeveloperOnlyOptions = field(
        default_factory=DeveloperOnlyOptions
    )
    kernel: KernelOptions = field(default_factory=KernelOptions)
    platform: JSONValue = field(default_factory=lambda: cast(JSONValue, {}))
    product: JSONValue = field(default_factory=lambda: cast(JSONValue, {}))
    board: JSONValue = field(default_factory=lambda: cast(JSONValue, {}))
    bootfs_files_package: str | None = field(default=None)

    # Packages we need to copy, so we'll need real types for those
    packages: list[PackageDetails] = field(default_factory=list)
    drivers: list[PackagedDriverDetails] = field(default_factory=list)
    packages_to_compile: list[CompiledPackageDefinition] = field(
        default_factory=list
    )

    # The type that's deserialized from what GN writes is different from that which will be written
    # out for Assembly to use.
    shell_commands: list[ShellCommandEntryFromGN] = field(default_factory=list)


ShellCommandsForAssembly = dict[str, list[str]]


@dataclass
class DeveloperProvidedFileEntryFromGN:
    field: str
    path: str


@dataclass
class DeveloperProvidedFilesNodeFromGN:
    node_path: str
    fields: list[DeveloperProvidedFileEntryFromGN]


@dataclass
class DeveloperProvidedFilesNodeForAssembly:
    node_path: str
    fields: dict[str, str]


@dataclass
class DeveloperOverridesForAssembly:
    """This is the schema used to write out the overrides file for Assembly to use."""

    target_name: str | None

    # The following are opaque dictionaries to this script, and don't need to be specified in any
    # further detail, because they are written out just as they are read in.
    developer_only_options: DeveloperOnlyOptions = field(
        default_factory=DeveloperOnlyOptions
    )
    kernel: KernelOptions = field(default_factory=KernelOptions)
    platform: JSONValue = field(default_factory=lambda: cast(JSONValue, {}))
    product: JSONValue = field(default_factory=lambda: cast(JSONValue, {}))
    board: JSONValue = field(default_factory=lambda: cast(JSONValue, {}))
    bootfs_files_package: str | None = field(default=None)

    # Packages we need to copy, so we'll need real types for those
    packages: list[PackageDetails] = field(default_factory=list)
    drivers: list[PackagedDriverDetails] = field(default_factory=list)
    packages_to_compile: list[CompiledPackageDefinition] = field(
        default_factory=list
    )

    # The type that's written out for Assembly to use is different from that which is read from GN.
    shell_commands: ShellCommandsForAssembly = field(default_factory=dict)

    # A mapping of all files found in the platform and product types that are being copied and need
    # to be tracked as relative to this file.
    developer_provided_files: list[
        DeveloperProvidedFilesNodeForAssembly
    ] = field(default_factory=list)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Tool for creating the file for Assembly developer overrides in-tree"
    )
    parser.add_argument(
        "--input",
        required=True,
        type=argparse.FileType("r"),
        help="Path to a json file containing the intermediate assembly developer overrides",
    )
    parser.add_argument(
        "--input-file-paths",
        type=argparse.FileType("r"),
        help="Path to a json file containing a list of input files listed in the intermediate file",
    )
    parser.add_argument(
        "--outdir",
        required=True,
        help="Path to the output dir that will contain the developer overrides",
    )
    parser.add_argument(
        "--depfile",
        help="Path to an optional depfile to write of all files used to construct the developer overrides",
    )
    args = parser.parse_args()
    deps: DepSet = set()

    # Remove the existing <outdir>, and recreate it and the "subpackages"
    # subdirectory.
    if os.path.exists(args.outdir):
        shutil.rmtree(args.outdir)
        os.makedirs(args.outdir)

    overrides_from_gn = json_load(DeveloperOverridesFromGN, args.input)
    deps.add(args.input.name)

    # Prep the result.
    overrides_for_assembly = DeveloperOverridesForAssembly(
        overrides_from_gn.target_name,
        developer_only_options=overrides_from_gn.developer_only_options,
        kernel=overrides_from_gn.kernel,
        platform=overrides_from_gn.platform,
        product=overrides_from_gn.product,
        board=overrides_from_gn.board,
        bootfs_files_package=overrides_from_gn.bootfs_files_package,
    )

    overrides_for_assembly.shell_commands = {}
    for shell_entry in overrides_from_gn.shell_commands:
        overrides_for_assembly.shell_commands[shell_entry.package] = [
            f"bin/{name}" for name in shell_entry.components
        ]

    package_copier = None
    if (
        overrides_from_gn.packages
        or overrides_from_gn.drivers
        or overrides_from_gn.bootfs_files_package
    ):
        package_copier = PackageCopier(args.outdir)

    if package_copier:
        if overrides_from_gn.packages:
            for package_entry in overrides_from_gn.packages:
                destination_path, _ = package_copier.add_package(
                    package_entry.package
                )
                overrides_for_assembly.packages.append(
                    PackageDetails(destination_path, package_entry.set)
                )

        if overrides_from_gn.drivers:
            for driver_entry in overrides_from_gn.drivers:
                destination_path, _ = package_copier.add_package(
                    driver_entry.package
                )
                overrides_for_assembly.drivers.append(
                    PackagedDriverDetails(
                        package=destination_path,
                        set=driver_entry.set,
                        components=driver_entry.components,
                    )
                )

        if overrides_from_gn.bootfs_files_package:
            manifest_path = os.path.join(
                args.outdir, overrides_from_gn.bootfs_files_package
            )
            destination_path, _ = package_copier.add_package(manifest_path)
            overrides_for_assembly.bootfs_files_package = str(destination_path)

        _, copy_deps = package_copier.perform_copy()
        deps.update(copy_deps)

    # TODO(https://fxbug.dev/406838880) - Refactor this to use the same mechanisms in
    # assembly_input_bundle.py.
    if overrides_from_gn.packages_to_compile:
        packages_to_compile: list[CompiledPackageDefinition] = []
        for package in overrides_from_gn.packages_to_compile:
            if package.contents:
                raise ValueError(
                    "\nExtra package contents for compiled_packages are not supported at this time.\n"
                )
            if package.includes:
                raise ValueError(
                    "\nExtra component includes for compiled_packages are not supported at this time.\n"
                )
            components: list[CompiledComponentDefinition] = []
            for component in package.components:
                shards: set[FilePath] = set()
                for shard in component.shards:
                    dest = os.path.join(
                        args.outdir,
                        "compiled_packages",
                        package.name,
                        component.component_name,
                        os.path.basename(shard),
                    )
                    deps.add(shard)
                    fast_copy_makedirs(shard, dest)
                    shards.add(os.path.relpath(dest, args.outdir))
                components.append(
                    CompiledComponentDefinition(
                        component.component_name, shards
                    )
                )
            packages_to_compile.append(
                CompiledPackageDefinition(
                    package.name,
                    components,
                    bootfs_package=package.bootfs_package,
                )
            )
        overrides_for_assembly.packages_to_compile = packages_to_compile

    outfile_path = os.path.join(args.outdir, "product_assembly_overrides.json")

    # There are potentially a few file paths listed in the overrides.  They need to be copied into
    # the outdir, and then the references to them in the json override values removed as they are
    # separately tracked so that they can be resolved based on the path to the developer overrides
    # file (and directory of associated resources).

    input_file_path_entries = []
    if args.input_file_paths:
        input_file_path_entries = json.load(args.input_file_paths)

    # Copy the files to a pair of resources dirs:
    for raw_entry in input_file_path_entries:
        # The input_file_path_entries is a list of dicts, as the serialization library doesn't
        # want to deserialize a 'list[Foo]'.
        #
        # So here the list of dicts parsed from json above is individually deserialized into
        # the appropriate class.
        entry = instance_from_dict(DeveloperProvidedFilesNodeFromGN, raw_entry)

        # This is the 'fields' map that contains the new (copied-to) paths for the files in the
        # developer overrides.
        fields_for_assembly = {}

        for field_entry in entry.fields:
            input_file = field_entry.path

            # This structures the files under 'resources' in the same structure that they have
            # in the build dir, except that source-files and outputs from actions will have
            # different root dirs.
            if input_file.startswith("../../"):
                # It's a source file, so strip the leading "../../" and put it in:
                #   resources/sources/path/to/file
                new_relative_path = os.path.join(
                    "resources", "sources", input_file[6:]
                )
            else:
                # It's the output of another action, or a generated file, so put it in
                #   resources/path/to/file
                new_relative_path = os.path.join("resources", input_file)

            # Copy the file
            fast_copy_makedirs(
                input_file, os.path.join(args.outdir, new_relative_path)
            )
            # Adding the source path to the depfile
            deps.add(input_file)

            # And then add it to the map of fields with files
            fields_for_assembly[field_entry.field] = new_relative_path

        # Add the fields for this node to the overrides for assembly struct.
        overrides_for_assembly.developer_provided_files.append(
            DeveloperProvidedFilesNodeForAssembly(
                entry.node_path, fields_for_assembly
            )
        )

        # Strip the file from the main part of the developer-overrides (as GN will have written them
        # to the platform and product overrides config values.

        # Start by getting the 'platform', 'product', etc. field from the struct.  As this isn't a
        # dict, but a struct, getaddr is used.
        path_elements = entry.node_path.split(".")
        starting_node_name = path_elements[0]
        starting_node = getattr(
            overrides_for_assembly, starting_node_name, None
        )
        if starting_node is None:
            raise ValueError(f"Unknown field: {starting_node_name}")

        # Get the dict for this node, iterating through the path to get the child dicts.
        current_node = starting_node
        for node_name in path_elements[1:]:
            if not isinstance(current_node, dict):
                raise TypeError(
                    f"Expected dict for node {node_name}, got {type(current_node)}"
                )
            if not node_name in current_node:
                raise ValueError(
                    f"Unable to locate node {node_name} from {path_elements} in {starting_node_name}={starting_node}"
                )
            current_node = current_node[node_name]

        # Remove all the fields that have fields from the node.
        if not isinstance(current_node, dict):
            raise TypeError(
                f"Expected dict at end of path {entry.node_path}, got {type(current_node)}"
            )
        for field_entry in entry.fields:
            if field_entry.field not in current_node:
                raise ValueError(
                    f"Unable to locate field {field_entry.field} at {path_elements} in {starting_node_name}={starting_node}"
                )
            current_node.pop(field_entry.field)

    # Write out the depfile.
    if args.depfile:
        with open(args.depfile, "w") as depfile:
            DepFile.from_deps(outfile_path, deps).write_to(depfile)

    # And write out the output for assembly.
    with open(outfile_path, "w") as output:
        json_dump(overrides_for_assembly, output, indent=2)

    return 0


if __name__ == "__main__":
    sys.exit(main())
