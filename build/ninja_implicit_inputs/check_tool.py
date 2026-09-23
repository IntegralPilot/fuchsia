#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Check for unknown Ninja implicit inputs after an `fx build` invocation.

For context, see //build/ninja_implicit_inputs/README.md.

This script is used to detect which GN target declarations must be fixed,
to declare properly their implicit source inputs through GN metadata
collection.

After the build, invoke the script passing as arguments a list of root
GN labels or Ninja paths. All implicit inputs stored in the Ninja deps log
will be compared with the following:

- The list of explicit inputs declared in the Ninja build plan.

- The list of implicit input files collected through GN metadata
  collection, and listed in the //build/ninja_implicit_inputs:manifest
  output file.

- The list of implicit input directories collected through GN metadata
  collection, and listed in the //build/ninja_implicit_inputs:manifest
  output file.

Any implicit input that doesn't belong to one of the list above is an error.
This script will print the GN labels of targets which depend on these files,
as well as their list, plus some human-readable instructions on how to fix
most of the problems.

Most of the time, this corresponds to missing headers declarations in
C++ target definitions.

Example usage, this is the simplest, which omits missing input files

```
fx build //zircon/public/sysroot_sdk
python3 build/ninja_implicit_inputs/check_tool.py //zircon/public/sysroot_sdk
```

Use --check-missing-inputs to also check for inputs that are missing from
the Fuchsia source tree. These are generally C++ headers that no longer
exist, or due to typographical errors.

This is not enabled by default because this over-reports errors from
unrelated //third_party targets, unless your `args.gn` is modified like
in the following example:

