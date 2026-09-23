#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Shared helper functions and constants for Chromium media merge workflow."""

import os
import subprocess
import sys


def get_fuchsia_repo_root():
    try:
        res = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=os.path.dirname(os.path.abspath(__file__)),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=True,
        )
        return res.stdout.strip()
    except Exception:
        res = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=os.getcwd(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=True,
        )
        return res.stdout.strip()


FUCHSIA_REPO = os.environ.get("FUCHSIA_REPO", get_fuchsia_repo_root())
_raw_chromium = os.environ.get(
    "CHROMIUM_REPO_MEDIA",
    os.environ.get(
        "CHROMIUM_REPO",
        os.path.join(FUCHSIA_REPO, "local_src/chromium/src/media"),
    ),
)
if (
    os.path.exists(os.path.join(_raw_chromium, "media"))
    and not _raw_chromium.rstrip("/").endswith("media")
):
    CHROMIUM_REPO_MEDIA = os.path.join(_raw_chromium, "media")
else:
    CHROMIUM_REPO_MEDIA = _raw_chromium

STATE_DIR = os.path.join(
    FUCHSIA_REPO, "src/media/third_party/chromium_media/.merge_state"
)
TODO_FILE = os.path.join(STATE_DIR, "todo_commits.txt")
COMPLETED_FILE = os.path.join(STATE_DIR, "completed.log")
FUCHSIA_CHROMIUM_MEDIA_DIR = "src/media/third_party/chromium_media"
FUCHSIA_MEDIA_PREFIX = "src/media/third_party/chromium_media/media"

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
MAINTAINED_FILES_PATH = os.path.join(SCRIPT_DIR, "maintained_files.txt")


def load_maintained_files():
    if os.path.exists(MAINTAINED_FILES_PATH):
        with open(MAINTAINED_FILES_PATH, "r") as f:
            return [line.strip() for line in f if line.strip()]
    return []


MAINTAINED_FILES = load_maintained_files()


def run_cmd(cmd, cwd=FUCHSIA_REPO, check=True):
    return subprocess.run(
        cmd,
        cwd=cwd,
        check=check,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def get_commit_msg(sha):
    res = run_cmd(
        ["git", "-C", CHROMIUM_REPO_MEDIA, "log", "-1", "--format=%s%n%n%b", sha]
    )
    return res.stdout.strip()


def update_lists(sha, reason=None):
    if os.path.exists(TODO_FILE):
        with open(TODO_FILE, "r") as f:
            todo = [line.strip() for line in f if line.strip()]
        todo = [
            item
            for item in todo
            if not (
                item.split()[0].startswith(sha)
                or sha.startswith(item.split()[0])
            )
        ]

        with open(TODO_FILE, "w") as f:
            for item in todo:
                f.write(item + "\n")

    entry = f"{sha} ({reason})" if reason else sha
    with open(COMPLETED_FILE, "a") as f:
        f.write(entry + "\n")


def commit_and_update(sha, full_msg=None):
    if not full_msg:
        full_msg = get_commit_msg(sha)
    run_cmd(["git", "add", "-u", FUCHSIA_CHROMIUM_MEDIA_DIR])
    run_cmd(["git", "add", FUCHSIA_MEDIA_PREFIX])
    res_diff = run_cmd(
        ["git", "diff", "--cached", "--quiet", "--", FUCHSIA_CHROMIUM_MEDIA_DIR],
        check=False,
    )
    if res_diff.returncode == 0:
        print(
            f"  [SKIP COMMIT] Commit {sha} produced 0 net changes after merge."
        )
        update_lists(sha, reason="empty commit after merge")
        return
    lines = full_msg.splitlines() if full_msg else [sha]
    subject = lines[0]
    body = "\n".join(lines[1:]).strip() if len(lines) > 1 else ""
    commit_args = ["git", "commit", "--no-status", "-m", subject]
    if body:
        commit_args.extend(["-m", body])
    commit_args.extend(["-m", f"[chromium_media] Upstream commit {sha}"])
    run_cmd(commit_args)
    print(f"  [COMMITTED] Successfully committed SHA: {sha}")
    update_lists(sha)


def skip_commit(sha, reason="skipped", revert_changes=True):
    if revert_changes:
        run_cmd(["git", "checkout", "--", FUCHSIA_MEDIA_PREFIX], check=False)
        for f in os.listdir(STATE_DIR):
            if f.endswith(".tmp") or f.endswith(".diff"):
                try:
                    os.remove(os.path.join(STATE_DIR, f))
                except OSError:
                    pass
    entry_reason = (
        f"skipped - {reason}" if reason and reason != "skipped" else "skipped"
    )
    update_lists(sha, reason=entry_reason)
    print(f"  [SKIPPED] Commit {sha} skipped ({reason}).")
