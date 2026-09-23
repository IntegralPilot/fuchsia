# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import importlib
import json
import os
import sys
import unittest
import zipimport


def load_tests_from_file(file_path: str) -> list[str]:
    """Loads test case names from a file using unittest discovery."""
    if not os.path.exists(file_path):
        print(f"File not found: {file_path}", file=sys.stderr)
        return []

    modules = []

    # For Bazel host tests, a file named bazel_host_py_test_info.json will be in the
    # current (runtime) directory.
    # LINT.IfChange(bazel_host_py_test_info_schema)
    bazel_host_py_test_info_file = "bazel_host_py_test_info.json"
    if os.path.exists(bazel_host_py_test_info_file):
        runfiles_dir = file_path + ".runfiles"
        assert os.path.isdir(
            runfiles_dir
        ), f"Test runfiles directory not found: {runfiles_dir}"

        with open(bazel_host_py_test_info_file, "r") as f:
            test_info = json.load(f)

        import_paths = test_info["imports"]
        binary_path = test_info["binary"]
        # LINT.ThenChange(//build/bazel/rules/host_tests/host_py_test.bzl:bazel_host_py_test_info_schema)

        for path in import_paths:
            import_path = os.path.join(runfiles_dir, path)
            sys.path.append(import_path)

        # Get rid of the main runfiles sub-directory (which will be "_main" or a repository name),
        # and of the .py suffix to compute the module path. E.g.:
        #
        #  _main/src/foo/test.py => src.foo.test
        #
        # _main/ should have been added as an import path in the loop above.
        assert binary_path.endswith(
            ".py"
        ), f"Binary path {binary_path} does not end with .py"
        binary_path_components = binary_path[:-3].split("/")
        assert (
            binary_path_components[0] in import_paths
        ), f"{binary_path_components}: Binary path {binary_path} does not start with an valid import path from: {import_paths}"
        module_name = ".".join(binary_path_components[1:])
        try:
            module = importlib.import_module(module_name)
            modules.append(module)
        except ImportError as e:
            print(f"Error importing module {module_name}: {e}", file=sys.stderr)
            return []
    else:
        # For the GN case, assume the test binary is a PYZ archive.
        sys.path.insert(0, file_path)

        # Try to load the module with the same name as the file
        module_name = os.path.splitext(os.path.basename(file_path))[0]
        try:
            modules.append(importlib.import_module(module_name))
        except (ImportError, zipimport.ZipImportError):
            # Fallback: list .py files in the PYZ
            import zipfile

            try:
                with zipfile.ZipFile(file_path, "r") as zf:
                    for name in zf.namelist():
                        if name.endswith(".py") and not name.startswith("__"):
                            # Convert path to module name
                            mod_name = name.replace("/", ".")[:-3]
                            try:
                                modules.append(
                                    importlib.import_module(mod_name)
                                )
                            except Exception:
                                pass
            except (zipfile.BadZipFile, OSError):
                return []

    loader = unittest.TestLoader()
    suite = unittest.TestSuite()

    try:
        for module in modules:
            suite.addTests(loader.loadTestsFromModule(module))

    except Exception as e:
        print(f"Error importing modules for {file_path}: {e}", file=sys.stderr)
        return []

    test_names = []

    # TestSuite can contain TestSuites or TestCases. Flatten it.
    def flatten_suite(s: unittest.TestSuite) -> None:
        for item in s:
            if isinstance(item, unittest.TestCase):
                # We want to extract "TestName.TestCaseName".
                # item.__class__.__name__ is `TestName`
                # item._testMethodName is `TestCaseName`
                test_names.append(
                    f"{item.__class__.__name__}.{item._testMethodName}"
                )
            elif isinstance(item, unittest.TestSuite):
                # Unittest can have nested TestSuites.
                flatten_suite(item)

    flatten_suite(suite)
    return test_names


def main() -> None:
    parser = argparse.ArgumentParser(description="List host tests from files.")
    parser.add_argument(
        "--list_tests",
        help="Path to the test file to list tests from.",
        required=True,
    )
    args = parser.parse_args()

    file_path = args.list_tests.strip()
    if not file_path:
        return

    tests = load_tests_from_file(file_path)
    if tests:
        print("\n".join(tests))


if __name__ == "__main__":
    main()
