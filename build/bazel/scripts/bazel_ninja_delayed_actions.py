#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"Run a set of Ninja-delayed Bazel actions"

import argparse
import dataclasses
import datetime
import json
import os
import sys
import typing as T
from pathlib import Path

# LINT.IfChange(imports)
_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import bazel_action_impl
import bazel_compdb_utils
import bazel_rust_analyzer_utils
import build_utils
from bazel_action_file_copy_utils import write_file_if_changed
from bazel_action_utils import (
    BazelGlobalArguments,
    BazelTargetInfo,
    BazelTargetInfosMap,
    update_gn_targets_symlink,
)
from workspace_utils import (
    BazelPackageAndTargetToGnInputsEntriesMap,
    BazelTargetGnInputsEntriesMap,
    GeneratedWorkspaceFiles,
    GnTargetsDirectoryManifestEntry,
    record_gn_targets_dir_from_entries,
)

_MODULES_DIR = os.path.join(_SCRIPT_DIR, "../../python/modules")
sys.path.insert(0, _MODULES_DIR)
from depfile import DepFile

# LINT.ThenChange(//build/bazel/bazel_action.gni:delayed_action_imports)

# Set this to True to debug operations locally in this script.
# IMPORTANT: Setting this to True will result in Ninja timeouts in CQ
# due to the stdout/stderr logs being too large.
_DEBUG = False

# Set this to True to enable debug printing of the action's timing profiles
_DEBUG_TIME_PROFILE = _DEBUG


@dataclasses.dataclass(frozen=True)
class TargetWithPlatform:
    target: str
    platform: str


