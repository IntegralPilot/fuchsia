# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any

from worktree_pool import WorktreePool


def run(args: Any, pool: WorktreePool) -> None:
    wt = pool.find_worktree(args.name)
    print(wt.path)
