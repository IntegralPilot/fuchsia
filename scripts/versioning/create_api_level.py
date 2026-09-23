#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Updates the Fuchsia platform version.
"""

import argparse
import fileinput
import glob
import json
import os
import pathlib
import secrets
import shutil
import sys


def _generate_random_abi_revision(already_used: set[int]) -> int:
    """Generates a random 64-bit ABI revision, avoiding values in
    `already_used` and other reserved ranges."""
    while True:
        # Reserve values in the top 1/256th of the space.
        candidate = secrets.randbelow(0xF000_0000_0000_0000)

        # If the ABI revision has already been used, discard it and go buy a
        # lottery ticket.
        if candidate not in already_used:
            return candidate


def update_version_history(
    new_api_level: int, version_history_path: str
) -> bool:
    """Updates version_history.json to include the given Fuchsia API level.

    The ABI revision for this API level is set to a new random value that has not
    been used before.
    """
    try:
        with open(version_history_path, "r+") as f:
            version_history = json.load(f)
            versions = version_history["data"]["api_levels"]
            if str(new_api_level) in versions:
                print(
                    f"error: Fuchsia API level {new_api_level} is already defined.",
                    file=sys.stderr,
                )
                return False

            abi_revision = _generate_random_abi_revision(
                set(int(v["abi_revision"], 16) for v in versions.values())
            )
            versions[str(new_api_level)] = dict(
                # Print `abi_revision` in hex, with a leading 0x, with capital
                # letters, padded to 16 chars.
                abi_revision=f"0x{abi_revision:016X}",
                phase="supported",
            )
            f.seek(0)
            json.dump(version_history, f, indent=4)
            # JSON dump does not include a new line at the end.
            f.write("\n")
            f.truncate()
            return True
    except FileNotFoundError:
        print(
            f"""error: Unable to open '{version_history_path}'.
Did you run this script from the root of the source tree?""",
            file=sys.stderr,
        )
        return False


def _create_owners_file(level_dir_path: str) -> None:
    """Creates an OWNERS file in `level_dir_path` with limited approvers."""
    owners_path = os.path.join(level_dir_path, "OWNERS")

    print(f"Creating {owners_path}")
    with open(owners_path, "w") as f:
        f.write("include /sdk/history/FROZEN_API_LEVEL_OWNERS\n")


def _update_availability_levels(
    new_api_level: int, availability_levels_file: str
) -> bool:
    """Adds the given Fuchsia API level to the C preprocessor defines file.

    Assumes lines are sorted in decreasing order of API level.
    """
    # Look for the most recent line and replace it with the line for the new
    # level and itself.
    last_api_level = new_api_level - 1
    line_format = "#define FUCHSIA_INTERNAL_LEVEL_{level}_() {level}"
    most_recent_api_level_line = line_format.format(level=last_api_level)
    new_line = line_format.format(level=new_api_level)

    try:
        with fileinput.FileInput(availability_levels_file, inplace=True) as f:
            for line in f:
                print(
                    line.replace(
                        most_recent_api_level_line,
                        f"{new_line}\n{most_recent_api_level_line}",
                    ),
                    end="",
                )
        return True
    except FileNotFoundError:
        print(
            f"""error: Unable to open '{availability_levels_file}'.
Did you run this script from the root of the source tree?""",
            file=sys.stderr,
        )
        return False


def create_sdk_history(root_source_dir: str, new_level: int) -> None:
    level_dir_path = os.path.join(
        root_source_dir, "sdk", "history", str(new_level)
    )

    try:
        os.makedirs(level_dir_path, exist_ok=True)
    except Exception as e:
        print(f"Failed to create directory for new level: {e}")
        sys.exit(1)

    _create_owners_file(level_dir_path)

    next_dir_path = os.path.join(root_source_dir, "sdk", "history", "NEXT")

    # The BUILD.bazel file just exposes the golden files in the directory to
    # Bazel targets, so the same file can be used for every level.
    shutil.copy(
        os.path.join(next_dir_path, "BUILD.bazel"),
        os.path.join(level_dir_path, "BUILD.bazel"),
    )

    ifs_files = glob.glob(
        "*.ifs",
        root_dir=next_dir_path,
    )

    # The .ifs files cannot be generated from scratch because they are also used as source files.
    # This creates empty files based on the contents of NEXT allowing the build to succeed and write the files with the correct contents.
    for ifs_file in ifs_files:
        # make an empty file
        with open(os.path.join(level_dir_path, ifs_file), "w"):
            pass


SDK_VERSION_HISTORY_PATH = "sdk/version_history.json"
AVAILABILITY_LEVELS_INC_PATH = (
    "zircon/system/public/zircon/availability_levels.inc"
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--new-api-level", type=int, required=True)
    parser.add_argument("--root-source-dir", type=pathlib.Path, required=True)

    args = parser.parse_args()

    new_level = args.new_api_level

    if not update_version_history(
        new_level, args.root_source_dir.joinpath(SDK_VERSION_HISTORY_PATH)
    ):
        return 1

    create_sdk_history(args.root_source_dir, new_level)

    # TODO(https://fxbug.dev/349622444): Enable building with
    # `FUCHSIA_INTERNAL_LEVEL_NEXT_()` undefined to ensure there are no stray
    # instances of `NEXT` that have not been converted to the new level.
    # Otherwise, perhaps use a preprocessor condition. This would avoid the need
    # to regenerate `availability_levels_file_path` twice but would include a
    # preprocessor condition in the production code. Do the same for the Rust
    # macros.
    if not _update_availability_levels(
        new_level, args.root_source_dir.joinpath(AVAILABILITY_LEVELS_INC_PATH)
    ):
        return 1

    print(
        f"""
API level {new_level} has been added.
"""
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