def main() -> int:
    time_profile = build_utils.TimeProfile()
    parser = argparse.ArgumentParser(description=__doc__)

    ##
    # Options for directory that the build is running in.
    parser.add_argument(
        "--build-dir",
        type=Path,
        help="Specify Ninja build directory (defaults to current directory)",
    )
    parser.add_argument(
        "--fuchsia-dir",
        type=Path,
        help="Specify Fuchsia source directory (defaults to auto-detected)",
    )

    parser.add_argument(
        "--delayed-actions-request",
        type=Path,
        required=True,
        help="Path to a json file describing the set of actions Ninja needs run.",
    )

    parser.add_argument(
        "--delayed-actions-response",
        type=Path,
        required=True,
        help="Path to a json file to write describing the status of the action.",
    )

    args = parser.parse_args()

    time_profile.start("load_config", "Load the configuration files.")

    try:
        bazel_paths = build_utils.BazelPaths.new(
            args.fuchsia_dir, args.build_dir
        )
    except ValueError as e:
        parser.error(str(e))

    # Load the extra global settings configured via GN global args
    global_bazel_args = BazelGlobalArguments.create_from_build_dir(
        bazel_paths.ninja_build_dir
    )

    # load the BazelTargetInfos so that we can find which targets need to be built in order
    # to build the requested outputs.
    bazel_target_infos = BazelTargetInfosMap.create_from_build_dir(
        bazel_paths.ninja_build_dir
    )

    time_profile.start("query_cache", "loading Bazel query cache")
    query_cache = build_utils.BazelQueryCache(
        bazel_paths.workspace / "fuchsia_build_generated/bazel_query_cache"
    )

    time_profile.start(
        "find_targets_for_outputs",
        "Find the Bazel targets that create the given ninja outputs",
    )

    ninja_request = DelayedActionsRequest.from_json(
        args.delayed_actions_request.read_text()
    )

    # This is a nested map, first by platform, then by bazel target label, to the BazelTargetInfo
    # struct used to define the outputs for the Bazel targets that we need to build.
    targets_by_platform: dict[str, dict[str, BazelTargetInfo]] = {}

    # This is a map from a bazel target (with platform label) to the DelayedAction from ninja that's
    # requesting its outputs to be built, along with the stamp file that needs to be created so that
    # ninja can tell that the action was performed.
    target_request_map: dict[
        TargetWithPlatform, tuple[DelayedAction, Path]
    ] = {}

    for action in ninja_request.actions:
        for output in action.ninja_outputs:
            target = bazel_target_infos.get_target(output)

            if target:
                # In order to line up the results with the action IDs (and to setup the depfile
                # infos) we need to know which action is responsible for which target, which
                # means that we can't have multiple actions creating the same output file.
                target_with_platform = TargetWithPlatform(
                    target.bazel_target,
                    target.bazel_platform_label,
                )
                existing_action, _ = target_request_map.setdefault(
                    target_with_platform, (action, Path(target.stamp_path))
                )
                if existing_action is not action:
                    parser.error(
                        f"Bazel target {target.bazel_target} is requested by multiple actions: "
                        + f"{action.action_id} and {existing_action.action_id}"
                    )

                platform_targets = targets_by_platform.setdefault(
                    target.bazel_platform_label, {}
                )
                platform_targets[target.bazel_target] = target

            elif is_debugging_output(output):
                # These files are listed as ninja outputs, but aren't actually outputs of Bazel.
                pass
            else:
                parser.error(f"Can't find a Bazel target for output: {output}")

    if _DEBUG:
        print()
        print("Bazel targets to build:")
        for platform, targets in sorted(targets_by_platform.items()):
            print()
            print(f"Using platform: {platform}")
            for target in sorted(targets):
                print(f"    {target}")
            print()

    bazel_action_runner = bazel_action_impl.BazelActionRunner(
        bazel_paths,
        global_bazel_args,
        query_cache,
    )
    # This will raise an exception on failure.
    try:
        for platform_label, platform_targets in targets_by_platform.items():
            time_profile.start("merging_bazel_target_infos")

            bazel_target_infos = list(platform_targets.values())
            platform_config = bazel_target_infos[0].bazel_platform_config

            (
                outputs,
                gn_target_manifests,
            ) = bazel_action_impl.merge_target_info_outputs(bazel_target_infos)

            gn_target_manifest_entries = merge_gn_target_manifests(
                gn_target_manifests
            )

            gn_targets_dir = (
                bazel_paths.ninja_build_dir
                / "build/bazel/ninja_delayed_action.gn_targets"
            )

            # The path for this file can't be `all_licenses.spdx.json` because the
            # `update_gn_targets_symlink()` function symlinks that path to this file, which creates
            # a symbolic link to itself.
            licenses_file = gn_targets_dir / "placeholder_licenses.spdx.json"
            licenses_file.parent.mkdir(parents=True, exist_ok=True)
            licenses_file.write_text(
                "This is a placeholder file - It should always be overwritten by Ninja during a build"
            )

            time_profile.start("generate_gn_targets_dir")
            generated = GeneratedWorkspaceFiles()
            record_gn_targets_dir_from_entries(
                generated,
                bazel_paths.ninja_build_dir,
                gn_target_manifest_entries,
                licenses_file,
            )
            generated.write(gn_targets_dir)

            update_gn_targets_symlink(
                bazel_paths, gn_targets_dir, check_license_timestamps=True
            )

            action_result = bazel_action_runner.run(
                command="build",
                platform_config=platform_config,
                platform_label=platform_label,
                targets=[
                    target_info.bazel_target
                    for target_info in bazel_target_infos
                ],
                outputs=outputs,
                time_profile=time_profile,
            )

            if global_bazel_args.auto_refresh_compdb:
                compdb_file = (
                    bazel_paths.ninja_build_dir / "compile_commands.json"
                )
                time_profile.start(
                    "generate_compdb",
                    "Generate {}".format(compdb_file),
                )
                compile_commands: list[dict[str, T.Any]] = []
                if compdb_file.exists() and compdb_file.stat().st_size > 0:
                    with open(compdb_file, "r") as f:
                        compile_commands = json.load(f)
                compile_commands.extend(
                    bazel_compdb_utils.compdb_for_labels(
                        bazel_paths.ninja_build_dir,
                        str(bazel_paths.launcher),
                        action_result.configured_args,
                        [
                            target_info.bazel_target
                            for target_info in bazel_target_infos
                        ],
                    )
                )
                write_file_if_changed(
                    compdb_file,
                    json.dumps(
                        bazel_compdb_utils.dedupe(compile_commands), indent=2
                    ),
                )

            # If any of the targets that were built are flagged as requiring the updating of the
            # the rust_project.json file, then do so now.
            if any(
                target_info.update_rust_project
                for target_info in bazel_target_infos
            ):
                rust_project_file = (
                    bazel_paths.ninja_build_dir / "rust-project.json"
                )
                time_profile.start(
                    "generate_rust_project_json",
                    "Generate {}".format(rust_project_file),
                )
                _sysroot_src_subdir = Path("lib/rustlib/src/rust/library")
                rust_sysroot = global_bazel_args.rust_sysroot

                base_rust_project: dict[str, T.Any] = {}
                if (
                    rust_project_file.exists()
                    and rust_project_file.stat().st_size > 0
                ):
                    with open(rust_project_file, "r") as f:
                        base_rust_project = json.load(f)

                if "sysroot" not in base_rust_project:
                    base_rust_project["sysroot"] = str(
                        rust_sysroot.resolve().absolute()
                    )
                if "sysroot_src" not in base_rust_project:
                    base_rust_project["sysroot_src"] = str(
                        rust_sysroot.resolve().absolute() / _sysroot_src_subdir
                    )
                if "crates" not in base_rust_project:
                    base_rust_project["crates"] = []

                new_rust_project = {
                    "sysroot": str(rust_sysroot.resolve().absolute()),
                    "sysroot_src": str(
                        rust_sysroot.resolve().absolute() / _sysroot_src_subdir
                    ),
                    "crates": action_result.rust_crates,
                }
                merged_rust_project = (
                    bazel_rust_analyzer_utils.merge_rust_project_jsons(
                        base_rust_project, [new_rust_project]
                    )
                )
                write_file_if_changed(
                    rust_project_file,
                    json.dumps(merged_rust_project, indent=2),
                )

            # Update the depfiles data and the stamp file
            time_profile.start("update_depfile_and_stampfiles")
            for target, sources in action_result.source_files.items():
                # Locate the action request and stamp path for this target.
                target_with_platform = TargetWithPlatform(
                    target, platform_label
                )
                action, stamp_path = target_request_map[target_with_platform]

                # Construct a depfile for it.
                depfile = DepFile(action.ninja_outputs[0])

                # With all outputs that are in the request
                for output in action.ninja_outputs[1:]:
                    depfile.add_output(output)

                # And all the sources for the target.
                for source in sources:
                    # Don't include our @gn_targets generated BUILD.bazel files as
                    # inputs for the depfile, because they're created on the fly.
                    if not (
                        source.startswith(
                            "build/bazel/ninja_delayed_action.gn_targets"
                        )
                        and source.endswith("BUILD.bazel")
                    ):
                        depfile.add_input(source)

                # And then write out the depfile
                with open(action.ninja_depfile, "w") as f:
                    depfile.write_to(f)

                # Update the stamp file.
                timestamp = datetime.datetime.now().timestamp()
                stamp_path.parent.mkdir(parents=True, exist_ok=True)
                if stamp_path.exists():
                    stamp_path.unlink()
                with open(stamp_path, "w") as f:
                    f.write(f"{timestamp}\n")

        rc = 0

    except bazel_action_impl.BazelActionError as e:
        rc = 1
        print(str(e), file=sys.stderr)

    time_profile.stop()
    if _DEBUG_TIME_PROFILE:
        time_profile.print(0.001)

    response = DelayedActionsResponse(ninja_request.request_id, rc, "")
    write_file_if_changed(args.delayed_actions_response, response.to_json())

    # Done!  (Don't return the 'rc' from above, that's for the action itself,
    # here we need to return 0 to tell Ninja that the script exited successfully.)
    return 0


