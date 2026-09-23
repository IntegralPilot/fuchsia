# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any

from worktree import Worktree, WorktreeState
from worktree_pool import WorktreePool
from worktree_printer import WorktreePrinter


def _format_leased_title(wt: Worktree) -> str:
    lease = wt.get_lease_info()
    if lease and lease.task_id:
        return lease.task_id
    return wt.name


def run(args: Any, pool: WorktreePool) -> None:
    leased_wts = [
        wt
        for wt in pool.get_worktrees()
        if wt.get_state() == WorktreeState.LEASED
    ]
    WorktreePrinter.print_worktrees(leased_wts, title_fn=_format_leased_title)
