#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities for parsing Bazel repository marker files and tracking inputs."""

import json
import os
import subprocess
import sys
import typing as T
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
from build_utils import BazelLauncher


class BazelRootRepoMapping:
    """Retrieves and parses the root workspace repository mapping from Bazel."""

    def __init__(
        self,
        mapping: dict[str, str],
    ) -> None:
        self._mapping = mapping
        self._reverse_mapping: dict[str, str] | None = None

    @classmethod
    def new_from_bazel(
        cls: type["BazelRootRepoMapping"],
        launcher: BazelLauncher,
    ) -> "BazelRootRepoMapping":
        """Create an instance by querying Bazel via `bazel mod dump_repo_mapping ""`.

        Args:
            launcher: A BazelLauncher instance.
        Returns:
            A new BazelRootRepoMapping instance.
        Raises:
            RuntimeError if there is a problem running the command or parsing its
            output.
        """
        ret = launcher.run_bazel_command(
            ["mod", "dump_repo_mapping", ""],
            stderr=subprocess.PIPE,
        )
        if ret.returncode != 0:
            raise RuntimeError(
                f"Failed to dump repository mapping from Bazel (exit code {ret.returncode}):\n{ret.stderr}"
            )
        try:
            # Expected format: A JSON object, keys are apparent names, and
            # values are canonical ones, such as:
            #
            # {
            #   "fuchsia_clang": "rules_fuchsia++fuchsia_clang_ext+fuchsia_clang",
            #   "": "",
            #   "main": "",
            #   "zlip": "zlib+"
            # }
            #
            # Note that keys "" and "main" map to the root workspace and always
            # have value "", and can be ignored here.
            #
            raw_mapping = json.loads(ret.stdout)
        except json.JSONDecodeError as e:
            raise RuntimeError(
                f"Failed to parse Bazel repo mapping JSON:\n{ret.stdout}"
            ) from e

        if not isinstance(raw_mapping, dict):
            raise RuntimeError(
                f"Bazel repo mapping should be JSON object, got: {repr(raw_mapping)}"
            )
        mapping = {
            k: v for k, v in raw_mapping.items() if k not in ("", "main")
        }
        return cls(mapping=mapping)

    @staticmethod
    def get_cache_path(build_dir: Path) -> Path:
        """Return the cache file path for the root repo mapping.

        Args:
            build_dir: Ninja build directory path.

        Returns:
            Path to regenerator_outputs/root_repo_mapping.json in the build directory.
        """
        return build_dir / "regenerator_outputs" / "root_repo_mapping.json"

    @classmethod
    def new_from_build_dir(
        cls: type["BazelRootRepoMapping"],
        build_dir: Path,
    ) -> "BazelRootRepoMapping":
        """Create an instance by loading the cached version from the build directory.

        This matches the file written to disk by save_to_disk().

        Args:
            build_dir: Ninja build directory path.

        Returns:
            A new BazelRootRepoMapping initialized with the cached mapping.

        Raises:
            FileNotFoundError: If the cache file does not exist.
            RuntimeError: If parsing the cache file fails.
        """
        cache_path = cls.get_cache_path(build_dir)
        if not cache_path.exists():
            raise FileNotFoundError(
                f"Root repo mapping cache file does not exist: {cache_path}"
            )
        try:
            mapping = json.loads(cache_path.read_text())
        except json.JSONDecodeError as e:
            raise RuntimeError(
                f"Failed to parse root repo mapping cache file {cache_path}: {e}"
            ) from e
        return cls(mapping=mapping)

    def save_to_disk(self, build_dir: Path) -> Path:
        """Store the repository mapping value to disk as a JSON file.

        Args:
            build_dir: Ninja build directory path.

        Returns:
            Path to the written cache file.

        Raises:
            OSError: If writing to the cache file fails.
        """
        cache_path = self.get_cache_path(build_dir)
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        content = json.dumps(self._mapping, indent=2) + "\n"
        if cache_path.exists() and cache_path.read_text() == content:
            return cache_path
        cache_path.write_text(content)
        return cache_path

    def get_mapping(self) -> dict[str, str]:
        """Return the dictionary mapping apparent repo names to canonical names.

        The keys "" and "main" are excluded in the result.

        Returns:
            A dict[str, str] mapping apparent repository names in the root workspace
            to their canonical repository names.
        """
        return dict(self._mapping)

    def get_canonical_name(self, apparent_name: str) -> str | None:
        """Look up the canonical repository name for a given apparent name."""
        return self._mapping.get(apparent_name)

    def get_apparent_name(self, canonical_name: str) -> str | None:
        """Look up the apparent repository name for a given canonical name."""
        if self._reverse_mapping is None:
            self._reverse_mapping = {v: k for k, v in self._mapping.items()}
        return self._reverse_mapping.get(canonical_name)

    def __getitem__(self, apparent_name: str) -> str:
        """Return the canonical repository name for an apparent name.

        Raises:
            KeyError: If apparent_name is not in the mapping.
        """
        return self._mapping[apparent_name]

    def __contains__(self, apparent_name: object) -> bool:
        return apparent_name in self._mapping

    def __iter__(self) -> T.Iterator[str]:
        return iter(self._mapping)

    def __len__(self) -> int:
        return len(self._mapping)
