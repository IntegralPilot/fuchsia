# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from build_utils import BazelPaths

"""Provides a way to map Bazel source file paths to the right location."""


class BazelSourcePathMapper:
    """A class to map Bazel source file paths, as returned by queries, to a path in the Fuchsia directory.

    This is needed for several reasons:

    - Queries return file paths relative to the execroot, which can begin with bazel-out/
      for Bazel artifacts, or external/<canonical_name>/ for files in external repositories,
      which can be either source files or generated files.

    - This also supports absolute input file paths, even if they point inside the Bazel
      workspace, execroot or output_base.

    - The Fuchsia workspace is under out/default/gen/build/bazel/workspace/ but
      its file and directory entries are symlinks to the Fuchsia checkout (e.g.
      $WORKSPACE/src --symlink--> $FUCHSIA/src) with a few exceptions such as
      `.bazelrc` or `fuchsia_build_generated/`.

    - Source file paths that belong to external repositories will often appear
      with an 'external/<canonical_name>/` prefix, which first needs to be
      translated into $OUTPUT_BASE/external/<canonical_name>/. In many cases,
      such files are actually symlinks into Fuchsia source files, and must be
      resolved with realpath() to get their proper location.

    - Bazel files located in its "install_base" directory have a timestamp
      far in the future (this is required by Bazel, which will check them each
      time it starts to verify they haven't been modified). Some of these belong
      to the @bazel_tools repository that contain input files. These must be
      ignored, or Ninja depfiles that contain them will create no-op breakages.

    - Using realpath() on a lot of files (e.g. when generating Ninja depfiles)
      is actually surprisingly slow, especially when long symlink chains are
      used, as happens often with Bazel. Hence this class also implements a small
      in-memory cache, since the same source paths often are used multiple
      times.
    """

    def __init__(self, bazel_paths: BazelPaths) -> None:
        self._fuchsia_dir = bazel_paths.fuchsia_dir

        # Absolute path to the workspace directory.
        self._workspace_prefix = f"{bazel_paths.workspace}/"

        # Workspace directory path, relative to the Ninja build directory.
        self._workspace_build_prefix = (
            os.path.relpath(bazel_paths.workspace, bazel_paths.ninja_build_dir)
            + "/"
        )
        self._output_base_prefix = f"{bazel_paths.output_base}/"
        self._external_dir = bazel_paths.output_base / "external"
        self._execroot_prefix = f"{bazel_paths.execroot}/"
        self._cache: dict[str, str] = {}

    @staticmethod
    def _is_workspace_generated_file(rel_path: str) -> bool:
        """Return True if a given workspace-relative path is build-generated."""
        # The exceptions to the workspace root symlinks. These are generated too.
        if rel_path == ".bazelrc":
            return True
        if rel_path.startswith("fuchsia_build_generated/"):
            return True
        return False

    def resolve_path(self, bazel_source_path: str) -> str:
        """Map a Bazel source file path to a path relative to the Fuchsia checkout.

        Args:
            bazel_source_path: A Bazel source path relative to the execroot, as returned
              by Bazel queries.

        Returns:
            A source file path relative to the Fuchsia source directory, or an empty
            string if the input path corresponds to a Bazel artifact (i.e. when it is
            a relative path starting with "bazel-out/", or an absolute path starting
            with "$OUTPUT_BASE/execroot/<name>/bazel-out/").
        """
        cache_value = self._cache.get(bazel_source_path)
        if cache_value is not None:
            return cache_value

        resolved_path = self.resolve_path_no_cache(bazel_source_path)
        self._cache[bazel_source_path] = resolved_path
        return resolved_path

    def resolve_path_no_cache(self, path: str) -> str:
        """Resolve a source file path to a Fuchsia-relative path. No caching."""
        if not path.startswith("/"):
            # A relative path, assume it is relative to the execroot.
            if path.startswith("bazel-out/"):
                # Bazel artifact path, not a source file.
                return ""

            if self._is_workspace_generated_file(path):
                return ""

            if path.startswith("external/"):
                # This file path belongs to an external repository, but could link to something
                # else, hence will have to be resolved with realpath() below.
                path = str(self._external_dir / path.removeprefix("external/"))

            else:
                # Assume regular source file path, relative to the workspace, ergo relative
                # to the Fuchsia source directory. Return as-is.
                return path

        # Use realpath() to resolve symlink chains if needed.
        path = os.path.realpath(path)

        if "prebuilt/third_party/bazel/" in path or "/install_base/" in path:
            # Assume a Bazel install_base file, must be ignored.
            return ""

        if path.startswith(self._execroot_prefix):
            relative_path = path.removeprefix(self._execroot_prefix)
            if relative_path.startswith("bazel-out/"):
                # Bazel artifact, not a source file path.
                return ""

            if relative_path.startswith("external/"):
                # A generated file inside a repository directory. Also an artifact.
                return ""

            # Assume source file.
            return relative_path

        if path.startswith(self._output_base_prefix):
            # Bazel artifact, not a source file path.
            return ""

        if path.startswith(self._workspace_prefix):
            # Workspace path, rebase to fuchsia directory by removing workspace prefix.
            fuchsia_path = path.removeprefix(self._workspace_prefix)
            if self._is_workspace_generated_file(fuchsia_path):
                return ""

            return fuchsia_path

        # Assume regular source file path, make it relative to the Fuchsia dir.
        return os.path.relpath(path, self._fuchsia_dir)
