#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import os
import sys

""" A script for providing suggestions for fx if no commands are found.

This script is not intended to be run directly, rather it should be run as part
of the fx script when it cannot find a command to run. The script expects a list
of commands to be piped in which will be used as the list of commands that will
be checked. This is done to avoid argument limits but it will also lead to hangs
if the program is not called with piping stdin into the program.

The list of commands should be a newline separated list of either command names
or paths to commands that fx will exec.
"""

# This dictionary contains a list of known suggestions that have been moved or
# migrated to different tools. The key is the command that the user provided and
# the value is what the user will see instead.
WELL_KNOWN_SUGGESTIONS = {}


def calculate_dl_edit_distance(first: str, second: str) -> int:
    """Calculates the Damerau-Levenshtein distance between two strings.

    We want to use Damerau-Levenshtein since it will treat transposition as a
    single edit instead of 2 edits. This will help us better match simple typos
    like a user typing 'buidl' instead of 'build'.

    Note: we cannot use an external library here because `fx` runs before the
    build system can realize dependencies.

    Args:
      - first: the first string to compare
      - second: the second string to compare

    Returns:
        The edit distance between the two values.
    """
    len1, len2 = len(first), len(second)
    edits = {}

    # Fill out edits matrix
    for i in range(-1, len1 + 1):
        edits[(i, -1)] = i + 1
    for j in range(-1, len2 + 1):
        edits[(-1, j)] = j + 1

    for i in range(len1):
        for j in range(len2):
            edits[(i, j)] = min(
                edits[(i - 1, j)] + 1,  # deletion
                edits[(i, j - 1)] + 1,  # insertion
                edits[(i - 1, j - 1)]
                + (0 if first[i] == second[j] else 1),  # substitution
            )
            if (
                i > 0
                and j > 0
                and first[i] == second[j - 1]
                and first[i - 1] == second[j]
            ):
                edits[(i, j)] = min(
                    edits[(i, j)], edits[(i - 2, j - 2)] + 1
                )  # transposition

    return edits[(len1 - 1, len2 - 1)]


def closest_match(
    cmd: str, commands: list[str], max_distance: int = 1
) -> str | None:
    """Find the closest matching command

    Checks the list of commands and finds the closest match or None if no matches
    satisfy the max_distance.
    """
    closest_cmd = None
    min_dist = float("inf")

    for command in commands:
        if command == cmd:
            continue
        dist = calculate_dl_edit_distance(cmd, command)
        if dist <= max_distance and dist < min_dist:
            min_dist = dist
            closest_cmd = command

    return closest_cmd


def sanitize_commands(commands: str) -> list[str]:
    """Convert a string of commands to a list of commands"""
    # Remove ".fx" and use the basename of the command to match what fx is looking for.
    return [
        os.path.basename(c.removesuffix(".fx"))
        for c in commands.split()
        if c != ""
    ]


def main() -> None:
    commands = sanitize_commands(sys.stdin.read())
    cmd = sys.argv[1]

    suggestion: str | None = None

    if cmd in WELL_KNOWN_SUGGESTIONS:
        suggestion = WELL_KNOWN_SUGGESTIONS[cmd]
    else:
        match = closest_match(cmd, commands)
        if match:
            suggestion = "Command '{}' not found. Did you mean '{}'?".format(
                cmd, match
            )

    if suggestion:
        print(suggestion)


if __name__ == "__main__":
    main()
