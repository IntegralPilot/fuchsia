#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for bazel_repository_utils.py."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR))

from bazel_repository_utils import BazelRootRepoMapping
from build_utils import BazelLauncher, BazelPaths, MockCommandRunner


class BazelRootRepoMappingTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.tmp_dir = Path(self._td.name)
        self.fuchsia_dir = self.tmp_dir / "fuchsia"
        self.fuchsia_dir.mkdir()
        (self.fuchsia_dir / ".jiri_manifest").write_text("")
        self.build_dir = self.fuchsia_dir / "out" / "default"
        self.build_dir.mkdir(parents=True)
        BazelPaths.write_topdir_config_for_test(
            self.fuchsia_dir, "gen/build/bazel"
        )
        self.bazel_paths = BazelPaths(self.fuchsia_dir, self.build_dir)

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_new_from_bazel_success(self) -> None:
        mock_runner = MockCommandRunner()
        sample_output = json.dumps(
            {
                "rules_fuchsia": "rules_fuchsia+",
                "": "",
                "main": "",
                "zlib": "zlib+",
                "fuchsia_sdk": "rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk",
            }
        )
        mock_runner.push_result(returncode=0, stdout=sample_output)
        launcher = BazelLauncher(launcher_script="bazel", runner=mock_runner)
        repo_mapping = BazelRootRepoMapping.new_from_bazel(launcher)

        expected = {
            "rules_fuchsia": "rules_fuchsia+",
            "zlib": "zlib+",
            "fuchsia_sdk": "rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk",
        }
        self.assertEqual(repo_mapping.get_mapping(), expected)
        self.assertEqual(
            repo_mapping.get_canonical_name("rules_fuchsia"), "rules_fuchsia+"
        )
        self.assertIsNone(repo_mapping.get_canonical_name("non_existent"))
        self.assertEqual(
            repo_mapping.get_apparent_name("rules_fuchsia+"), "rules_fuchsia"
        )
        self.assertEqual(
            repo_mapping.get_apparent_name(
                "rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk"
            ),
            "fuchsia_sdk",
        )
        self.assertIsNone(repo_mapping.get_apparent_name("non_existent+"))
        self.assertIn("rules_fuchsia", repo_mapping)
        self.assertNotIn("main", repo_mapping)
        self.assertNotIn("", repo_mapping)
        self.assertEqual(repo_mapping["zlib"], "zlib+")
        self.assertEqual(len(repo_mapping), 3)
        self.assertEqual(
            list(iter(repo_mapping)), ["rules_fuchsia", "zlib", "fuchsia_sdk"]
        )

    def test_new_from_bazel_failure(self) -> None:
        mock_runner = MockCommandRunner()
        mock_runner.push_result(returncode=1, stderr="Bazel failed")
        launcher = BazelLauncher(launcher_script="bazel", runner=mock_runner)
        with self.assertRaises(RuntimeError):
            BazelRootRepoMapping.new_from_bazel(launcher)

    def test_new_from_bazel_invalid_json(self) -> None:
        mock_runner = MockCommandRunner()
        mock_runner.push_result(returncode=0, stdout="not valid json")
        launcher = BazelLauncher(launcher_script="bazel", runner=mock_runner)
        with self.assertRaises(RuntimeError):
            BazelRootRepoMapping.new_from_bazel(launcher)

    def test_save_and_load_from_disk(self) -> None:
        mock_runner = MockCommandRunner()
        sample_output = json.dumps(
            {
                "rules_fuchsia": "rules_fuchsia+",
                "": "",
                "main": "",
                "zlib": "zlib+",
            }
        )
        mock_runner.push_result(returncode=0, stdout=sample_output)
        launcher = BazelLauncher(launcher_script="bazel", runner=mock_runner)
        repo_mapping = BazelRootRepoMapping.new_from_bazel(launcher)

        expected_path = (
            self.build_dir / "regenerator_outputs" / "root_repo_mapping.json"
        )
        self.assertEqual(
            BazelRootRepoMapping.get_cache_path(self.build_dir),
            expected_path,
        )

        cache_path = repo_mapping.save_to_disk(self.build_dir)
        self.assertEqual(cache_path, expected_path)
        self.assertTrue(expected_path.exists())

        # Load back from disk with new_from_build_dir
        loaded_mapping = BazelRootRepoMapping.new_from_build_dir(self.build_dir)
        self.assertEqual(
            loaded_mapping.get_mapping(),
            {"rules_fuchsia": "rules_fuchsia+", "zlib": "zlib+"},
        )

    def test_load_from_disk_missing(self) -> None:
        with self.assertRaises(FileNotFoundError):
            BazelRootRepoMapping.new_from_build_dir(self.build_dir)


if __name__ == "__main__":
    unittest.main()
