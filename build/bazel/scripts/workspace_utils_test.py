#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path
from textwrap import dedent

sys.path.insert(0, os.path.dirname(__file__))
import workspace_utils
from workspace_utils import BazelrcFromGnConfigGenerator, GnBuildArgs


class TestWorkspaceShouldExcludeFile(unittest.TestCase):
    def test_workspace_should_exclude_file(self) -> None:
        _EXPECTED_EXCLUDED_PATHS = [
            "out",
            ".jiri",
            ".fx",
            ".git",
            "bazel-bin",
            "bazel-repos",
            "bazel-out",
            "bazel-workspace",
        ]
        for path in _EXPECTED_EXCLUDED_PATHS:
            self.assertTrue(
                workspace_utils.workspace_should_exclude_file(path),
                msg=f"For path [{path}]",
            )

        _EXPECTED_INCLUDED_PATHS = [
            "out2",
            "src",
            ".clang-format",
            ".gn",
        ]
        for path in _EXPECTED_INCLUDED_PATHS:
            self.assertFalse(
                workspace_utils.workspace_should_exclude_file(path),
                msg=f"For path [{path}]",
            )


class TestGeneratedWorkspaceFiles(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.out = Path(self._td.name)
        (self.out / "elephant").write_text("trumpet!")
        self.input_file_path = self.out / "input_file"
        self.input_file_path.write_text("input")

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_with_no_file_hasher(self) -> None:
        ws_files = workspace_utils.GeneratedWorkspaceFiles()
        ws_files.record_file_content("zoo/lion", "roar!")
        ws_files.record_symlink("zoo/elephant", self.out / "elephant")
        ws_files.record_input_file_hash("no/such/file/exists")
        input_content = ws_files.read_text_file(self.input_file_path)

        expected_json = r"""{
  "zoo/elephant": {
    "target": "@OUT@/elephant",
    "type": "symlink"
  },
  "zoo/lion": {
    "content": "roar!",
    "type": "file"
  }
}""".replace(
            "@OUT@", str(self.out)
        )

        self.assertEqual(ws_files.to_json(), expected_json)
        self.assertEqual(ws_files.input_files, set([self.input_file_path]))
        self.assertEqual(input_content, "input")

        ws_files.write(self.out / "workspace")
        self.assertEqual(
            (self.out / "workspace" / "zoo" / "lion").read_text(), "roar!"
        )
        self.assertEqual(
            (self.out / "workspace" / "zoo" / "elephant").read_text(),
            "trumpet!",
        )
        self.assertEqual(
            str((self.out / "workspace" / "zoo" / "elephant").readlink()),
            "../../elephant",
        )

    def test_with_file_hasher(self) -> None:
        ws_files = workspace_utils.GeneratedWorkspaceFiles()
        ws_files.set_file_hasher(lambda path: f"SHA256[{path}]")
        ws_files.record_file_content("zoo/lion", "roar!")
        ws_files.record_symlink("zoo/elephant", self.out / "elephant")
        ws_files.record_input_file_hash("no/such/file/exists")
        input_content = ws_files.read_text_file(self.input_file_path)

        expected_json = r"""{
  "@INPUT_FILE_PATH@": {
    "hash": "SHA256[@INPUT_FILE_PATH@]",
    "type": "input_file"
  },
  "no/such/file/exists": {
    "hash": "SHA256[no/such/file/exists]",
    "type": "input_file"
  },
  "zoo/elephant": {
    "target": "@OUT@/elephant",
    "type": "symlink"
  },
  "zoo/lion": {
    "content": "roar!",
    "type": "file"
  }
}""".replace(
            "@OUT@", str(self.out)
        ).replace(
            "@INPUT_FILE_PATH@", str(self.input_file_path)
        )

        self.assertEqual(ws_files.to_json(), expected_json)
        self.assertEqual(ws_files.input_files, set([self.input_file_path]))
        self.assertEqual(input_content, "input")

        ws_files.write(self.out / "workspace")
        self.assertEqual(
            (self.out / "workspace" / "zoo" / "lion").read_text(), "roar!"
        )
        self.assertEqual(
            (self.out / "workspace" / "zoo" / "elephant").read_text(),
            "trumpet!",
        )
        self.assertEqual(
            str((self.out / "workspace" / "zoo" / "elephant").readlink()),
            "../../elephant",
        )
        self.assertFalse(
            (
                self.out / "workspace" / "no" / "such" / "file" / "exists"
            ).exists()
        )

    def test_update_if_needed(self) -> None:
        ws_files = workspace_utils.GeneratedWorkspaceFiles()
        ws_files.set_file_hasher(lambda path: f"SHA256[{path}]")
        ws_files.record_file_content("zoo/lion", "roar!")
        ws_files.record_symlink("zoo/elephant", self.out / "elephant")
        ws_files.record_input_file_hash("no/such/file/exists")

        ws_dir = self.out / "workspace"
        ws_manifest = self.out / "manifest"

        # The manifest file does not exist, so update the directory.
        self.assertTrue(ws_files.update_if_needed(ws_dir, ws_manifest))

        # A second call with the same inputs should do nothing.
        self.assertFalse(ws_files.update_if_needed(ws_dir, ws_manifest))

        # Modify the manifest file to an empty dict, verify that the output
        # directory is not empty.
        ws_manifest.write_text("{}")
        self.assertTrue(ws_files.update_if_needed(ws_dir, ws_manifest))
        self.assertListEqual(os.listdir(ws_dir), ["zoo"])

        # Now update the workspace to be empty. Verify that the manifest is now just "{}"
        # and that the output directory is empty.
        empty_files = workspace_utils.GeneratedWorkspaceFiles()
        self.assertTrue(empty_files.update_if_needed(ws_dir, ws_manifest))
        self.assertEqual(ws_manifest.read_text(), "{}")
        self.assertListEqual(os.listdir(ws_dir), [])


class RemoveDirTests(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = self._td.name

    def tearDown(self) -> None:
        self._td.cleanup()

    def _run_checks(
        self, test_name: str, top_dir: str, all_paths: set[str]
    ) -> None:
        for p in all_paths:
            self.assertTrue(
                os.path.exists(p),
                f"{test_name} setup failure: {p} does not exist after test dir creation",
            )

        workspace_utils.remove_dir(top_dir)

        for p in all_paths:
            self.assertFalse(
                os.path.exists(p),
                f"{test_name}: {p} still exists after remove_dir({top_dir})",
            )

    def test_remove_empty_dir(self) -> None:
        dir = tempfile.mkdtemp(dir=self._root)
        self._run_checks("empty_dir", dir, {dir})

    def test_dir_with_file(self) -> None:
        dir = tempfile.mkdtemp(dir=self._root)
        _, f = tempfile.mkstemp(dir=dir)
        self._run_checks("dir_with_file", dir, {dir, f})

    def test_dir_with_symlink(self) -> None:
        dir = str(tempfile.mkdtemp(dir=self._root))
        _, f = tempfile.mkstemp(dir=dir)
        l = f"{f}_link"
        os.symlink(f, l)
        self._run_checks("dir_with_symlink", dir, {dir, f, l})

    def test_dir_with_subdir_symlink(self) -> None:
        dir = tempfile.mkdtemp(dir=self._root)
        subdir = tempfile.mkdtemp(dir=dir)
        _, f = tempfile.mkstemp(dir=subdir)
        l = f"{subdir}_link"
        os.symlink(subdir, l, target_is_directory=True)
        self._run_checks("dir_with_subdir_symlink", dir, {dir, subdir, f, l})


class GnBuildArgsTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = Path(self._td.name)
        self._build_dir = tempfile.TemporaryDirectory()
        self._build_root = Path(self._build_dir.name)

    def tearDown(self) -> None:
        self._td.cleanup()
        self._build_dir.cleanup()

    def test_find_all_gn_build_variables_for_bazel(self) -> None:
        # First, a ValueError is raised if the file for main build is missing.
        with self.assertRaises(ValueError) as cm:
            GnBuildArgs.find_all_gn_build_variables_for_bazel(
                self._root, self._build_root
            )

        self.assertEqual(
            str(cm.exception),
            f"Missing required build arguments file: {self._build_root}/gn_build_variables_for_bazel.json",
        )

        # Second, create a main `gn_build_variables_for_bazel.json`` file, and
        # a vendor-specific one.
        main_file_path = self._build_root / "gn_build_variables_for_bazel.json"
        main_file_path.write_text("foo!")

        vendor_source_path = self._root / "vendor/alice&bob"
        vendor_source_path.mkdir(parents=True)
        vendor_file_path = (
            self._build_root
            / "vendor_alice&bob_gn_build_variables_for_bazel.json"
        )
        vendor_file_path.write_text("BAR?")

        relative_paths = GnBuildArgs.find_all_gn_build_variables_for_bazel(
            self._root, self._build_root
        )
        self.assertEqual(
            relative_paths,
            [
                "gn_build_variables_for_bazel.json",
                "vendor_alice&bob_gn_build_variables_for_bazel.json",
            ],
        )

    def test_generate_args_bzl(self) -> None:
        gn_args_to_export: list[dict[str, T.Any]] = [
            {
                "location": "//bob.gni",
                "name": "foo",
                "type": "bool",
                "value": True,
            },
            {
                "location": "//alice.gni",
                "name": "bar",
                "type": "string",
                "value": "some string",
            },
            {
                "location": "//alice.gni",
                "name": "zoo",
                "type": "string_or_false",
                "value": False,
            },
            {
                "location": "//alice.gni",
                "name": "zoo2",
                "type": "string_or_false",
                "value": "non-false string",
            },
            {
                "location": "//build/baz.gni",
                "name": "baz",
                "type": "array_of_strings",
                "value": ["1", "2", "3", "40", "fuzz"],
            },
            {
                "location": "//build/fuzz.gni",
                "name": "fuzz",
                "type": "path",
                "value": "//prebuilt/third_party/fuzz",
            },
            {
                "location": "//build/fizz.gni",
                "name": "absolute_fizz",
                "type": "path",
                "value": "/path/to/prebuilt/third_party/fuzz",
            },
        ]

        args_bzl = GnBuildArgs.generate_args_bzl(
            gn_args_to_export, Path("path/to/gn_build_variables_for_bazel.json")
        )
        self.assertEqual(
            args_bzl,
            r'''# AUTO-GENERATED BY FUCHSIA BUILD - DO NOT EDIT
# Variables listed from path/to/gn_build_variables_for_bazel.json

"""A subset of GN args that are needed in the Bazel build."""

# From //bob.gni
foo = True

# From //alice.gni
bar = "some string"

# From //alice.gni
zoo = ""

# From //alice.gni
zoo2 = "non-false string"

# From //build/baz.gni
baz = ['1', '2', '3', '40', 'fuzz']

# From //build/fuzz.gni
fuzz = "prebuilt/third_party/fuzz"

# From //build/fizz.gni
absolute_fizz = "/path/to/prebuilt/third_party/fuzz"
''',
        )

    def test_record_fuchsia_build_info_dir(self) -> None:
        generated = workspace_utils.GeneratedWorkspaceFiles()

        main_args_path = self._build_root / "gn_build_variables_for_bazel.json"
        main_args_path.write_text(
            json.dumps(
                [
                    {
                        "location": "//bob.gni",
                        "name": "foo",
                        "type": "bool",
                        "value": True,
                    },
                ]
            )
        )

        vendor_source_path = self._root / "vendor/alice"
        vendor_source_path.mkdir(parents=True)
        vendor_args_path = (
            self._build_root / "vendor_alice_gn_build_variables_for_bazel.json"
        )
        vendor_args_path.write_text(
            json.dumps(
                [
                    {
                        "location": "//alice.gni",
                        "name": "bar",
                        "type": "string",
                        "value": "some string",
                    },
                    {
                        "location": "//alice.gni",
                        "name": "zoo",
                        "type": "string_or_false",
                        "value": False,
                    },
                    {
                        "location": "//alice.gni",
                        "name": "zoo2",
                        "type": "string_or_false",
                        "value": "non-false string",
                    },
                ]
            )
        )

        bazel_test_suites_path = self._build_root / "bazel_test_suites.txt"
        bazel_test_suites_path.write_text(
            "\n".join(
                [
                    "//fake/test_suite:1",
                    "//fake/test_suite:2",
                ]
            )
        )

        GnBuildArgs.record_fuchsia_build_info_dir(
            self._root, self._build_root, generated
        )

        generated_json = json.loads(generated.to_json())

        EXPECTED_ARGS_BZL = r'''# AUTO-GENERATED BY FUCHSIA BUILD - DO NOT EDIT
# Variables listed from {}/gn_build_variables_for_bazel.json

"""A subset of GN args that are needed in the Bazel build."""

# From //bob.gni
foo = True
'''.format(
            self._build_root
        )

        EXPECTED_ALICE_ARGS_BZL = r'''# AUTO-GENERATED BY FUCHSIA BUILD - DO NOT EDIT
# Variables listed from {}/vendor_alice_gn_build_variables_for_bazel.json

"""A subset of GN args that are needed in the Bazel build."""

# From //alice.gni
bar = "some string"

# From //alice.gni
zoo = ""

# From //alice.gni
zoo2 = "non-false string"
'''.format(
            self._build_root
        )

        EXPECTED_MODULE_BAZEL = """\
module(name = "fuchsia_build_info", version = "1")

bazel_dep(name = "platforms", version = "1.0.0")
bazel_dep(name = "fuchsia_rules_common", version = "0.0")
"""

        EXPECTED_DEFAULT_BUILD_FLAGS_BUILD_BAZEL = f"""\
# Default build_flags() definition for C++ and Rust toolchains.

load("@fuchsia_rules_common//build_flags:toolchain.bzl", "build_flags_toolchain_instance")
load("@@//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")

build_flags_toolchain_instance(
    name = "fuchsia_default_build_flags",
    cxx_common_build_flags = [],
    cxx_executable_build_flags = [],
    cxx_shared_library_build_flags = [],
    rust_common_build_flags = [],
    rust_executable_build_flags = [],
    rust_shared_library_build_flags = [],
)

toolchain(
    name = "fuchsia_toolchain",
    target_compatible_with = ["@platforms//os:fuchsia"],
    toolchain = ":fuchsia_default_build_flags",
    toolchain_type = "@fuchsia_rules_common//build_flags:toolchain_type",
)

build_flags_toolchain_instance(
    name = "host_default_build_flags",
    cxx_common_build_flags = [],
    cxx_executable_build_flags = [],
    cxx_shared_library_build_flags = [],
    rust_common_build_flags = [],
    rust_executable_build_flags = [],
    rust_shared_library_build_flags = [],
)

toolchain(
    name = "host_toolchain",
    target_compatible_with = HOST_OS_CONSTRAINTS,
    toolchain = ":host_default_build_flags",
    toolchain_type = "@fuchsia_rules_common//build_flags:toolchain_type",
)
"""
        self.maxDiff = (
            None  # Ensure large dictionary differences are properly printed.
        )

        self.assertDictEqual(
            generated_json,
            {
                "BUILD.bazel": {
                    "content": "",
                    "type": "file",
                },
                "MODULE.bazel": {
                    "content": EXPECTED_MODULE_BAZEL,
                    "type": "file",
                },
                "args.bzl": {
                    "content": EXPECTED_ARGS_BZL,
                    "type": "file",
                },
                "vendor_alice_args.bzl": {
                    "content": EXPECTED_ALICE_ARGS_BZL,
                    "type": "file",
                },
                "default_build_flags/BUILD.bazel": {
                    "content": EXPECTED_DEFAULT_BUILD_FLAGS_BUILD_BAZEL,
                    "type": "file",
                },
            },
        )


class GnTargetsDirTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = Path(self._td.name)

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_simple(self) -> None:
        build_dir = self._root / "build"
        build_dir.mkdir()

        entries = workspace_utils.BazelPackageAndTargetToGnInputsEntriesMap(
            {
                "src/drivers/virtio": workspace_utils.BazelTargetGnInputsEntriesMap(
                    {
                        "package": workspace_utils.GnTargetsDirectoryManifestEntry(
                            bazel_name="package",
                            bazel_package="src/drivers/virtio",
                            generator_label="//src/drivers/virtio:package-archive(//build/toolchain/fuchsia:x64)",
                            output_files=["obj/src/drivers/virtio/package.far"],
                            license_spdx_file="obj/src/drivers/virtio/package-archive.licenses.spdx.json",
                        ),
                    }
                ),
                "bundles/assembly": workspace_utils.BazelTargetGnInputsEntriesMap(
                    {
                        "eng.bazel_inputs": workspace_utils.GnTargetsDirectoryManifestEntry(
                            bazel_name="eng.bazel_inputs",
                            bazel_package="bundles/assembly",
                            generator_label="//bundles/assembly:eng.platform_artifacts(//build/toolchain/fuchsia:x64)",
                            output_directory="obj/bundles/assembly/eng/platform_artifacts",
                            license_spdx_file="obj/bundles/assembly/eng/platform_artifacts/eng.platform_artifacts.licenses.spdx.json",
                        ),
                    }
                ),
            }
        )

        all_licenses_path = self._root / "all_licenses.spdx.json"
        all_licenses_path.write_text("")

        generated = workspace_utils.GeneratedWorkspaceFiles()
        workspace_utils.record_gn_targets_dir_from_entries(
            generated, build_dir, entries, all_licenses_path
        )

        generated_json = json.loads(generated.to_json())
        self.maxDiff = None
        self.assertListEqual(
            sorted(generated_json.keys()),
            [
                "BUILD.bazel",
                "MODULE.bazel",
                "_files/obj/bundles/assembly/eng/platform_artifacts",
                "_files/obj/bundles/assembly/eng/platform_artifacts/eng.platform_artifacts.licenses.spdx.json",
                "_files/obj/src/drivers/virtio/package-archive.licenses.spdx.json",
                "_files/obj/src/drivers/virtio/package.far",
                "all_license_files.txt",
                "all_licenses.spdx.json",
                "bundles/assembly/BUILD.bazel",
                "bundles/assembly/_files",
                "src/drivers/virtio/BUILD.bazel",
                "src/drivers/virtio/_files",
            ],
        )

        self.assertEqual(
            generated_json["BUILD.bazel"]["content"],
            dedent(
                """\
            # AUTO-GENERATED - DO NOT EDIT
            load("@rules_license//rules:license.bzl", "license")

            # This contains information about all the licenses of all
            # Ninja outputs exposed in this repository.
            # IMPORTANT: package_name *must* be "Legacy Ninja Build Outputs"
            # as several license pipeline exception files hard-code this under //vendor/...
            license(
                name = "all_licenses_spdx_json",
                package_name = "Legacy Ninja Build Outputs",
                license_text = "all_licenses.spdx.json",
                visibility = ["//visibility:public"]
            )
            """
            ),
        )

        self.assertEqual(
            generated_json["MODULE.bazel"]["content"],
            dedent(
                """\
                # AUTO-GENERATED - DO NOT EDIT

                module(name = "gn_targets", version = "1")

                bazel_dep(name = "rules_license", version = "1.0.0")"""
            ),
        )

        self.assertDictEqual(
            generated_json["all_licenses.spdx.json"],
            {
                "target": str(all_licenses_path.resolve()),
                "type": "symlink",
            },
        )

        self.assertDictEqual(
            generated_json[
                "_files/obj/src/drivers/virtio/package-archive.licenses.spdx.json"
            ],
            {
                "target": str(
                    build_dir
                    / "obj/src/drivers/virtio/package-archive.licenses.spdx.json"
                ),
                "type": "raw_symlink",
            },
        )

        self.assertDictEqual(
            generated_json["_files/obj/src/drivers/virtio/package.far"],
            {
                "target": str(build_dir / "obj/src/drivers/virtio/package.far"),
                "type": "raw_symlink",
            },
        )

        self.assertDictEqual(
            generated_json[
                "_files/obj/bundles/assembly/eng/platform_artifacts/eng.platform_artifacts.licenses.spdx.json"
            ],
            {
                "target": str(
                    build_dir
                    / "obj/bundles/assembly/eng/platform_artifacts/eng.platform_artifacts.licenses.spdx.json"
                ),
                "type": "raw_symlink",
            },
        )

        self.assertDictEqual(
            generated_json[
                "_files/obj/bundles/assembly/eng/platform_artifacts"
            ],
            {
                "target": str(
                    build_dir / "obj/bundles/assembly/eng/platform_artifacts"
                ),
                "type": "raw_symlink",
            },
        )

        self.assertDictEqual(
            generated_json["src/drivers/virtio/_files"],
            {
                "target": "../../../_files",
                "type": "raw_symlink",
            },
        )

        self.assertDictEqual(
            generated_json["bundles/assembly/_files"],
            {
                "target": "../../_files",
                "type": "raw_symlink",
            },
        )

        self.assertEqual(
            generated_json["bundles/assembly/BUILD.bazel"]["content"],
            dedent(
                """\
            # AUTO-GENERATED - DO NOT EDIT

            load("@rules_license//rules:license.bzl", "license")

            package(default_visibility = ["//visibility:public"])

            # From GN target: //bundles/assembly:eng.platform_artifacts(//build/toolchain/fuchsia:x64)
            license(
                name = "eng.bazel_inputs.license",
                package_name = "Legacy Ninja Build Outputs",
                license_text = "_files/obj/bundles/assembly/eng/platform_artifacts/eng.platform_artifacts.licenses.spdx.json",
            )
            filegroup(
                name = "eng.bazel_inputs",
                applicable_licenses = [":eng.bazel_inputs.license"],
                srcs = glob(["_files/obj/bundles/assembly/eng/platform_artifacts/**"], exclude_directories=1, allow_empty=True),
            )
            alias(
                name = "eng.bazel_inputs.directory",
                actual = "_files/obj/bundles/assembly/eng/platform_artifacts",
            )
            """
            ),
        )

        self.assertEqual(
            generated_json["src/drivers/virtio/BUILD.bazel"]["content"],
            dedent(
                """\
            # AUTO-GENERATED - DO NOT EDIT

            load("@rules_license//rules:license.bzl", "license")

            package(default_visibility = ["//visibility:public"])

            # From GN target: //src/drivers/virtio:package-archive(//build/toolchain/fuchsia:x64)
            license(
                name = "package.license",
                package_name = "Legacy Ninja Build Outputs",
                license_text = "_files/obj/src/drivers/virtio/package-archive.licenses.spdx.json",
            )
            filegroup(
                name = "package",
                applicable_licenses = [":package.license"],
                srcs = ["_files/obj/src/drivers/virtio/package.far"],
            )
            """
            ),
        )


class CheckRegeneratorInputsUpdatesTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = Path(self._td.name)
        self.build_dir = self._root / "build_dir"
        self.build_dir.mkdir()

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_missing_inputs_file(self) -> None:
        self.build_dir / "inputs.txt"
        updates = workspace_utils.check_regenerator_inputs_updates(
            self.build_dir, "inputs.txt"
        )
        self.assertSetEqual(updates, {"inputs.txt"})

    def test_no_inputs_changed(self) -> None:
        input1 = self._root / "input1"
        input1.write_text("hi")
        input2 = self._root / "input2"
        input2.write_text("hello")

        inputs_path = self.build_dir / "inputs.txt"
        inputs_path.write_text("../input1\n../input2\n")

        updates = workspace_utils.check_regenerator_inputs_updates(
            self.build_dir, "inputs.txt"
        )

        self.assertSetEqual(updates, set())

    def test_inputs_changed(self) -> None:
        input1 = self._root / "input1"
        input1.write_text("hi")
        input2 = self._root / "input2"
        input2.write_text("hello")

        inputs_path = self.build_dir / "inputs.txt"
        inputs_path.write_text("../input1\n../input2\n")

        inputs_ts = inputs_path.stat().st_mtime
        new_ts = inputs_ts + 1.5

        # Force a timestamp update on the first input file.
        os.utime(input1, times=(new_ts, new_ts))

        updates = workspace_utils.check_regenerator_inputs_updates(
            self.build_dir, "inputs.txt"
        )

        self.assertSetEqual(updates, {"../input1"})

        # Do the same for the second input file.
        os.utime(input2, times=(new_ts, new_ts))

        updates = workspace_utils.check_regenerator_inputs_updates(
            self.build_dir, "inputs.txt"
        )

        self.assertSetEqual(updates, {"../input1", "../input2"})

        # Update the inputs.txt timestamp too.
        os.utime(inputs_path, times=(new_ts, new_ts))

        updates = workspace_utils.check_regenerator_inputs_updates(
            self.build_dir, "inputs.txt"
        )

        self.assertSetEqual(updates, set())


class RepositoryNameTest(unittest.TestCase):
    def test_repository_name(self) -> None:
        for label, expected_repo_name in [
            ("@foo//path/to:target", "foo"),
            ("@@bar//:root", "bar"),
            ("@@rules_python+//path:to/target", "rules_python+"),
            ("@@rules_rust+1.3//:root", "rules_rust+1.3"),
            ("@@foo+bar+baz//extension:target", "foo+bar+baz"),
        ]:
            self.assertEqual(
                workspace_utils.repository_name(label), expected_repo_name
            )

    def test_innermost_repository_name(self) -> None:
        for label, expected_repo_name in [
            ("@foo//path/to:target", "foo"),
            ("@@bar//:root", "bar"),
            ("@@rules_python+//path:to/target", "rules_python+"),
            ("@@rules_rust+1.3//:root", "rules_rust+1.3"),
            ("@@rules_java++toolchains+local_jdk//some:path", "local_jdk"),
            (
                "@@bazel_tools+remote_coverage_tools_extension+remote_coverage_tools//:root",
                "remote_coverage_tools",
            ),
        ]:
            self.assertEqual(
                workspace_utils.innermost_repository_name(label),
                expected_repo_name,
            )


class BazelrcFromGnConfigGeneratorTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = Path(self._td.name)

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_1(self) -> None:
        # Generate mock version of the config JSON files.
        bazel_args_dir = self._root / "bazel_args"
        bazel_args_dir.mkdir()

        # Create configurations and their content.
        platforms = [
            "host",
            "fuchsia",  # Legacy alias for "fuchsia_sdk".
            "fuchsia_sdk",
            "fuchsia_platform",
            "linux_first_cpu",
            "fuchsia_sdk_second_cpu",
        ]

        configs = {
            "host": {
                "common": ["--first_flag", "--second_flag"],
                "build": ["--third_flag", "--fourth_flag"],
                "remote_build": ["--remote_flag"],
                "current_os": "linux",
                "current_cpu": "first_cpu",
            },
            "fuchsia": {
                "common": ["--common_flag"],
                "build": ["--configured_flag"],
                "remote_build": ["--remote_flag2"],
                "current_os": "fuchsia",
                "current_cpu": "second_cpu",
            },
        }
        for config_name, values in configs.items():
            config_file = bazel_args_dir / f"{config_name}.json"
            with config_file.open("w") as f:
                json.dump(values, f)

        generator = BazelrcFromGnConfigGenerator(platforms)
        output = generator.generate_bazelrc(self._root)

        self.maxDiff = None
        self.assertEqual(
            output,
            r"""# Auto-generated lists of --config=<name> settings, do not edit!
common:host_config_args --first_flag --second_flag
build:host_config_args --third_flag --fourth_flag

common:fuchsia_config_args --common_flag
build:fuchsia_config_args --configured_flag

common:host --config=host_config_args --platforms=//build/bazel/platforms:host
common:fuchsia --config=fuchsia_config_args --platforms=//build/bazel/platforms:fuchsia_sdk_second_cpu
common:fuchsia_sdk --config=fuchsia_config_args --platforms=//build/bazel/platforms:fuchsia_sdk_second_cpu
common:fuchsia_platform --config=fuchsia_config_args --platforms=//build/bazel/platforms:fuchsia_platform_second_cpu
common:linux_first_cpu --config=host_config_args --platforms=//build/bazel/platforms:linux_first_cpu
common:fuchsia_sdk_second_cpu --config=fuchsia_config_args --platforms=//build/bazel/platforms:fuchsia_sdk_second_cpu
""",
        )


class GenerateFuchsiaPlatformSysrootRepositoryTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = Path(self._td.name)
        (self._root / ".jiri_manifest").write_text("")
        self._sysroot_json_path = self._root / "sysroot.json"
        self._repository_dir = self._root / "repository"
        self._build_dir = self._root / "build_dir"
        self._build_dir.mkdir()

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_generate_fuchsia_platform_sysroot_repository_with_dist_targets(
        self,
    ) -> None:
        sysroot_entries = [
            {"source": "../src/foo.h", "dest": "include/foo.h"},
            {"source": "obj/libfoo.so", "dest": "lib/libfoo.so"},
            {"source": "user.libc_x64/libc.so", "dest": "dist/lib/ld.so.1"},
            {"source": "user.libc_x64/libc.so.debug", "dest": "debug/libc.so"},
            {
                "source": "user.libc_x64-asan/libc.so",
                "dest": "dist/lib/asan/ld.so.1",
            },
            {
                "source": "user.libc_x64-asan/libc.so.debug",
                "dest": "debug/asan/libc.so",
            },
        ]
        with self._sysroot_json_path.open("w") as f:
            json.dump(sysroot_entries, f)

        workspace_utils.generate_fuchsia_platform_sysroot_repository(
            self._repository_dir,
            "test_sysroot_repo_name",
            self._sysroot_json_path,
            "x64",
            self._build_dir,
        )

        build_bazel = self._repository_dir / "BUILD.bazel"
        self.assertTrue(build_bazel.exists())
        self.assertTrue((self._repository_dir / "LICENSE").is_symlink())

        self.maxDiff = None
        self.assertEqual(
            build_bazel.read_text(),
            """# AUTO-GENERATED - DO NOT EDIT

load(
    "@fuchsia_rules_common//debug_symbols:debug_symbols.bzl",
    "fuchsia_unstripped_binary",
)
load("@rules_license//rules:license.bzl", "license")

package(default_applicable_licenses = [":license"])

license(
    name = "license",
    license_kinds = ["@rules_license//licenses/spdx:BSD-2-Clause"],
    license_text = "LICENSE",
)

exports_files(["sysroot/empty"])

filegroup(
    name = "sysroot_header_files",
    srcs = [
        "sysroot/include/foo.h",
    ],
    visibility = ["//visibility:public"]
)

filegroup(
    name = "sysroot_library_files",
    srcs = [
        "sysroot/lib/libfoo.so",
    ],
    visibility = ["//visibility:public"]
)

fuchsia_unstripped_binary(
    name = "sysroot_library_dist",
    dest = "lib/ld.so.1",
    stripped_file = "sysroot/dist/lib/ld.so.1",
    unstripped_file = "sysroot/debug/libc.so",
    target_compatible_with = [
        "@platforms//os:fuchsia",
        "@platforms//cpu:x86_64",
    ],
    visibility = ["//visibility:public"],
)

fuchsia_unstripped_binary(
    name = "sysroot_library_dist.asan",
    dest = "lib/asan/ld.so.1",
    stripped_file = "sysroot/dist/lib/asan/ld.so.1",
    unstripped_file = "sysroot/debug/asan/libc.so",
    target_compatible_with = [
        "@platforms//os:fuchsia",
        "@platforms//cpu:x86_64",
    ],
    visibility = ["//visibility:public"],
)
""",
        )

        self.assertEqual(
            (self._repository_dir / "MODULE.bazel").read_text(),
            dedent(
                f"""\
                module(name = "test_sysroot_repo_name")

                bazel_dep(name = "fuchsia_rules_common", version = "")
                local_path_override(
                    module_name = "fuchsia_rules_common",
                    path = "{self._root}/build/bazel_sdk/fuchsia_rules_common",
                )
                bazel_dep(name = "platforms", version = "1.1.0")
                bazel_dep(name = "rules_license", version = "1.0.0")
                """
            ),
        )


if __name__ == "__main__":
    unittest.main()
