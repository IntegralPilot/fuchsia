#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit-test for generate_repository.py"""

import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(__file__))
from generate_repository import (
    OutputIdk,
    OutputPackageInfo,
    split_path_to_package_name,
)


class SplitPathToPackageNameTest(unittest.TestCase):
    def test_build_id_paths(self) -> None:
        _TEST_CASES = {
            ".build-id": ("", ".build-id"),
            ".build-id/foo": (".build-id", "foo"),
            ".build-id/xx/yyyyy": (".build-id", "xx/yyyyy"),
            ".build-id/a5/93847ef.debug": (".build-id", "a5/93847ef.debug"),
        }
        for path, expected in _TEST_CASES.items():
            self.assertEqual(split_path_to_package_name(path), expected)

    def test_arch_paths(self) -> None:
        _TEST_CASES = {
            "arch/cpu/sysroot/usr/lib/libfoo.so": (
                "arch/cpu/sysroot",
                "usr/lib/libfoo.so",
            ),
            "arch/cpu/sysroot/foo": ("arch/cpu/sysroot", "foo"),
            "arch/cpu/lib/libfoo.so": ("arch/cpu", "lib/libfoo.so"),
            "arch/cpu/libfoo.so": ("arch/cpu", "libfoo.so"),
        }
        for path, expected in _TEST_CASES.items():
            self.assertEqual(split_path_to_package_name(path), expected)

        with self.assertRaises(AssertionError) as cm:
            split_path_to_package_name("arch/libfoo.so")
        self.assertEqual(
            str(cm.exception), "Unexpected arch-related path arch/libfoo.so"
        )

    def test_packages_paths(self) -> None:
        _TEST_CASES = {
            "packages/blobs/0123456789abcdef": (
                "packages/blobs",
                "0123456789abcdef",
            ),
            "packages/heapdump-collector/x64-api-NEXT/release/package_manifest.json": (
                "packages/heapdump-collector",
                "x64-api-NEXT/release/package_manifest.json",
            ),
        }
        for path, expected in _TEST_CASES.items():
            self.assertEqual(split_path_to_package_name(path), expected)

        with self.assertRaises(AssertionError) as cm:
            split_path_to_package_name("packages/foo")
        self.assertEqual(
            str(cm.exception), "Unexpected package-related path: packages/foo"
        )


class OutputPackageInfoTest(unittest.TestCase):
    HEADER = r"""# AUTO-GENERATED - DO NOT EDIT !

package(default_visibility = ["//visibility:public"])
"""

    def test_empty(self) -> None:
        info = OutputPackageInfo()
        self.assertEqual(info.generate_build_bazel(), self.HEADER)

    def test_exports(self) -> None:
        info = OutputPackageInfo()
        info.add_export("foo_exported")
        info.add_export("bar_exported")
        self.assertEqual(
            info.generate_build_bazel(),
            self.HEADER
            + r"""
# The following are direct symlinks that can point into the Ninja output directory or the Fuchsia source dir.
exports_files([
    "bar_exported",
    "foo_exported",
])
""",
        )

    def test_filegroup(self) -> None:
        info = OutputPackageInfo()
        info.add_filegroup("foo", ["libfoo.h", "libfoo.cc"])
        info.add_filegroup("bin", ["main.cc"])
        self.assertEqual(
            info.generate_build_bazel(),
            self.HEADER
            + r"""
filegroup(
    name = "foo",
    srcs = [
        "libfoo.cc",
        "libfoo.h",
    ],
)

filegroup(
    name = "bin",
    srcs = ["main.cc"],
)
""",
        )

    def test_aliases(self) -> None:
        info = OutputPackageInfo()
        info.add_alias("foo", "foo_lib")
        info.add_alias("bin", "bin_program")
        self.assertEqual(
            info.generate_build_bazel(),
            self.HEADER
            + r"""
alias(
    name = "foo",
    actual = "foo_lib",
)

alias(
    name = "bin",
    actual = "bin_program",
)
""",
        )

    def test_all(self) -> None:
        info = OutputPackageInfo()
        info.add_export("foo_exported")
        info.add_filegroup("foo", ["libfoo.h", "libfoo.cc"])
        info.add_alias("foo_alias", "foo_lib")
        info.add_export("bar_exported")
        info.add_filegroup("bin", ["main.cc"])
        info.add_alias("bin_alias", "bin_program")

        self.assertEqual(
            info.generate_build_bazel(),
            self.HEADER
            + r"""
# The following are direct symlinks that can point into the Ninja output directory or the Fuchsia source dir.
exports_files([
    "bar_exported",
    "foo_exported",
])

filegroup(
    name = "foo",
    srcs = [
        "libfoo.cc",
        "libfoo.h",
    ],
)

filegroup(
    name = "bin",
    srcs = ["main.cc"],
)

alias(
    name = "foo_alias",
    actual = "foo_lib",
)

alias(
    name = "bin_alias",
    actual = "bin_program",
)
""",
        )

    def test_duplicate_names(self) -> None:
        info = OutputPackageInfo()
        info.add_filegroup("foo", [])

        with self.assertRaises(AssertionError) as cm:
            info.add_filegroup("foo", ["libfoo.h"])
        self.assertEqual(str(cm.exception), "Duplicate target name: foo")

        with self.assertRaises(AssertionError) as cm:
            info.add_alias("foo", "foo_actual")
        self.assertEqual(str(cm.exception), "Duplicate target name: foo")

        with self.assertRaises(AssertionError) as cm:
            info.add_export("foo")
        self.assertEqual(
            str(cm.exception), "Cannot export known target name: foo"
        )


