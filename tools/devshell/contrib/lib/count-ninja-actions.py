# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import sys
from dataclasses import dataclass
from pathlib import Path

# For who updates ninja:
# Please make sure the `parse_log_into_builds` function is up to date with the
# latest .ninja_log format. Please only update the next line when that function
# is up to date.
NINJIA_LOG_VERSION = "# ninja log v7"
VERSION_MISMATCH_MESSAGE = """
If you are updating ninja, please make sure to also update
//tools/devshell/contrib/lib/count-ninja-actions.py
so that build analytics are reported correctly.
If you are not updating ninja but still seeing this error message, please file a bug at
https://issues.fuchsia.dev/issues/new?component=1477619
"""

FUCHSIA_BUILD_DIR = os.environ.get("FUCHSIA_BUILD_DIR")


@dataclass(frozen=True)
class LogEntry:
    start: int
    end: int
    mtime: int
    targets: list[str]
    command_hash: str

    @staticmethod
    def from_string(log_line: str) -> "LogEntry":
        tokens = log_line.split("\t")
        return LogEntry(
            int(tokens[0]),
            int(tokens[1]),
            int(tokens[2]),
            [tokens[3]],
            tokens[4],
        )


@dataclass(frozen=True)
class Build:
    actions: list[LogEntry]

    def action_count(self) -> int:
        return len(self.actions)


class UnsupportedNinjaLogVersion(Exception):
    pass


def validate_ninja_log_version(path: Path) -> None:
    if not os.path.isfile(path):
        return
    with open(path) as logfile:
        # Find the first non-empty line
        line = ""
        while line == "":
            line = next(logfile).strip()

        if line != NINJIA_LOG_VERSION:
            e = UnsupportedNinjaLogVersion(line)
            e.add_note(VERSION_MISMATCH_MESSAGE)
            raise e


def parse_log_into_builds(path: Path) -> list[Build]:
    entries: list[LogEntry] = []
    with open(path) as logfile:
        for n, line in enumerate(logfile):
            if line.startswith("#"):
                continue
            try:
                entry = LogEntry.from_string(line)
            except ValueError as e:
                e.add_note(f"parsing line: {n+1}")
                raise e
            entries.append(entry)

    builds: list[Build] = []
    current_build_actions: list[LogEntry] = []
    for entry in entries:
        if not current_build_actions:
            # current build is empty
            current_build_actions.append(entry)
        else:
            previous = current_build_actions[-1]
            if entry.command_hash == previous.command_hash:
                # this is another output for the same action
                previous.targets.extend(entry.targets)

            elif entry.end >= previous.end:
                # current entry is after previous, or the current build is empty
                # so add this line to the current build.
                current_build_actions.append(entry)

            else:
                # start a new build with this entry
                builds.append(Build(current_build_actions))
                current_build_actions = [entry]
    builds.append(Build(current_build_actions))

    return builds


def count_ninja_actions_last_build(path: Path) -> int:
    try:
        builds = parse_log_into_builds(path)
    except:
        return -2
    return builds[-1].action_count() if builds else 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Tool counting ninja actions of the last build."
    )
    parser.add_argument(
        "path",
        metavar="PATH",
        nargs="?",
        default=None,
        help="path to the ninja log file to parse",
    )
    parser.add_argument(
        "--validate-ninja-log-version",
        default=False,
        action="store_true",
        help="validate the ninja log version and exit",
    )

    args = parser.parse_args()

    if args.path:
        path = args.path
    elif FUCHSIA_BUILD_DIR is None:
        print("-1")
        return 0
    else:
        path = os.path.join(FUCHSIA_BUILD_DIR, ".ninja_log")

    if args.validate_ninja_log_version:
        validate_ninja_log_version(Path(path))
        return 0

    print(count_ninja_actions_last_build(Path(path)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
