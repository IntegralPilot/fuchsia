# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import pathlib
import shutil
import uuid

from base import Benchmark


class CreateCitcWorkspace(Benchmark):
    """A benchmark that creates a CitC workspace for the fuchsia/fuchsia repo."""

    def __init__(self) -> None:
        super().__init__(
            name="create-citc-workspace",
            description="Creates a CitC workspace.",
            compare=["create-citc-workspace-and-fetch-prebuilts-with-snapshot"],
        )

    def setup(self) -> None:
        """Sets up the benchmark."""
        self.workspace_name = f"cog_benchmark_{uuid.uuid4().hex}"
        super().setup()

    def cleanup(self) -> None:
        """Cleans up the benchmark."""
        # run the command to delete the workspace
        self.run_command(["git", "citc", "delete", self.workspace_name])
        super().cleanup()

    def run(self) -> None:
        """Runs the benchmark."""
        self.run_command(
            ["git", "citc", "create", self.workspace_name, "fuchsia/fuchsia"]
        )


class CreateCitcWorkspaceAndFetchPrebuiltsNoSnapshot(Benchmark):
    """A benchmark that creates a CitC workspace for the fuchsia/fuchsia repo."""

    def __init__(self) -> None:
        # TODO(b/460268803): Add a flag to enable/disable snapshots
        super().__init__(
            name="create-citc-workspace-and-fetch-prebuilts-no-snapshot",
            description="Creates a CitC workspace and fetches prebuilts without using snapshots.",
            compare=["create-citc-workspace"],
        )

    def setup(self) -> None:
        """Sets up the benchmark."""
        self.workspace_name = f"cog_benchmark_{uuid.uuid4().hex}"
        super().setup()

    def cleanup(self) -> None:
        """Cleans up the benchmark."""
        workspace_fuchsia_path = pathlib.Path(
            f"/google/cog/cloud/{os.environ['USER']}/{self.workspace_name}/fuchsia"
        )
        cartfs_symlink_path = workspace_fuchsia_path / "cartfs-dir"

        if cartfs_symlink_path.is_symlink():
            cartfs_path = cartfs_symlink_path.resolve()
            if cartfs_path.exists():
                shutil.rmtree(cartfs_path)

        # run the command to delete the workspace
        self.run_command(["git", "citc", "delete", self.workspace_name])
        super().cleanup()

    def run(self) -> None:
        """Runs the benchmark."""
        self.run_command(
            ["git", "citc", "create", self.workspace_name, "fuchsia/fuchsia"]
        )
        workspace_fuchsia_path = pathlib.Path(
            f"/google/cog/cloud/{os.environ['USER']}/{self.workspace_name}/fuchsia"
        )
        self.run_command(
            ["scripts/cog/setup_cog_workspace.py"],
            cwd=workspace_fuchsia_path,
        )
