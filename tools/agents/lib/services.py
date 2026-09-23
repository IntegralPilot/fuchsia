# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Daemon service discovery and management for AI coding agents."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
from collections.abc import Sequence

from agents.lib import paths


def find_daemon_services(fuchsia_dir: pathlib.Path) -> list[str]:
    """Find all configured daemon service names across config directories."""
    found_services: list[str] = []
    for cfg_dir in paths.find_config_dirs(fuchsia_dir):
        services_file = cfg_dir / "services.txt"
        if services_file.is_file():
            with services_file.open("r", encoding="utf-8") as fh:
                for line in fh:
                    entry = line.strip()
                    if (
                        entry
                        and not entry.startswith("#")
                        and entry not in found_services
                    ):
                        found_services.append(entry)
    return found_services


def restart_daemons(
    service_names: Sequence[str], dry_run: bool = False
) -> None:
    """Try restarting active agent user daemons via systemd if installed."""
    if not service_names or not shutil.which("systemctl"):
        return

    for service_name in service_names:
        if dry_run:
            print(
                f"[DRY RUN] Would run: systemctl --user try-restart {service_name}"
            )
            continue

        subprocess.run(
            ["systemctl", "--user", "try-restart", service_name],
            capture_output=True,
            check=False,
        )