def merge_gn_target_manifests(
    manifests: list[Path],
) -> BazelPackageAndTargetToGnInputsEntriesMap:
    manifest_entries_package_map = BazelPackageAndTargetToGnInputsEntriesMap()
    for manifest_path in manifests:
        with open(manifest_path) as f:
            for entry_json in json.load(f):
                entry = GnTargetsDirectoryManifestEntry.from_json_value(
                    entry_json
                )

                bazel_package = entry.bazel_package
                name_map = manifest_entries_package_map.setdefault(
                    bazel_package, BazelTargetGnInputsEntriesMap()
                )

                bazel_name = entry.bazel_name
                found_entry = name_map.setdefault(bazel_name, entry)
                if found_entry != entry:
                    raise ValueError(
                        f"Found duplicate GN target entry for //{bazel_package}:{bazel_name}:  {found_entry.generator_label} vs {entry.generator_label}"
                    )
    return manifest_entries_package_map


@dataclasses.dataclass
class DelayedAction(object):
    action_id: int
    command: str
    description: str
    ninja_outputs: list[str]
    ninja_depfile: Path


@dataclasses.dataclass
class DelayedActionsRequest(object):
    request_id: int
    actions: list[DelayedAction]

    @classmethod
    def from_json(cls, raw: str) -> "DelayedActionsRequest":
        """Parse the json Ninja uses to describe a batch of delayed action requests.

        This parses the request from ninja which has the following schema:

            "version": Required. Integer. Must be 2
            "request_id": Required. Integer. Must be in 1..INT32_MAX range.
            "actions": Required. Array of objects. Each one with:

                "action_id": Required. Integer. Must be in 1..INT32_MAX range and
                    correspond to the action's index in the request.
                "command": Required. String. Command to run as a single string.
                "description": Optional. String. Command description from GN.
                "ninja_outputs": Optional. Array of Ninja output path strings,
                    relative to the build directory.

            "build_metadata": Optional. Object of key-value string pairs.
        """
        parsed: dict[str, T.Any] = json.loads(raw)
        assert parsed["version"] == 2

        return DelayedActionsRequest(
            request_id=int(parsed["request_id"]),
            actions=[
                DelayedAction(
                    action_id=a["action_id"],
                    command=a["command"],
                    description=a["description"],
                    ninja_outputs=a["ninja_outputs"],
                    ninja_depfile=Path(a["ninja_depfile"]),
                )
                for a in parsed["actions"]
            ],
        )


@dataclasses.dataclass
class DelayedActionsResponse(object):
    request_id: int
    status: int
    output: str

    def to_json(self) -> str:
        """Convert the response in the json format expected by Ninja.

        This converts the response into the schema expected by Ninja.
        """
        as_dict = dataclasses.asdict(self)
        as_dict["version"] = 1
        return json.dumps(as_dict, indent=2)


# These are the suffixes of files we use to debug Bazel actions, and
# they are listed as outputs in the DelayedActionsRequest, but aren't
# outputs from Bazel, that are in the BazelTargetInfos.
_DEBUGGING_OUTPUT_SUFFIXES = [
    "bazel_command.sh",
    "bazel_explain.txt",
    "debug_symbols.json",
    "bazel_action_timings.json",
    "bazel_events.log.json",
    "rust-project.json",
]


def is_debugging_output(output: str) -> bool:
    """Return whether the given output is one of our debugging outputs.

    This is used to filter out debugging outputs from the list of outputs
    passed to Bazel.
    """
    return any(output.endswith(suffix) for suffix in _DEBUGGING_OUTPUT_SUFFIXES)


if __name__ == "__main__":
    rc = main()
    sys.exit(rc)
