#!/usr/bin/env python3
# allow-non-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Preflight checks for cog scripts."""

import shutil
import subprocess
import sys

import cartfs
import logger
import workspace


def check_gcert_status() -> bool:
    """Checks if the user has a valid gcert certificate."""
    try:
        subprocess.check_call(["gcertstatus", "-check_ssh=false", "-quiet"])
        logger.log_debug("Gcert certificate is valid.")
        return True
    except (subprocess.CalledProcessError, FileNotFoundError):
        logger.log_error("You do not have a valid gcert certificate.")
        logger.log_error("Please run 'gcert' and try again.")
        return False


def check_git_citc_cogd() -> bool:
    """Checks if the user has git + git-citc installed and if the cwd is within a Cog workspace."""
    if shutil.which("git") is None:
        logger.log_error("Expected `git` to be in the PATH.")
        logger.log_error("Please run `sudo apt install git` and try again.")
        return False

    if shutil.which("git-citc") is None:
        logger.log_error("Expected `git-citc` to be in the PATH.")
        logger.log_error("Please run 'sudo apt install cogfsd' and try again.")
        return False

    try:
        workspace.Workspace.cogd_path()
        logger.log_debug("Running in a Cog workspace.")
        return True
    except workspace.NotInCogWorkspaceError:
        logger.log_error(
            "Expected the current directory to be within a Cog workspace."
        )
        logger.log_error(
            "Please run `cogd <workspace>` to enter a workspace and try again."
        )
        logger.log_error(
            "Use `git citc list` to list available workspaces or "
            "`git citc create <workspace> turquoise-internal/fuchsia-cog-superproject` "
            "to create a new workspace."
        )
        return False


def check_cartfs(require_grpc_cli: bool = True) -> bool:
    """Checks if cartfs is installed and running."""
    if shutil.which("cartfs") is None:
        logger.log_error("Expected `cartfs` to be in the PATH.")
        logger.log_error("Please run `sudo apt install cartfs` and try again.")
        return False

    try:
        cartfs.Cartfs.cartfs_uid()
        logger.log_debug("Cartfs uid found.")
    except cartfs.CartfsNotRunningError:
        logger.log_warn("Unable to find the uid for cartfs.")
        logger.log_error(
            "Please run `sudo apt install cartfs` and "
            "start cartfs with `systemctl start cartfs.service` and try again."
        )
        return False

    try:
        cartfs.Cartfs.find_mount_point()
        logger.log_debug("Cartfs mount point found.")
    except cartfs.CartfsNotRunningError:
        logger.log_warn("Unable to find the mount point for cartfs.")
        logger.log_error(
            "Please start cartfs with `systemctl start cartfs.service` and try again."
        )
        return False

    # Used for snapshotting.
    if require_grpc_cli and shutil.which("grpc_cli") is None:
        logger.log_error("Expected `grpc_cli` to be in the PATH.")
        logger.log_error(
            "Please run `sudo apt install grpc-cli` and try again."
        )
        return False

    logger.log_debug("Cartfs is installed and running.")

    return True


def _check_all(require_grpc_cli: bool) -> bool:
    # Skip Cog and CartFS checks if gcert is not available, since those checks would otherwise
    # surface irrelevant errors.
    if not check_gcert_status():
        return False

    cog_ok = check_git_citc_cogd()
    cartfs_ok = check_cartfs(require_grpc_cli)
    return cog_ok and cartfs_ok


def check_all(require_grpc_cli: bool = True) -> bool:
    """Runs all available preflight checks."""
    result = _check_all(require_grpc_cli)
    if not result:
        logger.log_error(
            "Refer to http://go/fuchsia-cog-user-guide for additional "
            "instructions on fuchsia development with cog."
        )
    return result


if __name__ == "__main__":
    sys.exit(int(not check_all()))
