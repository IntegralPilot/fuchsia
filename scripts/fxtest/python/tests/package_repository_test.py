# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import tempfile
import unittest
import unittest.mock as mock

import package_repository


class TestPackageRepository(unittest.TestCase):
    def test_extract_package_name_custom_domain(self) -> None:
        """Test that package name is extracted correctly for standard and custom domain URLs."""
        self.assertEqual(
            package_repository.extract_package_name_from_url(
                "fuchsia-pkg://fuchsia.com/my-pkg#meta/my-component.cm"
            ),
            "my-pkg",
        )
        self.assertEqual(
            package_repository.extract_package_name_from_url(
                "fuchsia-pkg://custom-repo.org/custom-pkg#meta/test.cm"
            ),
            "custom-pkg",
        )

    def test_targets_multi_slash_key(self) -> None:
        """Test that target keys with multiple slashes do not raise ValueError."""
        with tempfile.TemporaryDirectory() as td:
            repo_file = os.path.join(td, "package-repositories.json")
            targets_file = os.path.join(td, "targets.json")

            with open(repo_file, "w") as f:
                json.dump([{"targets": "targets.json"}], f)

            with open(targets_file, "w") as f:
                json.dump(
                    {
                        "signed": {
                            "targets": {
                                "multi/part/target/0": {
                                    "custom": {"merkle": "abc123merkle"}
                                }
                            }
                        }
                    },
                    f,
                )

            exec_env = mock.MagicMock()
            exec_env.package_repositories_file = repo_file

            repo = package_repository.PackageRepository.from_env(exec_env)
            self.assertEqual(repo.name_to_merkle.get("multi"), "abc123merkle")

    def test_resolve_component_url(self) -> None:
        repo = package_repository.PackageRepository({"my-pkg": "12345"})
        resolved = repo.resolve_component_url(
            "fuchsia-pkg://custom-domain.com/my-pkg#meta/test.cm"
        )
        self.assertEqual(
            resolved,
            "fuchsia-pkg://custom-domain.com/my-pkg?hash=12345#meta/test.cm",
        )


if __name__ == "__main__":
    unittest.main()