class OutputIdkTest(unittest.TestCase):
    def setUp(self) -> None:
        # self._tempdir = tempfile.TemporaryDirectory(prefix='OutputIdkTest-')
        self._tempdir = tempfile.mkdtemp(prefix="OutputIdkTest-")
        self.src_dir = Path(self._tempdir) / "src"
        self.src_dir.mkdir(parents=True)

        self.out_dir = Path(self._tempdir) / "out"
        self.out_dir.mkdir()

        self.out_idk = OutputIdk(self.out_dir)

    def test_symlinks(self) -> None:
        self.out_idk.add_symlink("link_one", self.src_dir / "target_one")
        self.out_idk.add_symlink("subdir/link_two", self.src_dir / "target_two")

        self.out_idk.write_all()

        link_one = self.out_dir / "link_one"
        self.assertTrue(link_one.is_symlink())
        self.assertEqual(str(link_one.readlink()), "../src/target_one")

        link_two = self.out_dir / "subdir" / "link_two"
        self.assertTrue(link_two.is_symlink())
        self.assertEqual(str(link_two.readlink()), "../../src/target_two")

    def test_add_file(self) -> None:
        self.out_idk.add_file("subdir/file_one", "first content")
        self.out_idk.add_json_file(
            "second.json",
            {
                "version": 1,
                "type": "json",
                "items": [
                    "foo",
                    "bar",
                ],
            },
        )
        self.out_idk.write_all()

        file_one = self.out_dir / "subdir" / "file_one"
        self.assertTrue(file_one.exists())
        self.assertEqual(file_one.read_text(), "first content")

        second_json = self.out_dir / "second.json"
        self.assertTrue(second_json.exists())
        self.assertEqual(
            second_json.read_text(),
            """{
  "version": 1,
  "type": "json",
  "items": [
    "foo",
    "bar"
  ]
}""",
        )

    def test_add_package(self) -> None:
        package_info = self.out_idk.get_package_info_for("package")
        package_info.add_alias("foo", "foo_actual")
        self.out_idk.write_all()

        build_bazel = self.out_dir / "package" / "BUILD.bazel"
        self.assertTrue(build_bazel.exists())
        self.assertEqual(
            build_bazel.read_text(),
            OutputPackageInfoTest.HEADER
            + """
alias(
    name = "foo",
    actual = "foo_actual",
)
""",
        )


if __name__ == "__main__":
    unittest.main()
