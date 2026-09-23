# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import pathlib
import tempfile
import unittest

import path_normalizer


class MockBazelPaths:
    def __init__(
        self, fuchsia_dir: pathlib.Path, build_dir: pathlib.Path
    ) -> None:
        self.fuchsia_dir = fuchsia_dir
        self.ninja_build_dir = build_dir
        self.top_dir = build_dir / "gen/build/bazel"
        self.output_base = self.top_dir / "output_base"
        self.workspace = self.top_dir / "workspace"
        self.execroot = self.output_base / "execroot" / "_main"
        self.launcher = self.top_dir / "bazel"


class PathNormalizerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.fuchsia_dir = pathlib.Path(self.temp_dir.name) / "fuchsia"
        self.build_dir = self.fuchsia_dir / "out" / "default"
        self.fuchsia_dir.mkdir(parents=True)
        self.build_dir.mkdir(parents=True)

        self.gn_normalizer = path_normalizer.GnPathNormalizer(
            self.fuchsia_dir, self.build_dir
        )

        mock_bazel_paths = MockBazelPaths(self.fuchsia_dir, self.build_dir)
        self.bazel_normalizer = path_normalizer.BazelPathNormalizer(
            mock_bazel_paths
        )

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_gn_path_normalizer(self) -> None:
        # Relative source root paths
        self.assertEqual(
            self.gn_normalizer.normalize_path("../../src/foo.cc"),
            "{SOURCE_ROOT}/src/foo.cc",
        )
        self.assertEqual(
            self.gn_normalizer.normalize_path("../.."),
            "{SOURCE_ROOT}",
        )
        # Relative out root
        self.assertEqual(
            self.gn_normalizer.normalize_path(".."),
            "{OUT_ROOT}",
        )
        self.assertEqual(
            self.gn_normalizer.normalize_path("../obj/foo"),
            "{OUT_ROOT}/obj/foo",
        )
        # Relative build dir / current dir
        self.assertEqual(
            self.gn_normalizer.normalize_path("."),
            "{BUILD_DIR}",
        )
        self.assertEqual(
            self.gn_normalizer.normalize_path("./obj/foo.o"),
            "obj/foo.o",
        )
        # Absolute paths
        self.assertEqual(
            self.gn_normalizer.normalize_path(str(self.fuchsia_dir)),
            "{SOURCE_ROOT}",
        )
        self.assertEqual(
            self.gn_normalizer.normalize_path(
                str(self.fuchsia_dir / "src/foo.cc")
            ),
            "{SOURCE_ROOT}/src/foo.cc",
        )
        self.assertEqual(
            self.gn_normalizer.normalize_path(str(self.build_dir)),
            "{BUILD_DIR}",
        )
        self.assertEqual(
            self.gn_normalizer.normalize_path(
                str(self.build_dir / "obj/foo.o")
            ),
            "obj/foo.o",
        )

        # Unresolved ../ should raise ValueError
        with self.assertRaises(ValueError):
            self.gn_normalizer.normalize_path("../../../outside/file.txt")

    def test_bazel_path_normalizer(self) -> None:
        # Relative paths in execroot
        self.assertEqual(
            self.bazel_normalizer.normalize_path("src/foo.cc"),
            "{SOURCE_ROOT}/src/foo.cc",
        )
        self.assertEqual(
            self.bazel_normalizer.normalize_path("./src/foo.cc"),
            "{SOURCE_ROOT}/src/foo.cc",
        )
        self.assertEqual(
            self.bazel_normalizer.normalize_path(
                "bazel-out/k8-fastbuild/bin/foo"
            ),
            "{BAZEL_OUT}/k8-fastbuild/bin/foo",
        )
        self.assertEqual(
            self.bazel_normalizer.normalize_path(
                "external/+rules_rust+rules_rust/foo.rs"
            ),
            "{OUTPUT_BASE}/external/+rules_rust+rules_rust/foo.rs",
        )
        self.assertEqual(
            self.bazel_normalizer.normalize_path(
                "./external/fuchsia_sdk/include/foo.h"
            ),
            "{OUTPUT_BASE}/external/fuchsia_sdk/include/foo.h",
        )
        self.assertEqual(
            self.bazel_normalizer.normalize_path("."),
            "{SOURCE_ROOT}",
        )
        # Absolute paths
        self.assertEqual(
            self.bazel_normalizer.normalize_path(
                str(self.bazel_normalizer._execroot / "src/foo.cc")
            ),
            "{SOURCE_ROOT}/src/foo.cc",
        )
        self.assertEqual(
            self.bazel_normalizer.normalize_path(
                str(self.fuchsia_dir / "src/foo.cc")
            ),
            "{SOURCE_ROOT}/src/foo.cc",
        )
        self.assertEqual(
            self.bazel_normalizer.normalize_path(
                str(
                    self.bazel_normalizer._output_base
                    / "external/foo_pkg/bar.cc"
                )
            ),
            "{OUTPUT_BASE}/external/foo_pkg/bar.cc",
        )

        # Unresolved ../ should raise ValueError
        with self.assertRaises(ValueError):
            self.bazel_normalizer.normalize_path("../unknown/repo/file.txt")


if __name__ == "__main__":
    unittest.main()
