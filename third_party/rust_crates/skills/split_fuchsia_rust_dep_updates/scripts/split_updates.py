#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Script to split a Fuchsia Rust dependency update commit.

It splits the target commit into:
1. A commit with pure copies/moves of vendored crates.
2. A commit with the actual content changes and migrations.
"""

import argparse
import os
import subprocess
import sys

# Ensure split_utils can be imported when running script from any directory.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import split_utils


def main() -> None:
    """Main entry point for splitting a Fuchsia Rust dependency update commit."""
    parser = argparse.ArgumentParser(description="Split Fuchsia Rust dep updates")
    parser.add_argument("--repo-path", required=True, help="Path to Fuchsia repo")
    parser.add_argument("--target-commit", default="HEAD", help="Commit to split")
    parser.add_argument("--base-commit", default="origin/main", help="Base commit")
    args = parser.parse_args()

    repo_path = args.repo_path
    split_utils.verify_clean_repo(repo_path)

    target_hash = split_utils.resolve_commit(repo_path, args.target_commit)
    base_hash = split_utils.resolve_commit(repo_path, args.base_commit)

    print(f"Target commit: {target_hash}")
    print(f"Base commit: {base_hash}")

    copies, moves = split_utils.discover_copies_and_moves(
        repo_path, base_hash, target_hash
    )

    if not copies and not moves:
        print("No upgrades detected to split.")
        sys.exit(0)

    print(f"Detected {len(copies)} copies and {len(moves)} moves.")

    split_utils.run(f"git checkout {base_hash}", cwd=repo_path)

    try:
        split_utils.apply_copies(repo_path, copies, base_ref=base_hash)
        split_utils.apply_moves(repo_path, base_hash, moves)

        msg = split_utils.format_copy_move_commit_message(base_hash, copies, moves)
        proc = subprocess.run(
            ["git", "commit", "-F", "-"],
            input=msg,
            text=True,
            check=True,
            cwd=repo_path,
        )
        commit1_hash = split_utils.resolve_commit(repo_path, "HEAD")

        commit2_hash = split_utils.finalize_actual_updates(
            repo_path, base_hash, target_hash, use_revert=True
        )

        print("\nSuccess!")
        print(f"Commit 1 (copies/moves): {commit1_hash}")
        print(f"Commit 2 (actual updates): {commit2_hash}")
        print("You are now in detached HEAD at Commit 2.")
        print("To update your branch, run: git checkout -B <branch_name>")

    except Exception as e:
        print(f"Error occurred: {e}")
        print("Attempting to restore original state...")
        split_utils.run(f"git checkout {target_hash}", cwd=repo_path)
        sys.exit(1)


if __name__ == "__main__":
    main()
