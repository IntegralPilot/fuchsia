#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
################################################################################
# WARNING: This script is currently not suitable for use in hermetic builds.   #
# It depends on the SELinux utility, `checkpolicy`, which is not available in  #
# the Fuchsia build.                                                           #
################################################################################
#
# Known limitations:
# - All output policies are `# handle_unknown deny`;
# - Conditionals are supported, assuming standard formatting where `if (...) {`
#   begins on the initial line and matching `}` closes the statement block.

import argparse
import os
import re
import subprocess
import sys
from collections.abc import Collection, Sequence

# Merged policy files must not attempt to declare new initial SIDs, which have a fixed set of
# values and ordering that forms part of the ABI, defined by the Reference Policy.
_INITIAL_SID_REGEX = "sid[ \t\v]+[^ \t\v]+"
_INITIAL_SID_PATTERN = re.compile(f"^{_INITIAL_SID_REGEX}$")

# Lines in policy files that match these patterns must be grouped together in
# the order the patterns appear.
_ORDERED_POLICY_STATEMENT_REGEXS = (
    "class[ \t\v]+[^ \t\v]+",
    _INITIAL_SID_REGEX,
    "common[ \t\v]+[^ \t\v]+.*",
    "class[ \t\v]+[^ \t\v]+[ \t\v]+.*",
    "default_(user|role|type|range)[ \t\v]+[^ \t\v]+.*",
    "sensitivity[ \t\v]+[^ \t\v]+.*",
    "dominance[ \t\v]+[^ \t\v]+.*",
    "category[ \t\v]+[^ \t\v]+.*",
    "level[ \t\v]+[^ \t\v]+.*",
    "mlsconstrain[ \t\v]+[^ \t\v]+.*",
    "policycap[ \t\v]+[^ \t\v]+.*",
    "attribute[ \t\v]+[^ \t\v]+.*",
    "bool[ \t\v]+[^ \t\v]+.*",
    "type[ \t\v]+[^ \t\v]+.*",
    "typealias[ \t\v]+[^ \t\v]+.*",
    "typeattribute[ \t\v]+[^ \t\v]+.*",
    "permissive[ \t\v]+[^ \t\v]+.*",
    "allow[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+.*",
    "neverallow[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+.*",
    "auditallow[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+.*",
    "dontaudit[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+.*",
    "allowxperm[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+[^ \t\v]+.*",
    "auditallowxperm[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+[^ \t\v]+.*",
    "dontauditxperm[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+[^ \t\v]+.*",
    "typebounds[ \t\v]+[^ \t\v]+.*",
    "type_transition[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+[^ \t\v]+",
    "type_member[ \t\v]+[^ \t\v]+.*",
    "type_change[ \t\v]+[^ \t\v]+.*",
    "type_transition[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v:]+([ \t\v]*:[ \t\v]*[^ \t\v]+)[ \t\v]+[^ \t\v]+[ \t\v]+.*",
    "range_transition[ \t\v]+[^ \t\v]+.*",
    r"if[ \t\v]*\(.*",
    "role[ \t\v]+[^ \t\v]+",
    "role[ \t\v]+[^ \t\v]+[ \t\v]+.*",
    "attribute_role[ \t\v]+[^ \t\v]+.*",
    "roleattribute[ \t\v]+[^ \t\v]+.*",
    "role_transition[ \t\v]+[^ \t\v]+.*$",
    "allow[ \t\v]+[^ \t\v]+[ \t\v]+[^ \t\v]+$",
    "user[ \t\v]+[^ \t\v]+.*$",
    "constrain[ \t\v]+[^ \t\v]+.*$",
    "sid[ \t\v]+[^ \t\v]+[ \t\v]+.*$",
    "fs_use_xattr[ \t\v]+[^ \t\v]+.*$",
    "fs_use_trans[ \t\v]+[^ \t\v]+.*$",
    "fs_use_task[ \t\v]+[^ \t\v]+.*$",
    "genfscon[ \t\v]+[^ \t\v]+.*$",
    "portcon[ \t\v]+[^ \t\v]+.*$",
)

_ORDERED_POLICY_STATEMENT_PATTERNS = tuple(
    (regex, re.compile(f"^{regex}$", re.DOTALL))
    for regex in _ORDERED_POLICY_STATEMENT_REGEXS
)

# Regular expression for empty/comment-only lines to check that all meaningful
# lines matched a pattern.
_WHITESPACE_OR_COMMENT_PATTERN = re.compile("^[ \t]*(#.*)?$")


def extract_statements(content: str) -> list[str]:
    """Parses text policy content into a list of atomic statement strings.
    Multi-line `if (...) { ... } [else { ... }]` blocks are preserved intact as single entries,
    while standard policy statements are split line by line.

    For simplicity, we assume specific formatting of `if` statements in the policy files:
    1. The initial brace `{` is in the same line as the `if` statement, and
    2. Any `else` blocks start with the closing brace of the previous block.
    """
    statements = []
    current_block: list[str] = []
    brace_count = 0

    for line in content.splitlines():
        # Strip comments to count braces accurately.
        stripped = line.split("#", 1)[0].strip()

        if current_block:
            current_block.append(line)
            brace_count += stripped.count("{") - stripped.count("}")
            if brace_count <= 0:
                statements.append("\n".join(current_block))
                current_block = []
                brace_count = 0
        elif re.match(r"^if\s*\(", stripped) and "{" in stripped:
            brace_count = stripped.count("{") - stripped.count("}")
            if brace_count > 0:
                current_block.append(line)
            else:
                statements.append(line)
        else:
            statements.append(line)

    if current_block:
        statements.append("\n".join(current_block))

    return statements


