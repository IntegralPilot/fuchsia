# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Path normalization utilities for GN and Bazel build command comparisons.
"""

import os
import pathlib
import sys
import typing as T

_FUCHSIA_DIR = pathlib.Path(__file__).parent.parent.parent.parent
sys.path.insert(0, str(_FUCHSIA_DIR / "build/bazel/scripts"))
import build_utils


class PathNormalizer(T.Protocol):
    """Base protocol for all path normalizer classes."""

    def normalize_path(self, path: str) -> str:
        ...


class GnPathNormalizer(PathNormalizer):
    """Normalizes paths appearing in GN / Ninja build commands.

    This is used to transform input paths as they appear in GN build
    command, and replacing well-known locations with {SOURCE_ROOT}
    or {BUILD_DIR} expressions.
    """

    def __init__(
        self,
        fuchsia_dir: pathlib.Path | str,
        build_dir: pathlib.Path | str,
    ) -> None:
        """Create instance."""
        self._fuchsia_dir = pathlib.Path(fuchsia_dir).resolve()
        self._build_dir = pathlib.Path(build_dir).resolve()
        # Compute relative path from build_dir to fuchsia_dir (e.g. "../..")
        self._rel_source_root = os.path.relpath(
            self._fuchsia_dir, self._build_dir
        )
        # Compute relative path from build_dir to parent of build_dir (e.g. "..")
        self._rel_out_root = os.path.relpath(
            self._build_dir.parent, self._build_dir
        )

    @property
    def fuchsia_dir(self) -> pathlib.Path:
        """Absolute path to the Fuchsia source directory path."""
        return self._fuchsia_dir

    @property
    def build_dir(self) -> pathlib.Path:
        """Absolute path to the Ninja build directory."""
        return self._build_dir

    def normalize_path(self, path: str) -> str:
        """Normalize a path from a GN command to a canonical string.

        Args:
          path: A path as it appears in a GN build command.

        Returns:
          A path string, where the absolute or relative values
          of the following paths are replaced by expressions such as:

          - The Fuchsia source directory is mapped to "{SOURCE_ROOT}"
          - The Ninja build directory is mapped to "{BUILD_DIR}"
          - The out/ directory is mapped to "{OUT_ROOT}"

          This also removes leading './' path components.

        Raises:
          ValueError if the path begins with '../' but cannot
          be resolved to a known expression.
        """
        if not path:
            return ""

        # Normalize slashes and remove leading ./
        p = path.strip()
        while p.startswith("./"):
            p = p[2:]

        # Check absolute path matches
        if os.path.isabs(p):
            resolved = pathlib.Path(p).resolve()
            if resolved == self._build_dir:
                return "{BUILD_DIR}"
            if resolved == self._build_dir.parent:
                return "{OUT_ROOT}"
            if resolved == self._fuchsia_dir:
                return "{SOURCE_ROOT}"
            try:
                rel_to_build = resolved.relative_to(self._build_dir)
                return self.normalize_path(str(rel_to_build))
            except ValueError:
                pass
            try:
                rel_to_src = resolved.relative_to(self._fuchsia_dir)
                return f"{{SOURCE_ROOT}}/{rel_to_src}"
            except ValueError:
                pass
            return str(resolved)

        # Handle exact relative roots
        if p in (".", ""):
            return "{BUILD_DIR}"
        if p == self._rel_out_root:
            return "{OUT_ROOT}"
        if p == self._rel_source_root:
            return "{SOURCE_ROOT}"

        # Handle prefix matching with rel_source_root (e.g. "../../src/foo.cc")
        src_prefix = f"{self._rel_source_root}/"
        if p.startswith(src_prefix):
            remainder = p[len(src_prefix) :]
            if remainder.startswith("../") or "/../" in remainder:
                raise ValueError(
                    f"Unexpected unresolved relative path in GN command: {path}"
                )
            return f"{{SOURCE_ROOT}}/{remainder}"

        out_prefix = f"{self._rel_out_root}/"
        if p.startswith(out_prefix):
            remainder = p[len(out_prefix) :]
            if remainder.startswith("../") or "/../" in remainder:
                raise ValueError(
                    f"Unexpected unresolved relative path in GN command: {path}"
                )
            return f"{{OUT_ROOT}}/{remainder}"

        # If a leading '../' persists, it's an unrecognized path layout
        if p.startswith("../"):
            raise ValueError(
                f"Unexpected unresolved relative path in GN command: {path}"
            )

        return p


class BazelPathNormalizer(PathNormalizer):
    """Normalizes paths appearing in Bazel build commands."""

    def __init__(self, bazel_paths: build_utils.BazelPaths) -> None:
        """Create instance."""
        self._bazel_paths = bazel_paths
        self._execroot = bazel_paths.execroot.resolve()
        self._workspace = bazel_paths.workspace.resolve()
        self._fuchsia_dir = bazel_paths.fuchsia_dir.resolve()
        self._output_base = bazel_paths.output_base.resolve()

    @property
    def bazel_paths(self) -> build_utils.BazelPaths:
        """A BazelPaths instance."""
        return self._bazel_paths

    def normalize_path(self, path: str) -> str:
        """Normalize a path from a Bazel command to a canonical string.

        Args:
           path: A path string, as it appears in a Bazel command.
        Returns:
           The input path string, with known path prefixes replaced
           with substitution expressions. Where:

           - external/<canonical_repo_name>/... -> {OUTPUT_BASE}/external/<canonical_repo_name>/...
           - bazel-out/... -> {BAZEL_OUT}/...
           - execroot/workspace/fuchsia_dir -> {SOURCE_ROOT}

          Removes leading './'.

        Raises:
          ValueError if input path begins with '../' and cannot
          be mapped to a known expression.
        """
        if not path:
            return ""

        p = path.strip()
        while p.startswith("./"):
            p = p[2:]

        # Check absolute path matches
        if os.path.isabs(p):
            resolved = pathlib.Path(p).resolve()
            if resolved == self._execroot or resolved == self._workspace:
                return "{SOURCE_ROOT}"
            if resolved == self._fuchsia_dir:
                return "{SOURCE_ROOT}"
            if resolved == self._output_base:
                return "{OUTPUT_BASE}"
            try:
                rel_to_exec = resolved.relative_to(self._execroot)
                return self.normalize_path(str(rel_to_exec))
            except ValueError:
                pass
            try:
                rel_to_output_base = resolved.relative_to(self._output_base)
                return self.normalize_path(str(rel_to_output_base))
            except ValueError:
                pass
            try:
                rel_to_src = resolved.relative_to(self._fuchsia_dir)
                return f"{{SOURCE_ROOT}}/{rel_to_src}"
            except ValueError:
                pass
            return str(resolved)

        if p in (".", ""):
            return "{SOURCE_ROOT}"

        if p.startswith("external/"):
            remainder = p[len("external/") :]
            if remainder.startswith("../") or "/../" in remainder:
                raise ValueError(
                    f"Unexpected unresolved relative path in Bazel command: {path}"
                )
            return f"{{OUTPUT_BASE}}/external/{remainder}"
        if p == "external":
            return "{OUTPUT_BASE}/external"

        if p.startswith("bazel-out/"):
            remainder = p[len("bazel-out/") :]
            if remainder.startswith("../") or "/../" in remainder:
                raise ValueError(
                    f"Unexpected unresolved relative path in Bazel command: {path}"
                )
            return f"{{BAZEL_OUT}}/{remainder}"
        if p == "bazel-out":
            return "{BAZEL_OUT}"

        if p.startswith("../") or "/../" in p:
            raise ValueError(
                f"Unexpected unresolved relative path in Bazel command: {path}"
            )

        return f"{{SOURCE_ROOT}}/{p}"


class MockPathNormalizer(PathNormalizer):
    """A mock PathNormalizer for tests that doesn't do anything."""

    def normalize_path(self, path: str) -> str:
        return path
