# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess
import sys
from pathlib import Path
from typing import Any


class Colors:
    GREEN = "\033[92m"
    YELLOW = "\033[93m"
    RED = "\033[91m"
    BLUE = "\033[94m"
    MAGENTA = "\033[95m"
    CYAN = "\033[96m"
    RESET = "\033[0m"
    BOLD = "\033[1m"


USE_COLORS = sys.stdout.isatty()


def colorize(text: str, color: str) -> str:
    if USE_COLORS:
        return f"{color}{text}{Colors.RESET}"
    return text


def format_col(text: str, width: int, color: str | None = None) -> str:
    padded = text
    if color:
        padded = colorize(text, color)
    padding = " " * max(0, width - len(text))
    return padded + padding


def run_jiri(
    jiri_root: Path, args: list[str], **kwargs: Any
) -> subprocess.CompletedProcess[Any]:
    """Runs a jiri command.

    Args:
        jiri_root: Path to the .jiri_root directory.
        args: List of arguments to pass to jiri.
        **kwargs: Additional arguments to pass to subprocess.run.

    Returns:
        The CompletedProcess object.
    """
    jiri_bin = jiri_root / "bin" / "jiri"
    if not jiri_bin.exists():
        jiri_bin = Path("jiri")

    print(f"Running jiri {' '.join(args)}...")
    return subprocess.run([str(jiri_bin)] + args, **kwargs)


def run_fx(
    worktree_path: Path, args: list[str], **kwargs: Any
) -> subprocess.CompletedProcess[Any]:
    """Runs an fx command in a specific worktree.

    Args:
        worktree_path: Path to the worktree.
        args: List of arguments to pass to fx.
        **kwargs: Additional arguments to pass to subprocess.run.

    Returns:
        The CompletedProcess object.
    """
    fx_bin = worktree_path / "scripts" / "fx"
    if not fx_bin.exists():
        raise FileNotFoundError(f"fx script not found at {fx_bin}")

    print(f"Running fx {' '.join(args)}...")
    # Ensure cwd is set to worktree_path if not specified
    if "cwd" not in kwargs:
        kwargs["cwd"] = str(worktree_path)
    return subprocess.run([str(fx_bin)] + args, **kwargs)


def run_git(
    repo_path: Path, args: list[str], quiet: bool = False, **kwargs: Any
) -> subprocess.CompletedProcess[Any]:
    """Runs a git command in a specific repository path.

    Args:
        repo_path: Path to the repository or worktree.
        args: List of arguments to pass to git.
        quiet: If True, do not print the command being run.
        **kwargs: Additional arguments to pass to subprocess.run.

    Returns:
        The CompletedProcess object.
    """
    if "cwd" not in kwargs:
        kwargs["cwd"] = str(repo_path)
    if not quiet:
        print(f"Running git {' '.join(args)} in {repo_path}...")
    return subprocess.run(["git"] + args, **kwargs)
