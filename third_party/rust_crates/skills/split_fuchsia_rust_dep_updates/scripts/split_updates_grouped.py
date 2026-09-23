#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Script to split a Fuchsia Rust dependency update commit into grouped copies/moves.

Automatically discovers copies and moves between a base commit and target commit,
partitions them into multiple sequential copy/move commits according to a chosen
grouping strategy (--batch-size, --num-groups, or --groups-file), and finalizes
the actual update commit on top.
"""

import argparse
import os
import sys

# Ensure split_utils can be imported when running script from any directory.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import split_utils


def main() -> None:
    """Main entry point for splitting a Fuchsia Rust dependency update commit into groups."""
    parser = argparse.ArgumentParser(
        description="Split Fuchsia Rust dep updates into grouped copy/move commits"
    )
    parser.add_argument("--repo-path", required=True, help="Path to Fuchsia repo")
    parser.add_argument("--target-commit", default="HEAD", help="Commit to split")
    parser.add_argument("--base-commit", default="origin/main", help="Base commit")
    parser.add_argument(
        "--crates-per-group",
        "--batch-size",
        dest="batch_size",
        type=int,
        default=None,
        help="Number of crate copies/moves per commit group (default: 10 if no other grouping option specified)",
    )
    parser.add_argument(
        "--num-groups",
        type=int,
        default=None,
        help="Number of sequential commit groups to divide updates across",
    )
    parser.add_argument(
        "--groups-file",
        help="Path to a JSON configuration file defining custom crate groups",
    )
    args = parser.parse_args()

    repo_path = args.repo_path
    split_utils.verify_clean_repo(repo_path)

    target_hash = split_utils.resolve_commit(repo_path, args.target_commit)
    base_hash = split_utils.resolve_commit(repo_path, args.base_commit)

    print(f"Target commit: {target_hash}")
    print(f"Base commit: {base_hash}")

    operations = split_utils.discover_operations(repo_path, base_hash, target_hash)
    if not operations:
        print("No upgrades detected to split.")
        sys.exit(0)

    groups = split_utils.group_operations(
        operations,
        batch_size=args.batch_size,
        num_groups=args.num_groups,
        groups_file=args.groups_file,
    )

    if not groups:
        print("No operations grouped to split.")
        sys.exit(0)

    total_ops = sum(len(g) for g in groups)
    print(f"Detected {total_ops} copy/move operations across {len(groups)} group(s).")

    split_utils.run(f"git checkout {base_hash}", cwd=repo_path)

    try:
        commit_hashes = []
        for i, group in enumerate(groups):
            part_num = i + 1
            print(f"\nApplying Group {part_num}/{len(groups)} ({len(group)} crates)...")
            split_utils.apply_actions(repo_path, base_hash, group)

            msg = f"[rust-3p] Copy and move vendored crates (part {part_num}/{len(groups)})"
            split_utils.run(f'git commit -m "{msg}"', cwd=repo_path)
            commit_hashes.append(split_utils.resolve_commit(repo_path, "HEAD"))

        print("\nAll copies/moves committed.")

        final_hash = split_utils.finalize_actual_updates(
            repo_path, base_hash, target_hash, use_revert=False
        )

        print("\nSuccess!")
        for i, h in enumerate(commit_hashes):
            print(f"Commit C{i+1} (Group {i+1}): {h}")
        print(f"Commit A (actual updates): {final_hash}")
        print("You are now in detached HEAD at Commit A.")
        print("To update your branch, run: git checkout -B <branch_name>")

    except Exception as e:
        print(f"Error occurred: {e}")
        print("Attempting to restore original state...")
        split_utils.run(f"git checkout {target_hash}", cwd=repo_path)
        sys.exit(1)


if __name__ == "__main__":
    main()
