# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import shutil
import subprocess
from pathlib import Path

import logger


def snapshot_workspace(
    workspace_to_snapshot_from: Path,
    workspace_to_snapshot_to: Path,
    cartfs_mount_point: Path,
) -> None:
    """Snapshots a workspace.

    This method copies a workspace to a new workspace using cartfs and
    cogfsd RPCs.

    Args:
        workspace_to_snapshot_from: The name of the workspace to snapshot from.
        workspace_to_snapshot_to: The name of the new workspace to create.
        cartfs_mount_point: The path to the cartfs mount point.
    """
    from_path = cartfs_mount_point / workspace_to_snapshot_from
    if not from_path.is_dir():
        raise ValueError(
            f"Source workspace directory {from_path} does not exist or is not a directory."
        )

    to_path = cartfs_mount_point / workspace_to_snapshot_to
    if to_path.exists():
        raise ValueError(
            f"Target workspace directory {to_path} already exists."
        )

    logger.log_info(
        f"Snapshotting workspace '{workspace_to_snapshot_from}' to '{workspace_to_snapshot_to}'"
    )

    copy_subdirs = [
        "integration",
        "fuchsia",
        "fuchsia-cog-superproject",
        ".fuchsia_commit_hash",
        ".integration_commit_hash",
    ]

    # Placeholders for endpoint and RPC names.
    cartfs_endpoint = "127.0.0.1:65001"
    cartfs_rpc_copy_directory = "cartfs.Cartfs.CopyDirectory"

    # We need to make the directory first because cartfs.CopyDirectory
    # does not update the directory immediately and a subsequent write
    # will fail. If we create the directory first, we can avoid this issue
    # and still correctly snapshot the workspace.
    to_path.mkdir(parents=True, exist_ok=True)

    for subdir in copy_subdirs:
        from_path_rel = workspace_to_snapshot_from / subdir
        to_path_rel = workspace_to_snapshot_to / subdir
        from_path_abs = cartfs_mount_point / from_path_rel
        to_path_abs = cartfs_mount_point / to_path_rel

        if not from_path_abs.exists():
            logger.log_debug(
                f"Skipping {from_path_rel} because it does not exist."
            )
            continue

        if not from_path_abs.is_dir():
            shutil.copyfile(from_path_abs, to_path_abs)
            continue

        logger.log_info(f"Copying from {from_path_rel} to {to_path_rel}")
        # We need to provide relative paths for the RPC calls.
        subprocess.run(
            [
                "grpc_cli",
                "call",
                cartfs_endpoint,
                cartfs_rpc_copy_directory,
                f'from_path: "{from_path_rel}"\nto_path: "{to_path_rel}"',
                "--channel_creds_type=insecure",
            ],
            check=True,
            capture_output=True,
            text=True,
        )

    logger.log_info(
        f"Done snapshotting workspace '{workspace_to_snapshot_from}' to '{workspace_to_snapshot_to}'"
    )

    # Delete the .fx/config/metrics file from the snapshot.
    metrics_file = to_path / "fuchsia" / ".fx" / "config" / "metrics"
    metrics_file.unlink(missing_ok=True)


def copy_cartfs_directory(from_path_rel: Path, to_path_rel: Path) -> None:
    """Copies a directory within CartFS using RPC.

    Args:
        from_path_rel: Relative path from CartFS mount point to source.
        to_path_rel: Relative path from CartFS mount point to target.
    """
    cartfs_endpoint = "127.0.0.1:65001"
    cartfs_rpc_copy_directory = "cartfs.Cartfs.CopyDirectory"

    logger.log_info(
        f"Copying from {from_path_rel} to {to_path_rel} via CartFS RPC"
    )
    subprocess.run(
        [
            "grpc_cli",
            "call",
            cartfs_endpoint,
            cartfs_rpc_copy_directory,
            f'from_path: "{from_path_rel}"\nto_path: "{to_path_rel}"',
            "--channel_creds_type=insecure",
        ],
        check=True,
        capture_output=True,
        text=True,
    )
