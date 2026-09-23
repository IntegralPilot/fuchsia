# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from worktree import Worktree, WorktreeState
from worktree_pool import WorktreePool
from worktree_printer import WorktreePrinter


def _format_title(wt: Worktree) -> str:
    if wt.get_state() == WorktreeState.LEASED:
        lease = wt.get_lease_info()
        if lease and lease.task_id:
            return f"{wt.name} [{lease.task_id}]"
    return wt.name


def run(pool: WorktreePool) -> None:
    WorktreePrinter.print_worktrees(
        pool.get_worktrees(), title_fn=_format_title
    )
