# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess
import sys
import typing as T
from pathlib import Path


def git_cmd(git_dir: Path, args: T.Sequence[str | Path]) -> str:
    """Run git command in a given directory."""
    try:
        ret = subprocess.run(
            ["git", "-C", str(git_dir)] + [str(a) for a in args],
            check=True,
            text=True,
            capture_output=True,
        )
        return ret.stdout.strip()
    except subprocess.CalledProcessError as e:
        print(e.stdout)
        print(e.stderr, file=sys.stderr)
        raise e


def git_init(git_dir: Path, branch: str | None = None) -> None:
    """Initialize a git repository forcing files ref-format and configuring standard test settings."""
    git_dir.mkdir(parents=True, exist_ok=True)
    # Force the traditional files backend (`-c init.defaultRefFormat=files`) to ensure
    # that loose reference files are physically created on disk. This is necessary because
    # these unit tests assert the exact physical structure of the loose reference files
    # (expecting them to exist under `.git/refs/heads/...`). On hosts running newer Git
    # versions (2.45+) with experimental features enabled, `git init` defaults to the
    # new binary `reftable` backend where loose reference files are not created, which
    # breaks these assertions.
    init_args = ["-c", "init.defaultRefFormat=files"]
    if branch:
        init_args.extend(["-c", f"init.defaultBranch={branch}"])
    init_args.append("init")
    git_cmd(git_dir, init_args)

    # required to avoid errors on CI build bots when running this test.
    git_cmd(git_dir, ["config", "--local", "user.email", "test@example.com"])
    git_cmd(git_dir, ["config", "--local", "user.name", "Test User"])
