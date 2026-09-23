#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from idk_generator.idk_generator import (
    AtomInfo,
    IdkGenerator,
    relative_to_walk_up,
)


class IdkGeneratorTest(unittest.TestCase):
    _FUCHSIA_SOURCE_DIR = Path(__file__).parent.parent.parent.parent
    _COLLECTION_INFO: AtomInfo = {
        "atom_id": "1.2.3.4",
        "atom_label": "//test_idk:collection",
        "atom_type": "collection",
        "category": "partner",
        "meta_dest": "meta/manifest.json",
        "prebuild_info": {
            "arch": {"host": "x86_64-linux-gnu", "target": ["arm64"]},
            "root": ".",
            "schema_version": "1",
        },
    }
    _SIMPLE_FIDL_LIBRARY_INFO: AtomInfo = {
        "atom_id": "sdk://pkg/fuchsia.simple",
        "atom_label": "//sdk/fidl/fuchsia.simple:fuchsia.simple_fidl_sdk",
        "atom_type": "fidl_library",
        "category": "partner",
        "meta_dest": "fidl/fuchsia.simple/meta.json",
        "idk_name": "fuchsia.simple",
        "prebuild_info": {
            "file_base": "fidl/fuchsia.simple",
        },
        "atom_deps": [],
        "atom_files": [],
        "is_stable": True,
        "api_area": "Unknown",
    }
    _SIMPLE_CC_SOURCE_LIBRARY_INFO: AtomInfo = {
        "atom_id": "sdk://pkg/simple_cc",
        "atom_label": "//sdk/lib/simple:simple_cc_sdk",
        "atom_type": "cc_source_library",
        "category": "partner",
        "meta_dest": "pkg/simple_cc_source/meta.json",
        "idk_name": "simple_cc_source",
        "prebuild_info": {
            "file_base": "pkg/simple_cc_source",
            "headers": [],
            "include_dir": "include",
            "sources": [],
        },
        "atom_deps": [],
        "atom_files": [],
        "is_stable": True,
    }
    _SIMPLE_DATA_INFO: AtomInfo = {
        "atom_id": "sdk://pkg/some_data",
        "atom_label": "//sdk/data/invalid:some_data_sdk",
        "atom_type": "data",
        "category": "partner",
        "meta_dest": "data/some/meta.json",
        "meta_contents": {"name": "some_data", "type": "data"},
        "prebuild_info": {},  # No prebuild_info for data atoms.
        "atom_files": [],
        "is_stable": True,
    }

    def setUp(self) -> None:
        self.tmpdir = tempfile.TemporaryDirectory()
        self.build_dir = Path(self.tmpdir.name) / "build_out"
        self.source_dir = self._FUCHSIA_SOURCE_DIR
        self.output_dir = Path(self.tmpdir.name) / "idk_output"

        self.build_dir.mkdir(parents=True, exist_ok=True)
        self.source_dir.mkdir(parents=True, exist_ok=True)

    def tearDown(self) -> None:
        self.tmpdir.cleanup()

    def _write_build_file(self, relative_path: str, content: str) -> Path:
        """Write content a file in the mocked build_dir."""
        path = self.build_dir / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return path

    # Use an atom and alias to ensure the check covers both lists.
    def test_init_duplicate_label_error(
        self,
    ) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            self._SIMPLE_DATA_INFO,
            {
                "atom_label": "//sdk/data/invalid:some_data_sdk",
                "atom_type": "alias",
                "atom_actual": "//sdk/data/invalid:other_data_sdk",
            },
        ]

        with self.assertRaisesRegex(
            AssertionError,
            "Multiple atoms have the same label '//sdk/data/invalid:some_data_sdk'.",
        ):
            _ = IdkGenerator(manifest, self.build_dir, self.source_dir)

    def test_init_with_external_build_dir(
        self,
    ) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
        ]

        source_dir = Path(self.tmpdir.name) / "fuchsia"
        source_dir.mkdir(parents=True, exist_ok=True)
        build_dir = Path(self.tmpdir.name) / "cartfs/out/1234"
        build_dir.mkdir(parents=True, exist_ok=True)

        generator = IdkGenerator(manifest, build_dir, source_dir)
        self.assertEqual(
            generator._prebuild_map._relative_source_prefix_from_build_dir,
            "../../../fuchsia/",
        )

    def test_generate_meta_file_contents_simple_collection(self) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
        ]

        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        ret_code, files_read = generator.GenerateMetaFileContents()
        self.assertEqual(ret_code, 0)
        self.assertEqual(files_read, set())
        self.assertEqual(generator.collection_meta_path, "meta/manifest.json")
        self.assertIn("meta/manifest.json", generator._meta_files)
        self.assertEqual(
            generator._meta_files["meta/manifest.json"]["parts"], []
        )

    def test_generate_meta_file_contents_unhandled_type(self) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {
                "atom_id": "sdk://pkg/unhandled",
                "atom_label": "//sdk/unhandled",
                "atom_type": "unhandled_type",
                "category": "partner",
                "meta_dest": "pkg/unhandled/meta.json",
                "prebuild_info": {},
                "atom_files": [],
                "is_stable": True,
            },
        ]

        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            "Atom type `unhandled_type` for `sdk://pkg/unhandled` is unsupported. Valid types are: \\['bind_library', .*, 'version_history'\\]"
            "",
        ):
            generator.GenerateMetaFileContents()

    def test_generate_meta_file_contents_multiple_collections_error(
        self,
    ) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {
                "atom_id": "1.2.3.4",
                "atom_label": "//sdk/collection:collection2",
                "atom_type": "collection",
                "category": "partner",
                "meta_dest": "meta/manifest2.json",
                "prebuild_info": {
                    "arch": {"host": "x86_64-linux-gnu", "target": ["arm64"]},
                    "root": ".",
                    "schema_version": "1",
                },
                "atom_files": [],
                "is_stable": True,
            },
        ]

        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError, "More than one collection info provided."
        ):
            generator.GenerateMetaFileContents()

    def test_generate_meta_file_contents_missing_deps(self) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {
                "atom_id": "sdk://pkg/test_cc",
                "atom_label": "//sdk/lib/test:test_cc_sdk",
                "atom_type": "cc_source_library",
                "category": "partner",
                "meta_dest": "pkg/test_cc_source/meta.json",
                "idk_name": "test_cc_source",
                "prebuild_info": {
                    "file_base": "pkg/test_cc_source",
                    "headers": [],
                    "include_dir": "include",
                    "sources": [],
                },
                "atom_deps": [self._SIMPLE_FIDL_LIBRARY_INFO["atom_label"]],
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            KeyError, "//sdk/fidl/fuchsia.simple:fuchsia.simple_fidl_sdk"
        ):
            generator.GenerateMetaFileContents()

    def test_generate_meta_file_contents_with_excluded_category(self) -> None:
        self._write_build_file("path/to/source.file", "")
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {
                "atom_id": "sdk://pkg/prebuilt_partner_atom",
                "atom_label": "//sdk/partner/prebuilt_category_atom",
                "atom_type": "data",
                "category": "prebuilt",
                "meta_dest": "partner/prebuilt_category_atom/meta.json",
                "meta_contents": {
                    "name": "prebuilt_category_atom",
                    "type": "data",
                },
                "prebuild_info": {},
                "atom_files": [
                    {
                        "dest": "partner/prebuilt_category_atom/file.txt",
                        "source": "path/to/source.file",
                    }
                ],
                "is_stable": True,
            },
            {
                "atom_id": "sdk://pkg/partner_atom",
                "atom_label": "//sdk/partner/partner_atom",
                "atom_type": "data",
                "category": "partner",
                "meta_dest": "partner/partner_atom/meta.json",
                "meta_contents": {"name": "partner_atom", "type": "data"},
                "prebuild_info": {},
                "atom_files": [
                    {
                        "dest": "partner/partner_atom/file.txt",
                        "source": "path/to/source.file",
                    }
                ],
                "is_stable": True,
            },
        ]

        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        ret_code, files_read = generator.GenerateMetaFileContents()
        self.assertEqual(ret_code, 0)
        self.assertEqual(files_read, set())
        self.assertIn("partner/partner_atom/meta.json", generator._meta_files)
        self.assertEqual(
            generator._meta_files["partner/partner_atom/meta.json"],
            {"name": "partner_atom", "type": "data"},
        )
        self.assertIn("partner/partner_atom/file.txt", generator._atom_files)
        self.assertEqual(
            generator._atom_files["partner/partner_atom/file.txt"],
            "path/to/source.file",
        )
        self.assertEqual(generator.collection_meta_path, "meta/manifest.json")
        collection_meta = generator._meta_files["meta/manifest.json"]
        self.assertEqual(len(collection_meta["parts"]), 1)
        self.assertEqual(
            collection_meta["parts"][0]["meta"],
            "partner/partner_atom/meta.json",
        )

    def test_generate_meta_file_contents_package_atom(self) -> None:
        package_manifest_content = {
            "version": "1",
            "package": {"name": "test_package_name", "version": "0"},
            "blobs": [
                {
                    "source_path": "path/to/blob1",
                    "path": "data/blob1.dat",
                    "merkle": "merkle1",
                    "size": 10,
                }
            ],
        }
        package_manifest_path = self._write_build_file(
            "obj/package/manifest.json", json.dumps(package_manifest_content)
        )

        self._write_build_file("path/to/blob1", "")

        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {
                "atom_id": "sdk://pkg/test_package",
                "atom_label": "//sdk/packages:test_package",
                "atom_type": "package",
                "category": "partner",
                "meta_dest": "packages/test_package/meta.json",
                "idk_name": "a_package",
                "prebuild_info": {
                    "package_manifest": str(
                        package_manifest_path.relative_to(self.build_dir)
                    ),
                    "api_level": "NEXT",
                    "arch": "arm64",
                },
                "atom_files": [],
                "is_stable": True,
            },
        ]

        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        ret_code, files_read = generator.GenerateMetaFileContents()

        self.assertEqual(ret_code, 0)
        self.assertEqual(len(files_read), 1)
        self.assertIn(
            str(package_manifest_path.relative_to(self.build_dir)), files_read
        )

        self.assertIn("packages/test_package/meta.json", generator._meta_files)
        pkg_meta = generator._meta_files["packages/test_package/meta.json"]
        self.assertEqual(pkg_meta["name"], "a_package")
        self.assertEqual(pkg_meta["type"], "package")
        self.assertEqual(len(pkg_meta["variants"]), 1)
        variant = pkg_meta["variants"][0]
        self.assertEqual(variant["api_level"], "NEXT")
        self.assertEqual(variant["arch"], "arm64")
        self.assertEqual(
            variant["manifest_file"],
            "packages/a_package/arm64-api-NEXT/release/package_manifest.json",
        )
        self.assertEqual(
            variant["files"],
            [
                "packages/a_package/arm64-api-NEXT/release/package_manifest.json",
                "packages/blobs/merkle1",
            ],
        )

        # Check for additional JSON file (the rewritten package manifest)
        rewritten_manifest_path = variant["manifest_file"]
        self.assertIn(rewritten_manifest_path, generator._json_files)
        rewritten_manifest_content = generator._json_files[
            rewritten_manifest_path
        ]
        self.assertEqual(
            rewritten_manifest_content["package"]["name"], "a_package"
        )  # Name should be updated
        self.assertEqual(len(rewritten_manifest_content["blobs"]), 1)
        self.assertTrue(
            rewritten_manifest_content["blobs"][0]["source_path"].startswith(
                "../../../blobs"
            )
        )

        # Check for package atom files (blobs)
        self.assertIn(
            "packages/blobs/merkle1",
            generator._package_atom_files,
        )
        self.assertEqual(
            generator._package_atom_files["packages/blobs/merkle1"],
            "path/to/blob1",
        )

    @mock.patch("os.mkdir")
    @mock.patch("io.open", new_callable=mock.mock_open)
    @mock.patch("os.symlink")
    def test_write_idk_contents_to_directory(
        self,
        mock_symlink: mock.Mock,
        mock_open: mock.Mock,
        mock_mkdir: mock.Mock,
    ) -> None:
        # Prepare generator with some data
        self._write_build_file("src/file1.txt", "")
        self._write_build_file("src/file2.dat", "")
        self._write_build_file("pkg/blobA", "")

        generator = IdkGenerator([], self.build_dir, self.source_dir)
        generator.collection_meta_path = "meta/manifest.json"
        generator._meta_files = {
            "meta/manifest.json": {"id": "1.2.3.4", "parts": []},
            "data/foo/meta.json": {"name": "foo", "type": "data"},
        }
        generator._json_files = {
            "packages/bar/manifest.json": {"name": "bar_pkg_manifest"}
        }
        generator._atom_files = {"data/foo/file1.txt": "src/file1.txt"}
        generator._package_atom_files = {"packages/blobs/blobA": "pkg/blobA"}

        (
            ret_code,
            additional_written_files,
        ) = generator.WriteIdkContentsToDirectory(self.output_dir)
        self.assertEqual(ret_code, 0)

        # Verify that directories were created for expected files.
        made_dirs = {call.args[0] for call in mock_mkdir.call_args_list}
        self.assertIn(
            self.output_dir / "meta", made_dirs
        )  # For meta/manifest.json
        self.assertIn(
            self.output_dir / "data/foo", made_dirs
        )  # For data/foo/meta.json
        self.assertIn(
            self.output_dir / "packages/bar", made_dirs
        )  # For packages/bar/manifest.json and file1.txt
        self.assertIn(
            self.output_dir / "packages/blobs", made_dirs
        )  # For packages/blobs/blobA (symlink parent)

        # Verify that expected files were opened for writing.
        written_file_paths = {
            call.args[0]
            for call in mock_open.call_args_list
            if call.args[1] == "w"
        }
        self.assertIn(
            self.output_dir / "meta/manifest.json", written_file_paths
        )
        self.assertIn(
            self.output_dir / "data/foo/meta.json", written_file_paths
        )
        self.assertIn(
            self.output_dir / "packages/bar/manifest.json", written_file_paths
        )

        # Verify symlink calls.
        expected_symlink_calls = [
            mock.call(
                os.path.relpath(
                    self.build_dir / "src/file1.txt",
                    self.output_dir / "data/foo",
                ),
                self.output_dir / "data/foo/file1.txt",
            ),
            mock.call(
                os.path.relpath(
                    self.build_dir / "pkg/blobA",
                    self.output_dir / "packages/blobs",
                ),
                self.output_dir / "packages/blobs/blobA",
            ),
        ]
        mock_symlink.assert_has_calls(expected_symlink_calls, any_order=True)

        # Verify additional_written_files
        assert len(additional_written_files) == 2
        self.assertIn(
            str(self.output_dir / "data/foo/meta.json"),
            additional_written_files,
        )
        self.assertIn(
            str(self.output_dir / "packages/bar/manifest.json"),
            additional_written_files,
        )
        self.assertNotIn(
            str(self.output_dir / "meta/manifest.json"),
            additional_written_files,
        )  # Collection manifest excluded

    def test_verify_dependency_relationships_valid(self) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            self._SIMPLE_FIDL_LIBRARY_INFO,
            {
                "atom_id": "sdk://pkg/test_cc",
                "atom_label": "//sdk/lib/test:test_cc_sdk",
                "atom_type": "cc_source_library",
                "category": "partner",
                "meta_dest": "pkg/test_cc_source/meta.json",
                "idk_name": "test_cc_source",
                "prebuild_info": {
                    "file_base": "pkg/test_cc_source",
                    "headers": [],
                    "include_dir": "include",
                    "sources": [],
                },
                "atom_deps": [self._SIMPLE_FIDL_LIBRARY_INFO["atom_label"]],
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        ret_code, files_read = generator.GenerateMetaFileContents()
        self.assertEqual(ret_code, 0)
        self.assertEqual(files_read, set())

    # This also covers the case that there are no deps on the incompatible
    # atoms and that multiple violations are reported.
    # This test uses the "internal" category to bypass the check that only
    # atoms in the "partner" category are included, which would prevent this
    # case from being tested.
    def test_verify_dependency_relationships_category_collection_incompatible(
        self,
    ) -> None:
        simple_fidl_with_internal_category = dict(
            self._SIMPLE_FIDL_LIBRARY_INFO
        )
        simple_fidl_with_internal_category["category"] = "internal"
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            simple_fidl_with_internal_category,
            {
                "atom_id": "sdk://pkg/test_cc",
                "atom_label": "//sdk/lib/test:test_cc_sdk",
                "atom_type": "cc_source_library",
                "category": "internal",
                "meta_dest": "pkg/test_cc_source/meta.json",
                "idk_name": "test_cc_source",
                "prebuild_info": {
                    "file_base": "pkg/test_cc_source",
                    "headers": [],
                    "include_dir": "include",
                    "sources": [],
                },
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            """Violations detected in collection `//test_idk:collection`:
"sdk://pkg/fuchsia.simple" has publication level "internal", which is incompatible with "partner".
"sdk://pkg/test_cc" has publication level "internal", which is incompatible with "partner".""",
        ):
            generator.GenerateMetaFileContents(include_internal_atoms=True)

    # This test uses the "internal" category to bypass the check that only
    # atoms in the "partner" category are included, which would prevent this
    # case from being tested.
    def test_verify_dependency_relationships_category_deps_incompatible(
        self,
    ) -> None:
        simple_fidl_with_internal_category = dict(
            self._SIMPLE_FIDL_LIBRARY_INFO
        )
        simple_fidl_with_internal_category["category"] = "internal"
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            simple_fidl_with_internal_category,
            {
                "atom_id": "sdk://pkg/test_cc",
                "atom_label": "//sdk/lib/test:test_cc_sdk",
                "atom_type": "cc_source_library",
                "category": "partner",
                "meta_dest": "pkg/test_cc_source/meta.json",
                "idk_name": "test_cc_source",
                "prebuild_info": {
                    "file_base": "pkg/test_cc_source",
                    "headers": [],
                    "include_dir": "include",
                    "sources": [],
                },
                "atom_deps": [simple_fidl_with_internal_category["atom_label"]],
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            """Violations detected in atom `//sdk/lib/test:test_cc_sdk`:
"sdk://pkg/fuchsia.simple" has publication level "internal", which is incompatible with "partner".""",
        ):
            generator.GenerateMetaFileContents(include_internal_atoms=True)

    # Atoms in the other categories are filtered, so they are not detected as
    # violations. This is a potential area for improvement.
    def test_verify_dependency_relationships_category_no_error_if_filtered(
        self,
    ) -> None:
        simple_fidl_with_prebuilt_category = dict(
            self._SIMPLE_FIDL_LIBRARY_INFO
        )
        simple_fidl_with_prebuilt_category["category"] = "prebuilt"
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            simple_fidl_with_prebuilt_category,
            {
                "atom_id": "sdk://pkg/test_cc",
                "atom_label": "//sdk/lib/test:test_cc_sdk",
                "atom_type": "cc_source_library",
                "category": "host_tool",
                "meta_dest": "pkg/test_cc_source/meta.json",
                "idk_name": "test_cc_source",
                "prebuild_info": {
                    "file_base": "pkg/test_cc_source",
                    "headers": [],
                    "include_dir": "include",
                    "sources": [],
                },
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        ret_code, files_read = generator.GenerateMetaFileContents()
        self.assertEqual(ret_code, 0)
        self.assertEqual(files_read, set())

    def test_verify_dependency_relationships_bind_invalid_deps(self) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            self._SIMPLE_CC_SOURCE_LIBRARY_INFO,
            {
                "atom_id": "sdk://pkg/test_bind",
                "atom_label": "//sdk/bind/fuchsia.test:fuchsia.test_bind_sdk",
                "atom_type": "bind_library",
                "category": "partner",
                "meta_dest": "bind/fuchsia.test/meta.json",
                "idk_name": "fuchsia.test",
                "prebuild_info": {
                    "file_base": "bind/fuchsia.test",
                },
                "atom_deps": [
                    self._SIMPLE_CC_SOURCE_LIBRARY_INFO["atom_label"]
                ],
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            "ERROR: 'bind_library' atom '//sdk/bind/fuchsia.test:fuchsia.test_bind_sdk' has a dependency on '//sdk/lib/simple:simple_cc_sdk' of type 'cc_source_library', which is not allowed.",
        ):
            generator.GenerateMetaFileContents()

    def test_verify_dependency_relationships_bind_without_api_area(
        self,
    ) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {
                "atom_id": "sdk://pkg/test_bind",
                "atom_label": "//sdk/bind/fuchsia.test:fuchsia.test_bind_sdk",
                "atom_type": "bind_library",
                "category": "partner",
                "meta_dest": "bind/fuchsia.test/meta.json",
                "idk_name": "fuchsia.test",
                "prebuild_info": {
                    "file_base": "bind/fuchsia.test",
                },
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            """Violations detected in atom `//sdk/bind/fuchsia.test:fuchsia.test_bind_sdk`:
sdk://pkg/test_bind must specify an API area. Valid areas: \\['Bluetooth', 'Component Framework', .*, 'WLAN', 'Unknown'\\]""",
        ):
            generator.GenerateMetaFileContents()

    def test_verify_dependency_relationships_cc_prebuilt_invalid_deps(
        self,
    ) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            self._SIMPLE_DATA_INFO,
            {
                "atom_id": "sdk://pkg/test_cc",
                "atom_label": "//sdk/lib/test:test_cc_sdk",
                "atom_type": "cc_prebuilt_library",
                "category": "partner",
                "meta_dest": "pkg/test_cc/meta.json",
                "idk_name": "test_cc",
                "prebuild_info": {
                    "file_base": "pkg/test_cc",
                    "headers": [],
                    "include_dir": "include",
                    "sources": [],
                },
                "atom_deps": [self._SIMPLE_DATA_INFO["atom_label"]],
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            "ERROR: 'cc_prebuilt_library' atom '//sdk/lib/test:test_cc_sdk' has a dependency on '//sdk/data/invalid:some_data_sdk' of type 'data', which is not allowed.",
        ):
            generator.GenerateMetaFileContents()

    def test_verify_dependency_relationships_cc_source_invalid_deps(
        self,
    ) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            self._SIMPLE_DATA_INFO,
            {
                "atom_id": "sdk://pkg/test_cc",
                "atom_label": "//sdk/lib/test:test_cc_sdk",
                "atom_type": "cc_source_library",
                "category": "partner",
                "meta_dest": "pkg/test_cc/meta.json",
                "idk_name": "test_cc",
                "prebuild_info": {
                    "file_base": "pkg/test_cc",
                    "headers": [],
                    "include_dir": "include",
                    "sources": [],
                },
                "atom_deps": [self._SIMPLE_DATA_INFO["atom_label"]],
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            "ERROR: 'cc_source_library' atom '//sdk/lib/test:test_cc_sdk' has a dependency on '//sdk/data/invalid:some_data_sdk' of type 'data', which is not allowed.",
        ):
            generator.GenerateMetaFileContents()

    def test_verify_dependency_relationships_fidl_invalid_deps(self) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            self._SIMPLE_CC_SOURCE_LIBRARY_INFO,
            {
                "atom_id": "sdk://pkg/test_fidl",
                "atom_label": "//sdk/fidl/fuchsia.test:fuchsia.test_fidl_sdk",
                "atom_type": "fidl_library",
                "category": "partner",
                "meta_dest": "fidl/fuchsia.test/meta.json",
                "idk_name": "fuchsia.test",
                "prebuild_info": {
                    "file_base": "fidl/fuchsia.test",
                },
                "atom_deps": [
                    self._SIMPLE_CC_SOURCE_LIBRARY_INFO["atom_label"]
                ],
                "atom_files": [],
                "is_stable": True,
                # API area is also missing.
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            "ERROR: 'fidl_library' atom '//sdk/fidl/fuchsia.test:fuchsia.test_fidl_sdk' has a dependency on '//sdk/lib/simple:simple_cc_sdk' of type 'cc_source_library', which is not allowed.",
        ):
            generator.GenerateMetaFileContents()

    def test_verify_dependency_relationships_fidl_without_api_area(
        self,
    ) -> None:
        simple_fidl_without_api_area = dict(self._SIMPLE_FIDL_LIBRARY_INFO)
        del simple_fidl_without_api_area["api_area"]

        # The ordering is important to cause two violations to be reported.
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {
                "atom_id": "sdk://pkg/test_fidl",
                "atom_label": "//sdk/fidl/fuchsia.test:fuchsia.test_fidl_sdk",
                "atom_type": "fidl_library",
                "category": "partner",
                "meta_dest": "fidl/fuchsia.test/meta.json",
                "idk_name": "fuchsia.test",
                "prebuild_info": {
                    "file_base": "fidl/fuchsia.test",
                },
                "atom_deps": [self._SIMPLE_FIDL_LIBRARY_INFO["atom_label"]],
                "atom_files": [],
                "is_stable": True,
            },
            simple_fidl_without_api_area,
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError,
            """Violations detected in atom `//sdk/fidl/fuchsia.test:fuchsia.test_fidl_sdk`:
sdk://pkg/test_fidl must specify an API area. Valid areas: \\['Bluetooth', 'Component Framework', .*, 'WLAN', 'Unknown'\\]
sdk://pkg/fuchsia.simple must specify an API area. Valid areas: \\['Bluetooth', 'Component Framework', .*, 'WLAN', 'Unknown'\\]""",
        ):
            generator.GenerateMetaFileContents()

    def test_verify_dependency_relationships_no_prebuild_info(self) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            {  # This atom has no "prebuild_info", so it should be skipped
                "atom_id": "sdk://pkg/no_prebuild",
                "atom_label": "//sdk/partner/no_prebuild",
                "atom_type": "data",
                "category": "partner",
                "meta_dest": "partner/no_prebuild/meta.json",
                "meta_contents": {"name": "no_prebuild", "type": "data"},
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        ret_code, files_read = generator.GenerateMetaFileContents()
        self.assertEqual(ret_code, 0)
        self.assertEqual(files_read, set())

    def test_verify_dependency_relationships_deps_in_unexpected_atom_type(
        self,
    ) -> None:
        manifest: list[AtomInfo] = [
            self._COLLECTION_INFO,
            self._SIMPLE_FIDL_LIBRARY_INFO,
            {
                "atom_id": "sdk://pkg/data_with_deps",
                "atom_label": "//sdk/data:data_with_deps_sdk",
                "atom_type": "data",
                "category": "partner",
                "meta_dest": "data/data_with_deps/meta.json",
                "meta_contents": {"name": "data_with_deps", "type": "data"},
                # Data atoms shouldn't have deps
                "atom_deps": [self._SIMPLE_FIDL_LIBRARY_INFO["atom_label"]],
                "atom_files": [],
                "is_stable": True,
            },
        ]
        generator = IdkGenerator(manifest, self.build_dir, self.source_dir)
        with self.assertRaisesRegex(
            AssertionError, "Unexpected atom type with deps: data"
        ):
            generator.GenerateMetaFileContents()


class RelativeToWalkUpTest(unittest.TestCase):
    def test_relative_to_walk_up(self) -> None:
        self.assertEqual(
            relative_to_walk_up(Path("/a/b"), Path("/a/c")), Path("../b")
        )
        self.assertEqual(
            relative_to_walk_up(Path("/a/b/c"), Path("/a/d/e")),
            Path("../../b/c"),
        )
        self.assertEqual(
            relative_to_walk_up(Path("/a/b"), Path("/a/b")), Path(".")
        )
        self.assertEqual(
            relative_to_walk_up(Path("/a/b"), Path("/a/b/c")), Path("..")
        )
        self.assertEqual(
            relative_to_walk_up(Path("/a/b/c"), Path("/a/b")), Path("c")
        )


# When run from GN as a compiled pyz file, `__file__` is no longer at the same
# location relative to the Fuchsia source directory. In this case, relative path
# from the target gen directory to the source directory is passed in. In that
# case, this function:
# 1. Pops this argument before running the unit tests.
# 2. Gets the absolute path to the target gen dir based on `__file__`.
# 3. Applies (1) to (2) to get the absolute path to Fuchsia source directory.
# Note that an absolute path cannot be passed from GN since bots may run tests
# from different directories than where they were built (while maintaining the
# same structure).
# LINT.IfChange(gn_test_main)
def gn_test_main() -> None:
    if len(sys.argv) > 1:
        fuchsia_source_relative_to_target_gen_dir = sys.argv.pop()
        # Drop this part of the path:
        # `idk_generator_unittest/idk_generator_unittest.pyz/idk_generator_unittest.py`.
        absolute_target_gen_dir = Path(__file__).parent.parent.parent
        fuchsia_source_absolute_path = Path(
            os.path.join(
                absolute_target_gen_dir,
                fuchsia_source_relative_to_target_gen_dir,
            )
        )
        IdkGeneratorTest._FUCHSIA_SOURCE_DIR = fuchsia_source_absolute_path
    unittest.main()


# LINT.ThenChange(BUILD.gn:gn_test_main)

if __name__ == "__main__":
    unittest.main()
