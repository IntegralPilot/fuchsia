#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import tempfile
import unittest
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR))

from bazel_source_path_mapper import BazelSourcePathMapper
from build_utils import BazelPaths


class BazelSourcePathMapperTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.fuchsia_dir = Path(self._td.name) / "fuchsia"
        self.fuchsia_dir.mkdir()
        (self.fuchsia_dir / ".jiri_manifest").write_text("")

        self.build_dir = self.fuchsia_dir / "out" / "build_dir"
        self.build_dir.mkdir(parents=True)

        BazelPaths.write_topdir_config_for_test(
            self.fuchsia_dir, "gen/build/bazel"
        )
        self.bazel_paths = BazelPaths(self.fuchsia_dir, self.build_dir)

        self.mapper = BazelSourcePathMapper(self.bazel_paths)

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_resolve_artifact_path(self) -> None:
        # Relative artifact paths
        self.assertEqual(self.mapper.resolve_path("bazel-out/foo/bar"), "")

        # Absolute artifact paths via execroot
        execroot_artifact = f"{self.bazel_paths.execroot}/bazel-out/foo/bar"
        self.assertEqual(self.mapper.resolve_path(execroot_artifact), "")

        # Absolute artifact paths via output_base
        output_base_artifact = f"{self.bazel_paths.output_base}/execroot/_main/bazel-out/config_dir/bin/foo/bar"
        self.assertEqual(self.mapper.resolve_path(output_base_artifact), "")

        # External repository generated files
        execroot_external = f"{self.bazel_paths.execroot}/external/repo_canonical_name+/generated"
        self.assertEqual(self.mapper.resolve_path(execroot_external), "")

    def test_resolve_regular_source_path(self) -> None:
        # A file relative to the execroot, not starting with bazel-out or external
        self.assertEqual(self.mapper.resolve_path("src/foo/bar"), "src/foo/bar")

        # A file with an absolute path to a source file.
        self.assertEqual(
            self.mapper.resolve_path(f"{self.fuchsia_dir}/src/foo/bar"),
            "src/foo/bar",
        )

    def test_resolve_external_repository_path(self) -> None:
        # Setup an external repo path that is a symlink into Fuchsia source directory
        foo_src = self.fuchsia_dir / "src" / "foo"
        foo_src.mkdir(parents=True)
        foo_file = foo_src / "bar.cc"
        foo_file.write_text("Hello")

        # external_dir is output_base/external
        external_dir = self.bazel_paths.output_base / "external"
        external_foo_repo = external_dir / "foo_repo"
        external_foo_repo.mkdir(parents=True)

        # Create a symlink in the external repo that points to the fuchsia source file
        os.symlink(foo_file, external_foo_repo / "bar.cc")

        # Resolve an external path
        self.assertEqual(
            self.mapper.resolve_path("external/foo_repo/bar.cc"),
            "src/foo/bar.cc",
        )

    def test_resolve_workspace_exception_path(self) -> None:
        # Path inside the workspace that is one of the generated files exception
        workspace_bazelrc = self.bazel_paths.workspace / ".bazelrc"
        self.assertEqual(self.mapper.resolve_path(str(workspace_bazelrc)), "")

        workspace_generated = (
            self.bazel_paths.workspace / "fuchsia_build_generated" / "foo"
        )
        self.assertEqual(self.mapper.resolve_path(str(workspace_generated)), "")

    def test_resolve_workspace_symlink(self) -> None:
        # Workspace is symlinked to the main source checkout.
        workspace_file = self.bazel_paths.workspace / "src/foo/bar.cc"

        # We don't necessarily need to create the actual symlinks, because path reasoning uses
        # removeprefix() to strip the workspace prefix.
        self.assertEqual(
            self.mapper.resolve_path(str(workspace_file)), "src/foo/bar.cc"
        )

    def test_cache(self) -> None:
        # First call calculates
        path = "src/foo/bar"
        self.assertEqual(self.mapper.resolve_path(path), "src/foo/bar")

        # Second call should use cache
        self.assertEqual(self.mapper.resolve_path(path), "src/foo/bar")

        # Verify cache content
        self.assertIn(path, self.mapper._cache)
        self.assertEqual(self.mapper._cache[path], "src/foo/bar")

    def test_install_base_ignored(self) -> None:
        install_base_file = "/path/to/prebuilt/third_party/bazel/some_file"
        self.assertEqual(self.mapper.resolve_path(install_base_file), "")

    def test_install_base_ignored_cached(self) -> None:
        # Test Jiri cached prebuilt bazel
        cached_install_base_file = "/path/to/fuchsia/.jiri_root/prebuilts/5db62ba47e93dba9fbb45195c7208820fc2b301073dc579d23c8ab1c5f4cdc96/install_base/some_file"
        self.assertEqual(self.mapper.resolve_path(cached_install_base_file), "")

        # Test fx-worktree shared cache bazel
        worktree_install_base_file = "/path/to/home/.fuchsia/worktrees/shared-prebuilts/merged/prebuilt_third_party_bazel_linux-x64/5db62ba47e93dba9fbb45195c7208820fc2b301073dc579d23c8ab1c5f4cdc96/install_base/some_file"
        self.assertEqual(
            self.mapper.resolve_path(worktree_install_base_file), ""
        )

    def test_resolve_relative_generated_file(self) -> None:
        self.assertEqual(self.mapper.resolve_path(".bazelrc"), "")
        self.assertEqual(
            self.mapper.resolve_path("fuchsia_build_generated/foo"), ""
        )

    def test_resolve_execroot_source(self) -> None:
        execroot_source = f"{self.bazel_paths.execroot}/src/foo/bar.cc"
        self.assertEqual(
            self.mapper.resolve_path(execroot_source), "src/foo/bar.cc"
        )

    def test_resolve_output_base_non_execroot(self) -> None:
        # Path starts with output_base but not execroot
        output_base_file = f"{self.bazel_paths.output_base}/some_file"
        self.assertEqual(self.mapper.resolve_path(output_base_file), "")

    def test_resolve_outside_path(self) -> None:
        # Path outside everything
        outside_path = "/some/other/path/file.cc"
        expected = os.path.relpath(outside_path, self.fuchsia_dir)
        self.assertEqual(self.mapper.resolve_path(outside_path), expected)

    def test_resolve_external_repository_non_symlink(self) -> None:
        # Setup an external repo path that is NOT a symlink
        external_dir = self.bazel_paths.output_base / "external"
        external_foo_repo = external_dir / "foo_repo"
        external_foo_repo.mkdir(parents=True)
        foo_file = external_foo_repo / "bar.cc"
        foo_file.write_text("Hello")

        # Resolve an external path that is a real file in external repo
        self.assertEqual(
            self.mapper.resolve_path("external/foo_repo/bar.cc"),
            "",
        )


if __name__ == "__main__":
    unittest.main()
