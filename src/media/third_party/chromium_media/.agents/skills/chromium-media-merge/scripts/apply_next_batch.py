#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Applies Chromium media commits incrementally from todo_commits.txt."""

import argparse
import os
import sys

import merge_helpers


def main():
    parser = argparse.ArgumentParser(
        description="Apply Chromium media commits incrementally or resolve state."
    )
    parser.add_argument(
        "-n",
        "--limit",
        type=int,
        default=None,
        help="Optional limit on number of commits to apply in this run",
    )
    parser.add_argument(
        "-c",
        "--continue",
        dest="continue_sha",
        type=str,
        help="Commit manually resolved SHA and update state files",
    )
    parser.add_argument(
        "-s",
        "--skip",
        dest="skip_sha",
        type=str,
        help="Skip SHA, revert uncommitted changes, and update state files",
    )
    parser.add_argument(
        "--reason",
        type=str,
        default="skipped",
        help="Reason for skipping commit when using --skip",
    )
    parser.add_argument(
        "--stop",
        action="store_true",
        help="Stop after --continue or --skip without processing subsequent commits",
    )
    args = parser.parse_args()

    if args.continue_sha:
        print(f"=== Continuing after manual resolution for commit: {args.continue_sha} ===")
        merge_helpers.commit_and_update(args.continue_sha)
        print("=== Successfully committed and updated state. ===")
        if args.stop:
            return

    if args.skip_sha:
        print(f"=== Skipping commit: {args.skip_sha} ({args.reason}) ===")
        merge_helpers.skip_commit(args.skip_sha, reason=args.reason)
        print("=== Successfully skipped and updated state. ===")
        if args.stop:
            return

    if not os.path.exists(merge_helpers.TODO_FILE):
        print(f"Error: {merge_helpers.TODO_FILE} not found.")
        sys.exit(1)

    with open(merge_helpers.TODO_FILE, "r") as f:
        todo = [line.strip() for line in f if line.strip()]

    if not todo:
        print("No remaining commits in todo_commits.txt!")
        return

    applied_count = 0
    max_count = args.limit if args.limit is not None else len(todo)
    for sha_entry in todo[:max_count]:
        sha = sha_entry.split()[0]
        print(f"\n--- [Commit {applied_count + 1}/{max_count}] Processing SHA: {sha} ---")
        msg = merge_helpers.get_commit_msg(sha)
        subject = msg.splitlines()[0] if msg else sha
        print(f"Subject: {subject}")

        patch_cmd = (
            [
                "git",
                "-C",
                merge_helpers.CHROMIUM_REPO_MEDIA,
                "format-patch",
                "-1",
                sha,
                "--stdout",
                "--",
            ]
            + merge_helpers.MAINTAINED_FILES
        )
        res_patch = merge_helpers.run_cmd(patch_cmd)
        patch_content = res_patch.stdout

        if not patch_content.strip():
            print(f"  [SKIP] Commit {sha} produced no diff on maintained files.")
            merge_helpers.update_lists(sha)
            continue

        patch_file = os.path.join(merge_helpers.STATE_DIR, "current_patch.diff")
        with open(patch_file, "w") as f:
            f.write(patch_content)

        # 1. Try git apply
        apply_cmd = [
            "git",
            "apply",
            f"--directory={merge_helpers.FUCHSIA_CHROMIUM_MEDIA_DIR}",
            patch_file,
        ]
        res_apply = merge_helpers.run_cmd(apply_cmd, check=False)

        if res_apply.returncode == 0:
            print("  [SUCCESS] git apply succeeded cleanly.")
            merge_helpers.commit_and_update(sha, msg)
            applied_count += 1
            continue

        print("  [INFO] git apply rejected. Attempting 3-way merge-file fallback...")

        # 2. Identify modified files in this commit
        diff_tree_cmd = (
            [
                "git",
                "-C",
                merge_helpers.CHROMIUM_REPO_MEDIA,
                "diff-tree",
                "--no-commit-id",
                "--name-only",
                "-r",
                sha,
                "--",
            ]
            + merge_helpers.MAINTAINED_FILES
        )
        res_files = merge_helpers.run_cmd(diff_tree_cmd)
        modified_files = [f for f in res_files.stdout.splitlines() if f.strip()]

        has_conflict = False
        conflicted_files = []

        for rel_path in modified_files:
            chromium_rel_path = rel_path
            media_rel_path = (
                rel_path[len("media/") :]
                if rel_path.startswith("media/")
                else rel_path
            )
            fuchsia_path = os.path.join(
                merge_helpers.FUCHSIA_MEDIA_PREFIX, media_rel_path
            )
            fuchsia_abs_path = os.path.join(
                merge_helpers.FUCHSIA_REPO, fuchsia_path
            )
            base_file = os.path.join(merge_helpers.STATE_DIR, "base.tmp")
            theirs_file = os.path.join(merge_helpers.STATE_DIR, "theirs.tmp")

            res_base = merge_helpers.run_cmd(
                ["git", "-C", merge_helpers.CHROMIUM_REPO_MEDIA, "show", f"{sha}^:{chromium_rel_path}"],
                check=False,
            )
            res_theirs = merge_helpers.run_cmd(
                ["git", "-C", merge_helpers.CHROMIUM_REPO_MEDIA, "show", f"{sha}:{chromium_rel_path}"],
                check=False,
            )

            if res_base.returncode != 0 or res_theirs.returncode != 0:
                print(
                    f"  [CONFLICT] File add/delete detected for {chromium_rel_path}. Stopping for manual inspection."
                )
                has_conflict = True
                conflicted_files.append(chromium_rel_path)
                break

            with open(base_file, "w") as f:
                f.write(res_base.stdout)
            with open(theirs_file, "w") as f:
                f.write(res_theirs.stdout)

            # git merge-file -p <ours> <base> <theirs>
            merge_cmd = [
                "git",
                "merge-file",
                "-p",
                fuchsia_path,
                base_file,
                theirs_file,
            ]
            res_merge = merge_helpers.run_cmd(merge_cmd, check=False)

            # Copy merged content back to fuchsia_path
            with open(fuchsia_abs_path, "w") as f:
                f.write(res_merge.stdout)

            if res_merge.returncode == 0:
                print(f"    [3-WAY CLEAN] {rel_path}")
            else:
                print(f"    [3-WAY CONFLICT] {rel_path} has unresolved conflict markers!")
                has_conflict = True
                conflicted_files.append(rel_path)

        if has_conflict:
            print("\n=======================================================")
            print(f"STOPPED at SHA: {sha} ({subject})")
            print(f"Conflicts detected in: {', '.join(conflicted_files)}")
            print("Please inspect and resolve conflict markers in the file(s), then commit and update state manually.")
            print("=======================================================")
            sys.exit(2)

        print("  [SUCCESS] All files merged cleanly via 3-way merge!")
        merge_helpers.commit_and_update(sha, msg)
        applied_count += 1

    print(f"\nRun complete: successfully applied {applied_count} commits.")


if __name__ == "__main__":
    main()
