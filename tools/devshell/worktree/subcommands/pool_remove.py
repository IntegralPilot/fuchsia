# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any

from worktree_pool import WorktreePool


def run(args: Any, pool: WorktreePool) -> None:
    wt = pool.get_worktree_by_name(args.name)
    if wt.get_lease_info() and not args.force:
        raise RuntimeError(
            f"Worktree '{args.name}' is leased. Use --force to remove."
        )
    pool.remove_worktree(args.name, force=args.force)