```
ROOT_LABEL=//zircon/public/sysroot_sdk
echo "ninja_implicit_inputs_root_labels = [ \"$ROOT_LABEL\" ]" >> out/default/args.gn
fx build $ROOT_LABEL
python3 build/ninja_implicit_inputs/check_tool.py \
    --check-missing-inputs $ROOT_LABEL
```
"""

import argparse
import json
import os
import subprocess
import sys
import typing as T
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import ninja_implicit_inputs as nii

sys.path.insert(0, os.path.join(_SCRIPT_DIR, "../bazel/scripts"))
import build_utils

_DEFAULT_OUTPUT = "/tmp/implicit_inputs.txt"


class DebugDir:
    """Models a directory where to store large files computed by this script.

    Useful to inspect the results when debugging what this tool does.
    Usage is:

      1) Create instance
      2) Call log_lines() or log_json() as many times as needed.

    Each file is written with an sequence index prefix, e.g.

       1-first
       2-second
       3-third
       ...
    """

    def __init__(self, path: None | Path) -> None:
        self._dir = path
        self._index = 1

    @property
    def enabled(self) -> bool:
        """Return True is logging is enabled."""
        return bool(self._dir)

    def _get_log_path(self, filename: str) -> None | Path:
        if not self._dir:
            return None

        result = self._dir / f"{self._index}-{filename}"
        self._index += 1
        return result

    def log_lines(self, filename: str, lines: T.Iterable[str]) -> None:
        """Write a list of strings into a debug text file.

        Args:
            filename: A filename. A unique '<index>-' prefix will
                be prepended to it automatically.
            lines: A sequence of strings. Each one will be written
                as a new text line in the output file.
        """
        log_path = self._get_log_path(filename)
        if log_path:
            log_path.write_text("\n".join(sorted(lines)))

    def log_json(self, filename: str, content: T.Any) -> None:
        """Write a JSON value into a debug output file.

        Args:
            filename: A filename. A unique '<index>-' prefix will
                be prepended to it automatically.
            content: A JSON-serializable value. Written as indented
                JSON string into the output file, for readability.
        """
        log_path = self._get_log_path(filename)
        if log_path:
            with log_path.open("w") as f:
                json.dump(content, f, indent=2)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawTextHelpFormatter
    )
    parser.add_argument(
        "--fuchsia-dir",
        type=Path,
        help="Path to Fuchsia source directory (auto-detected)",
    )
    parser.add_argument(
        "--build-dir",
        type=Path,
        help="Path to Ninja build directory (auto-detected)",
    )
    parser.add_argument(
        "--debug-dir",
        type=Path,
        help="Path to a directory that will contain debug files for this tool.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=_DEFAULT_OUTPUT,
        help=f"Save output to file, default to {_DEFAULT_OUTPUT}",
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="Perform build before running the tool. Implies --check-missing-inputs.",
    )
    parser.add_argument(
        "--check-missing-inputs",
        action="store_true",
        help="Also check for missing explicit inputs. The result is only correct "
        + "if a build was performed.",
    )
    parser.add_argument(
        "targets",
        metavar="TARGET",
        type=str,
        nargs="+",
        help="Either a Ninja target path, or a GN label.",
    )
    args = parser.parse_args()

    def log(msg: str) -> None:
        print(msg, file=sys.stderr)

    bazel_paths = build_utils.BazelPaths.new(args.fuchsia_dir, args.build_dir)
    host_tag = build_utils.get_host_tag()

    fuchsia_dir = bazel_paths.fuchsia_dir
    build_dir = bazel_paths.ninja_build_dir

    if args.build:
        args.check_missing_inputs = True

        build_cmd = [f"{fuchsia_dir}/scripts/fx", "build"] + args.targets
        ret = subprocess.run(build_cmd)
        if ret.returncode != 0:
            return ret.returncode

    debug_dir = DebugDir(args.debug_dir)

    ninja_tool = fuchsia_dir / f"prebuilt/third_party/ninja/{host_tag}/ninja"

    ninja_runner, ninja_outputs = nii.create_ninja_runner_and_outputs(
        ninja_tool, build_dir
    )

    # First qualify all root GN targets passed as argument
    gn_qualifier = nii.create_gn_qualifier(build_dir)
    root_targets = [
        gn_qualifier.qualify_label(target)
        for target in args.targets
        if target.startswith("//")
    ] + [target for target in args.targets if not target.startswith("//")]

    if not args.check_missing_inputs:
        # If the value of ninja_implicit_inputs_root_label matches
        # or root_targets set, we can enable --check-missing-inputs
        # automatically since the results will be correct.
        with (build_dir / "args.json").open() as f:
            args_json = json.load(f)

        args_root_targets = [
            gn_qualifier.qualify_label(target)
            for target in args_json.get("ninja_implicit_inputs_root_labels", [])
        ]

        if set(root_targets) == set(args_root_targets):
            log("Enabling --check-missing-inputs due to build configuration.")
            args.check_missing_inputs = True

    # In order to properly map toolchain-less GN labels to their
    # full form (e.g. //src/foo -> //src/foo:foo(//build/toolchain/fuchsia:arm64)),
    # a GnLabelQualifier instance which knows about the current build
    # configuration is required.

    ninja_targets = []
    for target in root_targets:
        if target.startswith("//"):
            qualified_label = gn_qualifier.qualify_label(target)
            ninja_paths = ninja_outputs.gn_label_to_paths(qualified_label)
            if not ninja_paths:
                parser.error(f"Unknown GN label: {target}")
            # Only the first output is necessary for the queries below.
            ninja_targets.append(ninja_paths[0])
        else:
            ninja_targets.append(target)

    log(f"Number of root Ninja paths: {len(ninja_targets)}")
    debug_dir.log_lines("ninja_targets.txt", ninja_targets)

    # Retrieve known implicit input files and directories.
    log(f"Loading implicit inputs manifest.")
    implicit_inputs = nii.ImplicitInputs.create_from_build_dir(
        bazel_paths.ninja_build_dir
    )
    log(
        f"Found {len(implicit_inputs.all_known_files)} implicit files, and {len(implicit_inputs.all_known_dirs)} implicit directories"
    )

    debug_dir.log_lines("known_files.txt", implicit_inputs.all_known_files)
    debug_dir.log_lines("known_directories.txt", implicit_inputs.all_known_dirs)

    log(f"Finding unknown implicit source inputs from Ninja deps log.")
    unknown_source_inputs = nii.find_unknown_implicit_source_inputs(
        ninja_targets, implicit_inputs, ninja_runner
    )

    if debug_dir.enabled:
        # set() is not JSON-serializable, so convert it to a sorted list for output.
        unknown_map_json = {
            build_target: sorted(implicit_inputs)
            for build_target, implicit_inputs in unknown_source_inputs.items()
        }
        debug_dir.log_json("unknown_map.json", unknown_map_json)

    unknown_inputs: set[str] = set()
    for unknown_sources in unknown_source_inputs.values():
        unknown_inputs.update(unknown_sources)

    log(f"Found {len(unknown_inputs)} unknown input paths.")
    debug_dir.log_lines("unknown_inputs.txt", unknown_inputs)

    output_file: None | T.TextIO = None
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        output_file = args.output.open("w")

    has_error = False

    # First, check for missing files
    if args.check_missing_inputs:
        log(f"Checking for missing input files.")
        gn_labels_to_missing = implicit_inputs.check_for_missing_files(
            fuchsia_dir, build_dir
        )
        if debug_dir.enabled:
            gn_labels_to_missing_json = {
                gn_label: sorted(missing_files)
                for gn_label, missing_files in gn_labels_to_missing.items()
            }
            debug_dir.log_json(
                "gn_labels_to_missing.json", gn_labels_to_missing_json
            )

        if gn_labels_to_missing:
            has_error = True
            nii.print_missing_source_inputs_error(
                gn_labels_to_missing, fuchsia_dir, build_dir, sys.stdout
            )
            if output_file:
                nii.print_missing_source_inputs_error(
                    gn_labels_to_missing, fuchsia_dir, build_dir, output_file
                )

    # Second, check for unlisted depfile inputs.
    log(f"Checking for unknown depfile inputs.")
    gn_labels_to_sources = nii.map_implicit_source_inputs(
        unknown_inputs, bazel_paths.fuchsia_dir, ninja_runner, ninja_outputs
    )

    if debug_dir.enabled:
        # set() is not serializable.
        gn_labels_json = {
            gn_label: sorted(implicit_inputs)
            for gn_label, implicit_inputs in gn_labels_to_sources.items()
        }
        debug_dir.log_json("gn_labels_to_sources.json", gn_labels_json)

    if gn_labels_to_sources:
        has_error = True
        nii.print_implicit_source_inputs_error(gn_labels_to_sources, sys.stdout)
        if output_file:
            nii.print_implicit_source_inputs_error(
                gn_labels_to_sources, output_file
            )

    if has_error:
        if output_file:
            print(
                f"This report has been saved to {args.output} for easier reading."
            )
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
