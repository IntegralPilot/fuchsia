# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess
from functools import cache
from pathlib import Path


class CartfsError(Exception):
    """Base exception for Cartfs errors."""


class CartfsNotRunningError(CartfsError):
    """Raised when cartfs is either not installed or not running."""


class Cartfs:
    """A class to interact with a cartfs filesystem."""

    @staticmethod
    @cache
    def cartfs_uid() -> str:
        try:
            return subprocess.check_output(
                ["id", "-u", "cartfs"],
                text=True,
            ).strip()
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            raise CartfsNotRunningError(
                "Unable to find the uid for cartfs. Is it installed and running?"
            ) from e

    @staticmethod
    @cache
    def find_mount_point() -> Path:
        """Finds the mount point for cartfs."""
        try:
            output = subprocess.check_output(
                [
                    "findmnt",
                    "-n",
                    "-t",
                    "fuse",
                    "-O",
                    f"user_id={Cartfs.cartfs_uid()}",
                    "-o",
                    "TARGET",
                ],
                text=True,
            ).strip()

            if not output:
                raise CartfsNotRunningError(
                    "Unable to find the mount point for cartfs. Is it running?"
                )

            return Path(output.splitlines()[0])
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            raise CartfsNotRunningError(
                "Unable to find the mount point for cartfs. Is it running?"
            ) from e

    def __init__(self) -> None:
        """Initializes a Cartfs instance."""
        self.mount_point = self.__class__.find_mount_point()

    def suggest_cartfs_dir_name(self, base_name: str) -> Path:
        """Suggests a directory name within the cartfs mount point.

        Args:
            base_name: The base name for the workspace directory.

        Returns:
            A path to a directory that does not yet exist.
        """
        path = Path(base_name)
        counter = 1
        while (self.mount_point / path).exists():
            path = Path(f"{base_name}-{counter}")
            counter += 1
        return path
