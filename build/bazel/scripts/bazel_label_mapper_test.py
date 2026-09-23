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

from bazel_label_mapper import BazelLabelMapper
from bazel_repository_utils import BazelRootRepoMapping
from build_utils import BazelPaths


class BazelLabelMapperTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.tmp_dir = Path(self._td.name)

        self.fuchsia_dir = self.tmp_dir / "fuchsia"
        self.fuchsia_dir.mkdir()
        self.write_fuchsia_file(".jiri_manifest", "")

        self.build_dir = self.fuchsia_dir / "out" / "default"
        self.build_dir.mkdir(parents=True)

        BazelPaths.write_topdir_config_for_test(
            self.fuchsia_dir, "gen/build/bazel"
        )
        self.bazel_paths = BazelPaths(self.fuchsia_dir, self.build_dir)
        self.workspace_dir = self.bazel_paths.workspace
        self.workspace_dir.mkdir(parents=True, exist_ok=True)
        self.output_base = self.bazel_paths.output_base
        self.external_dir = self.output_base / "external"
        self.external_dir.mkdir(parents=True, exist_ok=True)

        self.sample_mapping = {
            "boringssl": "boringssl+",
            "fuchsia_sdk": "rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk",
            "fuchsia_prebuilt_rust": "+_repo_rules+fuchsia_prebuilt_rust",
        }
        self.root_repo_mapping = BazelRootRepoMapping(
            mapping=self.sample_mapping
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def write_fuchsia_file(self, dst_path: str, content: str) -> Path:
        file_path = self.fuchsia_dir / dst_path
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(content)
        return file_path

    def test_root_workspace_labels(self) -> None:
        mapper = BazelLabelMapper(
            str(self.workspace_dir),
            str(self.build_dir),
            root_repo_mapping=self.root_repo_mapping,
        )

        foo_file = self.workspace_dir / "src" / "foo" / "bar.cc"
        foo_file.parent.mkdir(parents=True, exist_ok=True)
        foo_file.write_text("// dummy")

        for label in (
            "//src/foo:bar.cc",
            "@//src/foo:bar.cc",
            "@@//src/foo:bar.cc",
        ):
            self.assertEqual(
                mapper.source_label_to_path(label),
                str(foo_file.resolve()),
            )

    def test_external_apparent_repo_mapping(self) -> None:
        mapper = BazelLabelMapper(
            str(self.workspace_dir),
            str(self.build_dir),
            root_repo_mapping=self.root_repo_mapping,
        )

        # Create source in Fuchsia tree and symlink from external repo
        src_file = self.write_fuchsia_file(
            "src/crypto/boringssl.c", "/* crypto */"
        )

        canonical_repo_dir = self.external_dir / "boringssl+"
        canonical_repo_dir.mkdir(parents=True, exist_ok=True)
        ext_file = canonical_repo_dir / "boringssl.c"
        os.symlink(src_file, ext_file)

        # Apparent label @boringssl//:boringssl.c should map to canonical repo boringssl+
        # and resolve to the realpath in the Fuchsia checkout.
        self.assertEqual(
            mapper.source_label_to_path("@boringssl//:boringssl.c"),
            str(src_file.resolve()),
        )

    def test_external_canonical_repo_label(self) -> None:
        mapper = BazelLabelMapper(
            str(self.workspace_dir),
            str(self.build_dir),
            root_repo_mapping=self.root_repo_mapping,
        )

        src_file = self.write_fuchsia_file("sdk/pkg/fdio/vfs.h", "/* vfs */")

        canonical_repo_dir = (
            self.external_dir / "rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk"
        )
        canonical_pkg_dir = canonical_repo_dir / "pkg" / "fdio"
        canonical_pkg_dir.mkdir(parents=True, exist_ok=True)
        ext_file = canonical_pkg_dir / "vfs.h"
        os.symlink(src_file, ext_file)

        # Both apparent and canonical labels should map to the same file
        self.assertEqual(
            mapper.source_label_to_path("@fuchsia_sdk//pkg/fdio:vfs.h"),
            str(src_file.resolve()),
        )
        self.assertEqual(
            mapper.source_label_to_path(
                "@@rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk//pkg/fdio:vfs.h"
            ),
            str(src_file.resolve()),
        )

    def test_external_transitive_fallback(self) -> None:
        mapper = BazelLabelMapper(
            str(self.workspace_dir),
            str(self.build_dir),
            root_repo_mapping=self.root_repo_mapping,
        )

        src_file = self.write_fuchsia_file(
            "third_party/transitive/lib.cc", "// transitive"
        )

        transitive_repo_dir = (
            self.external_dir / "rules_python++transitive+sub_repo"
        )
        transitive_repo_dir.mkdir(parents=True, exist_ok=True)
        ext_file = transitive_repo_dir / "lib.cc"
        os.symlink(src_file, ext_file)

        # Canonical name not in root mapping should be used as-is
        self.assertEqual(
            mapper.source_label_to_path(
                "@@rules_python++transitive+sub_repo//:lib.cc"
            ),
            str(src_file.resolve()),
        )

    def test_content_hash_lookup_with_reverse_mapping(self) -> None:
        mapper = BazelLabelMapper(
            str(self.workspace_dir),
            str(self.build_dir),
            root_repo_mapping=self.root_repo_mapping,
        )

        # Create content hash file in regenerator_outputs/bazel_content_hashes/fuchsia_sdk.hash
        hashes_dir = (
            self.build_dir / "regenerator_outputs" / "bazel_content_hashes"
        )
        hashes_dir.mkdir(parents=True, exist_ok=True)
        hash_file = hashes_dir / "fuchsia_sdk.hash"
        hash_file.write_text("hash123")

        canonical_repo_dir = (
            self.external_dir / "rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk"
        )
        canonical_repo_dir.mkdir(parents=True, exist_ok=True)
        # Generated file (not a symlink to outside workspace)
        gen_file = canonical_repo_dir / "generated.txt"
        gen_file.write_text("generated")

        # Label from canonical or apparent repo name should map to fuchsia_sdk.hash
        self.assertEqual(
            mapper.source_label_to_path("@fuchsia_sdk//:generated.txt"),
            str(hash_file.resolve()),
        )
        self.assertEqual(
            mapper.source_label_to_path(
                "@@rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk//:generated.txt"
            ),
            str(hash_file.resolve()),
        )

    def test_load_mapping_from_disk_cache(self) -> None:
        # Save mapping to disk cache
        self.root_repo_mapping.save_to_disk(self.build_dir)

        # Instantiate mapper without passing root_repo_mapping explicitly
        mapper = BazelLabelMapper(
            str(self.workspace_dir),
            str(self.build_dir),
        )

        src_file = self.write_fuchsia_file(
            "src/crypto/boringssl.c", "/* crypto */"
        )

        canonical_repo_dir = self.external_dir / "boringssl+"
        canonical_repo_dir.mkdir(parents=True, exist_ok=True)
        ext_file = canonical_repo_dir / "boringssl.c"
        os.symlink(src_file, ext_file)

        self.assertEqual(
            mapper.source_label_to_path("@boringssl//:boringssl.c"),
            str(src_file.resolve()),
        )


if __name__ == "__main__":
    unittest.main()
