# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import shlex
import shutil
import subprocess
import sys
from typing import Any

from utils import run_fx
from worktree_pool import WorktreePool


def run(args: Any, pool: WorktreePool) -> None:
    wt = pool.add_worktree(args.name)

    symlink_local = getattr(args, "symlink_local", False)
    copy_local = getattr(args, "copy_local", False)

    if symlink_local or copy_local:
        src_local = pool.fuchsia_dir / "local"
        if not src_local.exists():
            print(
                f"Error: 'local' directory not found at {src_local}",
                file=sys.stderr,
            )
            sys.exit(1)

        dest_local = wt.path / "local"

        if symlink_local:
            try:
                dest_local.symlink_to(src_local)
                print(f"Symlinked {src_local} to {dest_local}")
            except Exception as e:
                print(f"Error creating symlink: {e}", file=sys.stderr)
                sys.exit(1)
        elif copy_local:
            try:
                print(f"Copying {src_local} to {dest_local}...")
                shutil.copytree(src_local, dest_local, symlinks=True)
                print("Copy complete.")
            except Exception as e:
                print(f"Error copying directory: {e}", file=sys.stderr)
                sys.exit(1)

    if args.set is not None:
        for s in args.set:
            set_args = shlex.split(s)
            try:
                run_fx(wt.path, ["set"] + set_args, check=True)
            except FileNotFoundError as e:
                print(f"Warning: {e}, cannot run fx set", file=sys.stderr)
            except subprocess.CalledProcessError as e:
                print(f"Failed to run fx set '{s}': {e}", file=sys.stderr)
                sys.exit(1)
