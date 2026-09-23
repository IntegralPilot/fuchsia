#!/usr/bin/env fuchsia-vendored-python
# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import difflib
import filecmp
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

# Verifies that the candidate golden file matches the provided golden.

RUN_IN_NINJA_DIR_PREFIX = "fx run-in-build-dir "

MANUAL_UPDATE_HEADER = """
Please run the following to acknowledge this change:
```"""

# TODO(https://fxbug.dev/384955652): Provide the proper command when run in a sub-build.
MANUAL_UPDATE_BODY_ENTRY = """{command_prefix}cp \\
    {candidate} \\
    {golden}
"""

MANUAL_UPDATE_BODY_DEAD_GOLDENS = """
Some old golden files are no longer being checked and should be removed with:
fx run-in-build-dir git rm -f {dead_goldens}
If new golden files have been added then it may also be necessary to then do:
fx run-in-build-dir git add {golden_dir}
"""

MANUAL_UPDATE_FOOTER = """```

Or, you can simply rebuild with `update_goldens=true` set in your GN args.

Note: If you are seeing this on an automated build failure and are trying to
reproduce, ensure that
    {label}
is in your GN graph.
"""


def print_failure_msg(
    manual_updates: list[dict[str, str]],
    dead_goldens: list[str],
    golden_dir: Path,
    label: str,
    run_in_ninja_dir: bool,
) -> None:
    if manual_updates:
        print(MANUAL_UPDATE_HEADER)
    for update in manual_updates:
        print(
            MANUAL_UPDATE_BODY_ENTRY.format(
                command_prefix=RUN_IN_NINJA_DIR_PREFIX
                if run_in_ninja_dir
                else "",
                candidate=update["candidate"],
                golden=update["golden"],
            )
        )
    if dead_goldens:
        print(
            MANUAL_UPDATE_BODY_DEAD_GOLDENS.format(
                dead_goldens=" ".join(str(f) for f in sorted(dead_goldens)),
                golden_dir=golden_dir,
            )
        )
    print(MANUAL_UPDATE_FOOTER.format(label=label))


def get_diff_lines(file1: str, file2: str) -> list[str]:
    """Returns a list of strings representing the unified diff of the two file."""
    with open(file1) as f:
        lines1 = f.readlines()
    with open(file2) as f:
        lines2 = f.readlines()
    return list(difflib.unified_diff(lines1, lines2, file1, file2))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--label", help="GN label for this test", required=True)
    parser.add_argument(
        "--source-root", help="Path to the Fuchsia source root", required=True
    )
    parser.add_argument(
        "--comparisons",
        metavar="FILE",
        help="Path at which to find the JSON file containing the comparisons",
        required=True,
    )
    parser.add_argument(
        "--depfile", help="Path at which to write the depfile", required=False
    )
    parser.add_argument(
        "--stamp-file",
        help="Path at which to write the stamp file",
        required=True,
    )
    parser.add_argument(
        "--bless",
        help="Overwrites the golden with the candidate if they don't match - or creates it if it does not yet exist",
        action="store_true",
    )
    parser.add_argument(
        "--warn",
        help="Whether API changes should only cause warnings",
        action="store_true",
    )
    parser.add_argument(
        "--err-msg",
        help="Additional error message to display if files don't match",
    )
    parser.add_argument(
        "--binary",
        help="Use binary comparison for the files.",
        action="store_true",
    )
    parser.add_argument(
        "--golden-dir",
        type=Path,
        help="Directory to clear of unchecked files.",
    )
    parser.add_argument(
        "--paths-relative-to-source-root",
        help="The paths being compared are relative to the Fuchsia source root rather than the Ninja output directory. This affects the manual update message.",
        action="store_true",
    )
    args = parser.parse_args()

    with open(args.comparisons) as f:
        comparisons = json.load(f)

    inputs = []
    manual_updates = []
    goldens = set()

    assert comparisons, "No comparisons were specified"
    for comparison in comparisons:
        # Unlike the candidate and formatted_golden, which are build directory
        # -relative paths, the golden is source-relative.
        golden = os.path.join(args.source_root, comparison["golden"])
        candidate = comparison["candidate"]
        inputs.extend([candidate, golden])
        goldens.add(Path(golden))

        # A formatted golden might have been supplied. Compare against that if
        # present. (In the case of a non-existent golden, this file is empty.)
        formatted_golden = comparison.get("formatted_golden")
        if formatted_golden:
            inputs.append(formatted_golden)

        diff_lines = []
        if os.path.exists(golden):
            if args.binary:
                current_comparison_failed = not filecmp.cmp(
                    candidate, formatted_golden or golden
                )
            else:
                diff_lines = get_diff_lines(
                    formatted_golden or golden, candidate
                )
                current_comparison_failed = bool(diff_lines)
        else:
            current_comparison_failed = True

        if current_comparison_failed:
            type = "Warning" if args.warn or args.bless else "Error"
            msg = f"\n{type}: "
            if args.err_msg:
                msg += args.err_msg
            else:
                msg += f"Golden file mismatch: `{golden}`"
                msg += f"\n\tCompared to: `{candidate}`)"

            if not os.path.exists(golden):
                msg += f"\n\tGolden file does not exist: `{golden}`"

            if diff_lines:
                max_diff_lines = 16
                if len(diff_lines) > max_diff_lines:
                    msg += f"\nDiff (-golden +actual, truncated):\n"
                else:
                    msg += f"\nDiff (-golden +actual):\n"
                msg += "".join(diff_lines[:max_diff_lines])

            print(msg)

            if args.bless:
                os.makedirs(os.path.dirname(golden), exist_ok=True)
                shutil.copyfile(candidate, golden)
            else:
                manual_updates.append(dict(golden=golden, candidate=candidate))

    dead_goldens: set[str] = set()
    if args.golden_dir:
        outside_goldens = sorted(
            {
                str(file)
                for file in goldens
                if not Path(file).is_relative_to(args.golden_dir)
            }
        )
        if outside_goldens:
            outside_goldens = "\n*** ".join(outside_goldens)
            sys.stderr.write(
                f"""
*** Some golden files are not within {args.golden_dir}:
*** {outside_goldens}
"""
            )
            return 2
        dir_files = set(
            file for file in args.golden_dir.rglob("*") if not file.is_dir()
        )
        dead_goldens = dir_files - goldens
        if dead_goldens and args.bless:
            subprocess.check_call(
                ["git", "rm", "-f", "--ignore-unmatch"] + sorted(dead_goldens)
            )
            dead_goldens = set()
            subprocess.check_call(["git", "add", args.golden_dir])

    # Print all of the manual update instructions once at the end to reduce the
    # amount of rebuilding and copy-pasting.
    if manual_updates or dead_goldens:
        print_failure_msg(
            manual_updates,
            list(dead_goldens),
            args.golden_dir,
            args.label,
            run_in_ninja_dir=not args.paths_relative_to_source_root,
        )
        if not args.warn:
            return 1

    with open(args.stamp_file, "w") as stamp_file:
        stamp_file.write("Golden!\n")

    if args.depfile:
        with open(args.depfile, "w") as depfile:
            depfile.write("%s: %s\n" % (args.stamp_file, " ".join(inputs)))

    return 0


if __name__ == "__main__":
    sys.exit(main())
