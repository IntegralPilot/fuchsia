# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import sys
import tempfile
import time
import typing as T
import unittest
from pathlib import Path
from unittest import mock

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, os.path.join(_SCRIPT_DIR, "../bazel/scripts"))
import build_utils
import file_to_test_package


class TestFileToTestPackageFinder(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp_dir = tempfile.TemporaryDirectory()
        self.build_dir = Path(self.tmp_dir.name)
        self.fuchsia_dir = self.build_dir / "fuchsia"
        self.fuchsia_dir.mkdir()

        self.mock_outputs = mock.Mock()
        self.mock_log = mock.Mock()
        self.mock_runner = build_utils.MockCommandRunner()
        self.finder = file_to_test_package.FileToTestPackageFinder(
            self.build_dir,
            self.fuchsia_dir,
            self.mock_outputs,
            self.mock_log,
            host_tag="linux-x64",
            command_runner=self.mock_runner,
        )

        self.run_gn_refs_patcher = mock.patch.object(
            self.finder, "_run_gn_refs"
        )
        self.mock_run_gn_refs = self.run_gn_refs_patcher.start()

        def gn_refs_side_effect(target: str) -> set[str]:
            if target == "//src:lib":
                return {"//src:lib_test", "//src:pkg"}
            if target == "//src:foo":
                return {"//src:foo_test"}
            if target == "//src:complex":
                return {"//src:complex_test"}
            if target == "//src:multi":
                return {"//src:multi_test"}
            return set()

        self.mock_run_gn_refs.side_effect = gn_refs_side_effect
        self.addCleanup(self.run_gn_refs_patcher.stop)

    def tearDown(self) -> None:
        self.tmp_dir.cleanup()

    @mock.patch("builtins.open")
    @mock.patch("pathlib.Path.exists", autospec=True)
    def test_find_test_packages_fast_rust(
        self, mock_exists: mock.MagicMock, mock_open: mock.MagicMock
    ) -> None:
        source_path = "src/lib.rs"
        abs_source = self.fuchsia_dir / source_path

        def exists_side_effect(path: Path) -> bool:
            if path == self.build_dir / "rust-project.json":
                return True
            if path == self.build_dir / "tests.json":
                return True
            return False

        mock_exists.side_effect = exists_side_effect

        # Two crates for same file: lib and lib_test
        rust_content = {
            "crates": [
                {
                    "root_module": str(abs_source),
                    "label": "//src:lib",
                    "cfg": ["feature=default"],
                },
                {
                    "root_module": str(abs_source),
                    "label": "//src:lib_test",
                    "cfg": ["test", "feature=default"],
                },
            ]
        }
        tests_content = [
            {
                "test": {
                    "label": "//src:lib_test",
                }
            }
        ]

        def open_side_effect(
            path: Path | str,
            *args: T.Any,
            **kwargs: T.Any,
        ) -> mock.MagicMock:
            p = str(path)
            m = mock.MagicMock()
            if "rust-project.json" in p:
                return m
            if "tests.json" in p:
                return m
            if "cache" in p:
                raise OSError("Cache missing")
            return m

        mock_open.side_effect = open_side_effect

        mock_file = mock.Mock()
        mock_open.return_value.__enter__.return_value = mock_file

        with mock.patch("json.load") as mock_json_load:
            mock_json_load.side_effect = [rust_content, tests_content]

            # Mock gn refs to return dependents of only the used target
            # If we picked //src:lib, we might see //src:lib_test and //src:other
            # If we picked //src:lib_test, we might see //src:pkg

            self.mock_run_gn_refs.side_effect = None

            def gn_refs_side_effect(target: str) -> set[str]:
                if target == "//src:lib_test":
                    return {"//src:lib_test_pkg", "//src:lib_test"}
                if target == "//src:lib":
                    return {
                        "//src:lib_test",
                        "//src:lib_pkg",
                    }  # Should not happen if preference works
                return set()

            self.mock_run_gn_refs.side_effect = gn_refs_side_effect

            result = self.finder.find_test_packages_fast(source_path)

            # We expect it to find //src:lib_test (test crate), run gn refs on it, and find //src:lib_test
            # Assuming tests.json has //src:lib_test
            self.assertEqual(result, {"//src:lib_test"})

            # Verify _run_gn_refs called with //src:lib_test
            self.mock_run_gn_refs.assert_called_with("//src:lib_test")

    @mock.patch("json.load")
    @mock.patch("builtins.open")
    @mock.patch("pathlib.Path.exists", autospec=True)
    def test_find_test_packages_fast_cpp_compile_commands(
        self,
        mock_exists: mock.MagicMock,
        mock_open: mock.MagicMock,
        mock_json_load: mock.MagicMock,
    ) -> None:
        source_path = "src/foo.cc"
        abs_source = self.fuchsia_dir / source_path

        def exists_side_effect(self_path: Path) -> bool:
            p = str(self_path)
            if "rust-project.json" in p:
                return False
            if "compile_commands.json" in p:
                return True
            if "tests.json" in p:
                return True
            if "cache" in p:
                return False  # Cache doesn't exist
            return False

        mock_exists.side_effect = exists_side_effect

        cc_content = [{"file": str(abs_source), "output": "obj/src/foo.o"}]
        tests_content = [
            {
                "test": {
                    "label": "//src:foo_test",
                }
            }
        ]
        path_map = {
            "compile_commands.json": cc_content,
            "tests.json": tests_content,
        }

        def open_side_effect(
            path: Path | str,
            *args: T.Any,
            **kwargs: T.Any,
        ) -> mock.MagicMock:
            p = str(path)
            m = mock.MagicMock()
            if "cache" in p:
                raise OSError
            for k, v in path_map.items():
                if p.endswith(k):
                    m.__enter__.return_value.tagged_content = v
                    return m
            return m

        mock_open.side_effect = open_side_effect

        # cc -> tests
        mock_json_load.side_effect = [cc_content, tests_content]

        self.mock_outputs.path_to_gn_label.return_value = "//src:foo"

        result = self.finder.find_test_packages_fast(source_path)

        self.assertEqual(result, {"//src:foo_test"})
        self.mock_outputs.path_to_gn_label.assert_called_with("obj/src/foo.o")

    @mock.patch("json.load")
    @mock.patch("builtins.open")
    @mock.patch("pathlib.Path.exists", autospec=True)
    def test_find_test_packages_fast_cpp_o_attached(
        self,
        mock_exists: mock.MagicMock,
        mock_open: mock.MagicMock,
        mock_json_load: mock.MagicMock,
    ) -> None:
        source_path = "src/complex.cc"
        abs_source = self.fuchsia_dir / source_path

        def exists_side_effect(path: Path) -> bool:
            p = str(path)
            if "rust-project.json" in p:
                return False
            if "compile_commands.json" in p:
                return True
            if "tests.json" in p:
                return True
            if "cache" in p:
                return False
            return False

        mock_exists.side_effect = exists_side_effect

        cc_content = [
            {
                "file": str(abs_source),
                "command": "clang++ -c src/complex.cc -oobj/src/complex.o",
                "output": "",
            }
        ]
        tests_content = [{"test": {"label": "//src:complex_test"}}]

        mock_file = mock.MagicMock()
        mock_open.return_value.__enter__.return_value = mock_file

        # cc -> tests
        mock_json_load.side_effect = [cc_content, tests_content]

        # Handle cache open failure
        def open_se(
            path: Path | str,
            *args: T.Any,
            **kwargs: T.Any,
        ) -> mock.MagicMock:
            if "cache" in str(path):
                raise OSError
            return mock_file

        mock_open.side_effect = open_se

        self.mock_outputs.path_to_gn_label.return_value = "//src:complex"

        result = self.finder.find_test_packages_fast("src/complex.cc")
        self.assertEqual(result, {"//src:complex_test"})
        self.mock_outputs.path_to_gn_label.assert_called_with(
            "obj/src/complex.o"
        )

    @mock.patch("json.load")
    @mock.patch("builtins.open")
    @mock.patch("pathlib.Path.exists", autospec=True)
    def test_find_test_packages_fast_cpp_multiple_o(
        self,
        mock_exists: mock.MagicMock,
        mock_open: mock.MagicMock,
        mock_json_load: mock.MagicMock,
    ) -> None:
        source_path = "src/multi.cc"
        abs_source = self.fuchsia_dir / source_path

        def exists_side_effect(path: Path) -> bool:
            p = str(path)
            if "rust-project.json" in p:
                return False
            if "compile_commands.json" in p:
                return True
            if "tests.json" in p:
                return True
            if "cache" in p:
                return False
            return False

        mock_exists.side_effect = exists_side_effect

        cc_content = [
            {
                "file": str(abs_source),
                "command": "clang++ -c src/multi.cc -o obj/src/fake.o -o obj/src/multi.o",
                "output": "",
            }
        ]
        tests_content = [{"test": {"label": "//src:multi_test"}}]

        mock_file = mock.MagicMock()
        mock_open.return_value.__enter__.return_value = mock_file

        mock_json_load.side_effect = [cc_content, tests_content]

        def open_se(
            path: Path | str,
            *args: T.Any,
            **kwargs: T.Any,
        ) -> mock.MagicMock:
            if "cache" in str(path):
                raise OSError
            return mock_file

        mock_open.side_effect = open_se

        self.mock_outputs.path_to_gn_label.return_value = "//src:multi"

        result = self.finder.find_test_packages_fast("src/multi.cc")
        self.assertEqual(result, {"//src:multi_test"})
        self.mock_outputs.path_to_gn_label.assert_called_with("obj/src/multi.o")

    @mock.patch("json.load")
    @mock.patch("builtins.open")
    @mock.patch("pathlib.Path.exists", autospec=True)
    def test_missing_tests_json_fallback(
        self,
        mock_exists: mock.MagicMock,
        mock_open: mock.MagicMock,
        mock_json_load: mock.MagicMock,
    ) -> None:
        source_path = "src/lib.rs"
        abs_source = self.fuchsia_dir / source_path

        # rust-project.json exists, tests.json does NOT
        def exists_side_effect(path: Path) -> bool:
            p = str(path)
            if "rust-project.json" in p:
                return True
            if "tests.json" in p:
                return False
            return False

        def simple_exists(obj: Path | str) -> bool:
            p = str(obj)
            return "rust-project.json" in p

        mock_exists.side_effect = simple_exists

        # Content
        rust_content = {
            "crates": [
                {
                    "root_module": str(abs_source),
                    "label": "//src:lib",
                }
            ]
        }

        mock_open.side_effect = [mock.MagicMock()]
        mock_json_load.return_value = rust_content

        result = self.finder.find_test_packages_fast(source_path)

        self.assertEqual(result, {"//src:lib"})
        self.assertTrue(
            any(
                "WARNING" in str(arg) and "tests.json not found" in str(arg)
                for arg in self.mock_log.call_args[0]
            )
        )

    @mock.patch("json.load")
    @mock.patch("builtins.open")
    @mock.patch("pathlib.Path.exists", autospec=True)
    def test_corrupted_tests_json(
        self,
        mock_exists: mock.MagicMock,
        mock_open: mock.MagicMock,
        mock_json_load: mock.MagicMock,
    ) -> None:
        source_path = "src/lib.rs"
        abs_source = self.fuchsia_dir / source_path
        mock_exists.return_value = True
        rust_content = {
            "crates": [
                {
                    "root_module": str(abs_source),
                    "label": "//src:lib",
                }
            ]
        }

        # open rust-project, open tests.json. Cache NOT loaded because tests.json load fails first.
        # 1. get gn_labels
        # 2. check tests_json.exists()
        # 3. try json.load(tests_json). If fail -> return gn_labels.
        # So cache logic is unreachable if tests.json is corrupted.
        mock_json_load.side_effect = [
            rust_content,
            json.JSONDecodeError("Expecting value", "doc", 0),
        ]

        mock_open.return_value.__enter__.return_value = mock.MagicMock()

        result = self.finder.find_test_packages_fast(source_path)

        # Should fall back to found labels
        self.assertEqual(result, {"//src:lib"})
        self.mock_log.assert_called_with(mock.ANY)
        found_error = False
        for call_args in self.mock_log.call_args_list:
            if "ERROR: Failed to parse corrupted" in str(call_args):
                found_error = True
                break
        self.assertTrue(found_error, "Did not find expected error log")

    @mock.patch("json.load")
    @mock.patch("builtins.open")
    @mock.patch("pathlib.Path.exists", autospec=True)
    def test_corrupted_rust_project_json(
        self,
        mock_exists: mock.MagicMock,
        mock_open: mock.MagicMock,
        mock_json_load: mock.MagicMock,
    ) -> None:
        source_path = "src/lib.rs"
        self.fuchsia_dir / source_path
        mock_exists.return_value = True

        mock_file = mock.MagicMock()
        mock_open.return_value.__enter__.return_value = mock_file

        def json_side_effect(f: T.Any) -> list[T.Any]:
            call_args = mock_open.call_args[0]
            path = str(call_args[0])
            if "rust-project.json" in path:
                raise json.JSONDecodeError("Expecting value", "doc", 0)
            return []

        mock_json_load.side_effect = json_side_effect

        result = self.finder.find_test_packages_fast(source_path)

        self.assertEqual(
            result, set()
        )  # Should handle gracefully and return empty set (or search other files, but here we mock only one)

        # Verify log called
        self.mock_log.assert_called_with(mock.ANY)
        found_error = False
        for call_args in self.mock_log.call_args_list:
            if "rust-project.json search failed" in str(call_args):
                found_error = True
                break
        self.assertTrue(
            found_error, "Did not find expected rust-project.json failure log"
        )

    def test_gn_refs_integration(self) -> None:
        # We want to test the actual _run_gn_refs method, so stop the patcher that mocks it
        self.run_gn_refs_patcher.stop()

        source_path = "src/lib.rs"
        abs_source = self.fuchsia_dir / source_path

        # Create files
        rust_path = self.build_dir / "rust-project.json"
        rust_content = {
            "crates": [
                {
                    "root_module": str(abs_source),
                    "label": "//src:lib",
                }
            ]
        }
        with open(rust_path, "w") as f:
            json.dump(rust_content, f)

        tests_path = self.build_dir / "tests.json"
        tests_content = [
            {"test": {"label": "//src:lib_test", "package_label": "//src:pkg"}}
        ]
        with open(tests_path, "w") as f:
            json.dump(tests_content, f)

        # gn refs output
        # Matches the test label
        self.mock_runner.push_result(
            stdout="//src:pkg\n//src:lib_test\n", returncode=0
        )

        result = self.finder.find_test_packages_fast(source_path)

        self.assertEqual(result, {"//src:lib_test"})

        # Verify gn refs called
        self.assertEqual(len(self.mock_runner.commands), 1)
        cmd = self.mock_runner.commands[0]
        self.assertIn("refs", cmd)
        self.assertIn("//src:lib", cmd)

        # Verify cache saved
        cache_path = self.build_dir / "file_to_test_package_cache.json"
        self.assertTrue(cache_path.exists())
        with open(cache_path) as f:
            data = json.load(f)
        self.assertIn("mapping", data)
        self.assertEqual(data["mapping"]["//src:lib"], ["//src:lib_test"])

    def test_heuristic_prefers_local(self) -> None:
        # We want to test the actual _run_gn_refs method, so stop the patcher that mocks it
        self.run_gn_refs_patcher.stop()

        source_path = "src/foo/lib.rs"
        abs_source = self.fuchsia_dir / source_path

        # Create files
        rust_path = self.build_dir / "rust-project.json"
        rust_content = {
            "crates": [
                {
                    "root_module": str(abs_source),
                    "label": "//src/foo:lib",
                }
            ]
        }
        with open(rust_path, "w") as f:
            json.dump(rust_content, f)

        tests_path = self.build_dir / "tests.json"
        tests_content = [
            {
                "test": {
                    "label": "//src/foo:lib_test",
                    "package_label": "//src/foo:pkg",
                }
            },
            {
                "test": {
                    "label": "//src/other:integration_test",
                    "package_label": "//src/other:pkg",
                }
            },
        ]
        with open(tests_path, "w") as f:
            json.dump(tests_content, f)

        # gn refs returns BOTH
        self.mock_runner.push_result(
            stdout="//src/foo:pkg\n//src/other:pkg\n//src/foo:lib_test\n//src/other:integration_test\n",
            returncode=0,
        )

        result = self.finder.find_test_packages_fast(source_path)

        # Verify mostly local test is returned
        self.assertEqual(result, {"//src/foo:lib_test"})

    def test_cache_hit(self) -> None:
        source_path = "src/lib.rs"
        (self.fuchsia_dir / "src").mkdir(parents=True, exist_ok=True)
        (self.fuchsia_dir / source_path).touch()

        # Create dependencies with OLD timestamp
        dep_mtime = time.time() - 100

        rust_path = self.build_dir / "rust-project.json"
        # We need mapping source to //src:lib
        abs_source = self.fuchsia_dir / source_path
        # os.path.realpath is used in the implementation, so we must match it
        real_abs_source = os.path.realpath(abs_source)

        rust_content = {
            "crates": [
                {
                    "root_module": str(real_abs_source),
                    "label": "//src:lib",
                }
            ]
        }
        with open(rust_path, "w") as f:
            json.dump(rust_content, f)
        os.utime(rust_path, (dep_mtime, dep_mtime))

        tests_path = self.build_dir / "tests.json"
        with open(tests_path, "w") as f:
            json.dump([], f)
        os.utime(tests_path, (dep_mtime, dep_mtime))

        # Other deps
        for dep in ["compile_commands.json", "args.gn"]:
            p = self.build_dir / dep
            p.touch()
            os.utime(p, (dep_mtime, dep_mtime))

        # Create cache with NEWER timestamp
        cache_mtime = time.time()
        cache_content = {
            "mapping": {"//src:lib": ["//src:cached_test"]},
        }
        cache_path = self.build_dir / "file_to_test_package_cache.json"
        with open(cache_path, "w") as f:
            json.dump(cache_content, f)
        os.utime(cache_path, (cache_mtime, cache_mtime))

        result = self.finder.find_test_packages_fast(abs_source)

        self.assertEqual(result, {"//src:cached_test"})
        # Verify no commands run (gn refs mocked)
        self.assertEqual(len(self.mock_runner.commands), 0)

    def test_heuristic_prefers_local_implicit_target(self) -> None:
        """Test that heuristic works for targets like //src/foo (no colon)."""
        # We want to test the actual _run_gn_refs method, so stop the patcher that mocks it
        self.run_gn_refs_patcher.stop()

        source_path = "src/foo/lib.rs"
        abs_source = self.fuchsia_dir / source_path

        # Create files
        rust_path = self.build_dir / "rust-project.json"
        rust_content = {
            "crates": [
                {
                    "root_module": str(abs_source),
                    "label": "//src/foo",  # implicit colon
                }
            ]
        }
        with open(rust_path, "w") as f:
            json.dump(rust_content, f)

        tests_path = self.build_dir / "tests.json"
        tests_content = [
            {
                "test": {
                    "label": "//src/foo:lib_test",
                    "package_label": "//src/foo:pkg",
                }
            },
            {
                "test": {
                    "label": "//src/other:integration_test",
                    "package_label": "//src/other:pkg",
                }
            },
        ]
        with open(tests_path, "w") as f:
            json.dump(tests_content, f)

        # gn refs returns BOTH
        self.mock_runner.push_result(
            stdout="//src/foo:pkg\n//src/other:pkg\n//src/foo:lib_test\n//src/other:integration_test\n",
            returncode=0,
        )

        result = self.finder.find_test_packages_fast(abs_source)

        # Verify mostly local test is returned (//src/foo:lib_test)
        self.assertEqual(result, {"//src/foo:lib_test"})


if __name__ == "__main__":
    unittest.main()