def _filter_lines(
    lines: Collection[str], pattern: re.Pattern[str]
) -> Sequence[str]:
    return tuple(line for line in lines if pattern.match(line) is not None)


def _negative_filter_lines(
    lines: Collection[str], pattern: re.Pattern[str]
) -> Sequence[str]:
    return tuple(line for line in lines if pattern.match(line) is None)


def compile_text_policy_to_binary_policy(
    checkpolicy_executable_path: str,
    input_file_path: str,
    output_file_path: str,
    handle_unknown: str = "deny",
) -> None:
    subprocess.run(
        [
            checkpolicy_executable_path,
            "--mls",  # Enable Multi-Level Security.
            "--sort",  # Sort ocontexts consistent with semanage behaviour.
            "--optimize",  # Optimize out redundant rules.
            "-c",
            "33",
            "--output",
            output_file_path,
            "--handle-unknown",
            handle_unknown,
            "-t",
            "selinux",
            input_file_path,
        ],
        check=True,
    )


def merge_text_policies(
    initial_sids_path: str,
    input_file_paths: list[str],
    output_file_path: str,
    handle_unknown: str = "deny",
) -> None:
    # Accumulate lines of input from all `input_file_paths` and sort them.
    unsorted_input_lines = set()
    for input_path in input_file_paths:
        with open(input_path, mode="rt") as input_file:
            unsorted_input_lines.update(extract_statements(input_file.read()))
    input_lines = sorted(unsorted_input_lines)

    # Validate that no input lines attempted to create additional initial SIDs.
    if len(_filter_lines(input_lines, _INITIAL_SID_PATTERN)) != 0:
        raise ValueError(f"Policy attempts to define new initial SIDS")

    # Fetch the initial SIDs defined by the Reference Policy.
    with open(initial_sids_path, mode="rt") as initial_sids_file:
        initial_sids_lines = initial_sids_file.read().splitlines()

    # Accumulate input lines grouped according to which statement pattern
    # they match. This step is required to ensure that `checkpolicy` will
    # compile the combined policy statements from all `input_file_paths`.
    policy_lines_from_input_files: list[str] = []
    for regex, matcher in _ORDERED_POLICY_STATEMENT_PATTERNS:
        matched_input_lines = _filter_lines(input_lines, matcher)

        if regex == _INITIAL_SID_REGEX:
            # Validate that no input lines attempted to create additional initial SIDs.
            if len(matched_input_lines) != 0:
                raise ValueError(f"Policy attempts to define new initial SIDS")

            # Put the SID definition lines in-place.
            matched_input_lines = _filter_lines(initial_sids_lines, matcher)

        policy_lines_from_input_files.extend(matched_input_lines)

    # Filter out empty or comment-only lines and count them. This will be
    # used to ensure that no meaningful lines are discarded when the policy
    # has been processed.
    expected_lines = frozenset(
        _negative_filter_lines(
            input_lines + initial_sids_lines, _WHITESPACE_OR_COMMENT_PATTERN
        )
    )
    expected_num_lines = len(expected_lines)

    # Ensure that no meaningful policy statements were discarded.
    actual_num_lines = len(policy_lines_from_input_files)
    if actual_num_lines != expected_num_lines:
        # If some statements were discarded, emit a diff to stderr and
        # `raise`.
        text_policy_set = frozenset(policy_lines_from_input_files)
        for found_but_unexpected_line in text_policy_set - expected_lines:
            print(f"< {found_but_unexpected_line}", file=sys.stderr)
        for expected_but_not_found_line in expected_lines - text_policy_set:
            print(f"> {expected_but_not_found_line}", file=sys.stderr)

        raise ValueError(
            f"Expected policy with {expected_num_lines} from {input_file_paths}, but filtered policy contains {actual_num_lines} lines"
        )

    with open(output_file_path, mode="wt") as output_file:
        # All policies must begin with a `# handle_unknown ...` clause. Tests
        # usually implement "default deny" and allow what is necessary.
        output_file.write(f"# handle_unknown {handle_unknown}\n")

        for line in policy_lines_from_input_files:
            output_file.write(f"{line}\n")


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Merge SELinux policy fragments."
    )
    parser.add_argument(
        "--initial-sids", required=True, help="Path to initial_sids file"
    )
    parser.add_argument(
        "--fragments-manifest",
        help="Path to manifest file containing list of fragment paths (one per line)",
    )
    parser.add_argument(
        "--inputs",
        nargs="*",
        default=[],
        help="Direct policy fragment files",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Path to write the merged policy .conf",
    )
    parser.add_argument(
        "--handle-unknown",
        default="deny",
        choices=["allow", "deny", "reject"],
        help="handle_unknown setting",
    )
    parser.add_argument("--depfile", help="Path to write Ninja depfile")
    return parser.parse_args(argv)


def main(argv: Sequence[str]) -> int:
    args = parse_args(argv)

    fragments = list(args.inputs)
    if args.fragments_manifest is not None:
        with open(args.fragments_manifest, mode="rt", encoding="utf-8") as f:
            for line in f:
                path = line.strip()
                if path:
                    fragments.append(path)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    merge_text_policies(
        args.initial_sids,
        fragments,
        args.output,
        handle_unknown=args.handle_unknown,
    )

    if args.depfile:
        os.makedirs(
            os.path.dirname(os.path.abspath(args.depfile)), exist_ok=True
        )
        all_deps = [args.initial_sids] + sorted(set(fragments))
        with open(args.depfile, mode="wt", encoding="utf-8") as f:
            f.write(f"{args.output}: {' '.join(all_deps)}\n")

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
