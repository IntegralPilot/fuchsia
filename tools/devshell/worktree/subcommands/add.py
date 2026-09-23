# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import subprocess
import sys
from typing import Any

from utils import run_jiri
from worktree import NoFreeWorktreesError, WorktreeState
from worktree_pool import WorktreePool


def run(args: Any, pool: WorktreePool) -> None:
    task_name = getattr(args, "name", None)
    pool_name = getattr(args, "pool_name", None)

    if not task_name:
        print("Error: Must specify a worktree name", file=sys.stderr)
        sys.exit(1)

    for wt in pool.get_worktrees():
        if wt.get_state() == WorktreeState.LEASED:
            lease = wt.get_lease_info()
            if lease and lease.task_id == task_name:
                raise ValueError(
                    f"Task '{task_name}' is already active (leased by '{wt.name}')"
                )

    try:
        if pool_name:
            wt = pool.get_worktree_by_name(pool_name)
            wt.acquire_lease(task_id=task_name)
        else:
            wt = pool.get_any_free_worktree()
            wt.acquire_lease(task_id=task_name)
    except NoFreeWorktreesError:
        print(
            "No free worktrees available in the pool. Provisioning a new one...",
            file=sys.stderr,
        )
        try:
            wt = pool.add_worktree()
            wt.acquire_lease(task_id=task_name)
        except Exception as e:
            print(
                f"Error: Failed to auto-provision worktree: {e}",
                file=sys.stderr,
            )
            sys.exit(1)

    if getattr(args, "sync", False):
        try:
            run_jiri(
                pool.jiri_root,
                ["worktree", "sync", str(wt.path)],
                check=True,
            )
        except subprocess.CalledProcessError as e:
            print(f"Failed to sync worktree: {e}", file=sys.stderr)
            wt.release_lease()
            sys.exit(1)

    symlink_path = pool.worktrees_dir / task_name
    if symlink_path != wt.path:
        if symlink_path.is_symlink() or symlink_path.exists():
            symlink_path.unlink()
        symlink_path.symlink_to(wt.name)

    if getattr(args, "json", False):
        print(
            json.dumps(
                {
                    "worktree_id": task_name,
                    "path": str(symlink_path),
                }
            )
        )
    else:
        print(
            f"Successfully created worktree '.jiri_root/worktrees/{task_name}'"
        )
