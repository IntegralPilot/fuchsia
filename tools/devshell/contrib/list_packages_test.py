#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import json
import os
import re
import sys
import tempfile
import unittest
from collections.abc import Iterable
from unittest import mock

import list_packages


class TestPrintPackages(unittest.TestCase):
    def validate(self, input: Iterable[str], output: str) -> None:
        f = io.StringIO()
        with contextlib.redirect_stdout(f):
            list_packages.print_packages(input)

        self.assertEqual(f.getvalue(), output)

    def test_single_package(self) -> None:
        self.validate(["package"], "package\n")

    def test_multiple_packages(self) -> None:
        self.validate(
            ["package0", "package2", "package1", "package3"],
            """package0
package1
package2
package3
""",
        )


class TestExtractPackages(unittest.TestCase):
    def test_extract_packages_from_listing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            manifest1_path = os.path.join(temp_dir, "pkg1_manifest.json")
            manifest2_path = os.path.join(temp_dir, "pkg2_manifest.json")
            with open(manifest1_path, "w") as f:
                json.dump({"package": {"name": "alpha"}}, f)
            with open(manifest2_path, "w") as f:
                json.dump({"package": {"name": "beta"}}, f)

            listing = {
                "content": {
                    "manifests": ["pkg1_manifest.json", "pkg2_manifest.json"]
                }
            }
            packages = list(
                list_packages.extract_packages_from_listing(
                    listing, lambda s: True, build_dir=temp_dir
                )
            )
            self.assertEqual(packages, ["alpha", "beta"])


class TestMainErrorHandling(unittest.TestCase):
    @mock.patch.dict(os.environ, {}, clear=True)
    @mock.patch.object(sys, "argv", ["list_packages.py"])
    def test_unset_fuchsia_build_dir(self) -> None:
        with self.assertRaises(RuntimeError) as cm:
            list_packages.main()
        self.assertEqual(
            str(cm.exception),
            'Environment variable "FUCHSIA_BUILD_DIR" is not set.',
        )

    @mock.patch.object(sys, "argv", ["list_packages.py"])
    def test_missing_manifest_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            with mock.patch.dict(os.environ, {"FUCHSIA_BUILD_DIR": temp_dir}):
                manifest_list_path = os.path.join(
                    temp_dir, "all_package_manifests.list"
                )
                with self.assertRaises(RuntimeError) as cm:
                    list_packages.main()
                self.assertEqual(
                    str(cm.exception),
                    f"'{manifest_list_path}' not found. Run 'fx build' or 'fx build updates' to assemble package manifests.",
                )

    @mock.patch.object(sys, "argv", ["list_packages.py", "[invalid("])
    def test_main_invalid_regex(self) -> None:
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            with self.assertRaises(SystemExit) as cm:
                list_packages.main()
        self.assertEqual(cm.exception.code, 2)
        self.assertIn("invalid regular expression '[invalid('", err.getvalue())


class TestMain(unittest.TestCase):
    def _setup_manifests(self, temp_dir: str, package_names: list[str]) -> None:
        manifest_filenames = []
        for pkg in package_names:
            manifest_name = f"{pkg}.json"
            manifest_filenames.append(manifest_name)
            with open(os.path.join(temp_dir, manifest_name), "w") as f:
                json.dump({"package": {"name": pkg}}, f)
        manifest_list_path = os.path.join(
            temp_dir, "all_package_manifests.list"
        )
        with open(manifest_list_path, "w") as f:
            json.dump({"content": {"manifests": manifest_filenames}}, f)

    @mock.patch.object(sys, "argv", ["list_packages.py"])
    def test_main_success(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            self._setup_manifests(temp_dir, ["my-package"])
            f = io.StringIO()
            with mock.patch.dict(os.environ, {"FUCHSIA_BUILD_DIR": temp_dir}):
                with contextlib.redirect_stdout(f):
                    list_packages.main()
            self.assertEqual(f.getvalue(), "my-package\n")

    @mock.patch.object(sys, "argv", ["list_packages.py", "pack"])
    def test_main_substring_filter(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            self._setup_manifests(
                temp_dir, ["my-package", "package-2", "other-tool"]
            )
            f = io.StringIO()
            with mock.patch.dict(os.environ, {"FUCHSIA_BUILD_DIR": temp_dir}):
                with contextlib.redirect_stdout(f):
                    list_packages.main()
            self.assertEqual(f.getvalue(), "my-package\npackage-2\n")

    @mock.patch.object(sys, "argv", ["list_packages.py", "-e", "package-2"])
    def test_main_exact_filter(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            self._setup_manifests(
                temp_dir, ["my-package", "package-2", "other-tool"]
            )
            f = io.StringIO()
            with mock.patch.dict(os.environ, {"FUCHSIA_BUILD_DIR": temp_dir}):
                with contextlib.redirect_stdout(f):
                    list_packages.main()
            self.assertEqual(f.getvalue(), "package-2\n")


class TestFilter(unittest.TestCase):
    def test_no_pattern(self) -> None:
        f = list_packages.get_filter(None)
        self.assertTrue(f("foo"))
        self.assertTrue(f("bar"))

    def test_substring_matching_default(self) -> None:
        f = list_packages.get_filter("pkg")
        self.assertTrue(f("pkg"))
        self.assertTrue(f("my-pkg"))
        self.assertTrue(f("pkg-test"))
        self.assertTrue(f("my-pkg-test"))
        self.assertFalse(f("other"))

    def test_exact_matching(self) -> None:
        f = list_packages.get_filter("pkg", exact=True)
        self.assertTrue(f("pkg"))
        self.assertFalse(f("my-pkg"))
        self.assertFalse(f("pkg-test"))
        self.assertFalse(f("other"))

    def test_regex_matching_substring(self) -> None:
        f = list_packages.get_filter(r"pkg-\d+")
        self.assertTrue(f("prefix-pkg-123-suffix"))
        self.assertTrue(f("pkg-1"))
        self.assertFalse(f("pkg-abc"))

    def test_regex_matching_exact(self) -> None:
        f = list_packages.get_filter(r"pkg-\d+", exact=True)
        self.assertTrue(f("pkg-123"))
        self.assertFalse(f("prefix-pkg-123"))
        self.assertFalse(f("pkg-123-suffix"))
        self.assertFalse(f("pkg-abc"))

    def test_invalid_regex_raises_re_error(self) -> None:
        with self.assertRaises(re.error):
            list_packages.get_filter("[invalid(")


if __name__ == "__main__":
    unittest.main()
