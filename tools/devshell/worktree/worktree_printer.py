# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Callable, Iterable

from utils import Colors, colorize
from worktree import SyncStatus, Worktree


def format_time_ago(sec: float | None) -> str:
    if sec is None:
        return colorize("never built", Colors.RED)
    if sec < 60:
        return "just now"
    minutes = int(sec) // 60
    hours = minutes // 60
    days = hours // 24
    if days > 0:
        return f"{days}d ago"
    if hours > 0:
        return f"{hours}h ago"
    return f"{minutes}m ago"


class WorktreePrinter:
    """Formats and displays Worktree information in structured terminal output.

    Provides helper methods to render worktree status, lease details, build timestamps,
    and sync health across list and pool subcommands.
    """

    @staticmethod
    def print_worktrees(
        worktrees: Iterable[Worktree],
        title_fn: Callable[[Worktree], str] | None = None,
    ) -> None:
        wt_list = list(worktrees)
        if not wt_list:
            return

        max_left_len = 0
        wt_entries = []

        for wt in wt_list:
            display_name = title_fn(wt) if title_fn else wt.name
            parts = []

            sync_status, behind, new = wt.get_sync_status()
            if (
                sync_status != SyncStatus.SYNCED
                and sync_status != SyncStatus.UNKNOWN
            ):
                sync_parts = []
                if behind > 0:
                    sync_parts.append(colorize(f"{behind} behind", Colors.RED))
                if new > 0:
                    sync_parts.append(colorize(f"{new} new", Colors.BLUE))
                if sync_parts:
                    parts.append(", ".join(sync_parts))

            build_entries = []
            try:
                selected_dir = wt.selected_build_dir()
            except (FileNotFoundError, ValueError, OSError):
                selected_dir = None

            for bd in wt.build_dirs():
                try:
                    out_rel = str(bd.path.relative_to(wt.path))
                except ValueError:
                    out_rel = str(bd.path)
                is_active = selected_dir is not None and bd.path == selected_dir
                suffix = " *" if is_active else ""
                label = f"{out_rel}{suffix}:"
                max_left_len = max(max_left_len, 4 + len(label))

                cfg = bd.get_build_config()
                time_str = format_time_ago(bd.get_build_time_ago_sec())
                build_entries.append((label, cfg, time_str, is_active))

            wt_entries.append((display_name, parts, build_entries))

        wt_entries.sort(key=lambda item: item[0])
        left_width = max_left_len + 4

        for name, parts, builds in wt_entries:
            header = colorize(name, Colors.BOLD)
            if parts:
                header = f"{header} ({', '.join(parts)})"
            print(header)
            for i, (label, cfg, time_str, is_active) in enumerate(builds):
                is_last = i == len(builds) - 1
                prefix = "└── " if is_last else "├── "
                left_part = f"{prefix}{label}"
                line = f"{left_part:<{left_width}}{cfg} ({time_str})"
                if is_active:
                    line = colorize(line, Colors.GREEN)
                print(line)
            if builds:
                print()
