#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import shutil
import sys
import unittest
from collections import defaultdict
from contextlib import contextmanager
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Iterator
from unittest.mock import patch

# Import the real devshell `fx publish` python implementation.
import publish


@contextmanager
def patch_cwd(dir_path: str) -> Iterator[None]:
    assert dir_path and os.path.isdir(
        dir_path
    ), f"Expected {dir_path} to be a directory."
    path = Path(dir_path)
    cwd = os.getcwd()

    with TemporaryDirectory() as temp_dir:
        working_dir = Path(temp_dir) / path
        # Avoid polluting the build dir: Keep test hermetic by copying the test
        # dir to a working temporary directory.
        shutil.copytree(path, working_dir)

        # "./{path}" should resolve to an equivalent directory for cwd="{cwd}"
        # and cwd="{working_dir}".
        target = Path(*[".."] * (len(path.parts) - 1))
        symlink = working_dir / path
        symlink.parent.mkdir(parents=True, exist_ok=True)
        symlink.symlink_to(target)

        try:
            os.chdir(working_dir)
            yield
        finally:
            os.chdir(cwd)
            symlink.unlink()


class FxPublishTest(unittest.TestCase):
    TEST_DIR: str | None = None
    EXPECTED_CACHE_PACKAGES: list[str] = []

    def setUp(self) -> None:
        self.fx_commands: defaultdict[str, list[tuple[str, ...]]] = defaultdict(
            lambda: []
        )

    def fake_fx_command(self, subcommand: str, *args: str) -> list[str]:
        self.fx_commands[subcommand].append(args)
        return ["true", subcommand, *args]

    def _test_publish_cache(self, test_dir: str) -> None:
        with (
            patch(
                "sys.argv",
                [
                    publish.__file__,
                    "cache",
                    "--quiet",
                ],
            ),
            patch("publish.fx_command", self.fake_fx_command),
            patch_cwd(test_dir),
        ):
            # Launches the real devshell publish tool under the fake build
            # directory.
            self.assertEqual(publish.main(), 0)

            # Check that the right ninja targets are built.
            self.assertEqual(
                [set(build_args) for build_args in self.fx_commands["build"]],
                [
                    set(
                        [
                            "//build/images/updates:prepare_publish",
                            "//build/images/updates:assembly_cache_packages.list",
                        ]
                    )
                ],
            )

            # Check that the required packages are published.
            repo_targets = json.loads(
                Path("amber-files/repository/targets.json").read_text()
            )
            for cache_package in FxPublishTest.EXPECTED_CACHE_PACKAGES:
                self.assertIn(
                    cache_package, repo_targets["signed"]["targets"].keys()
                )

    def test_publish_cache(self) -> None:
        self.assertIsNotNone(FxPublishTest.TEST_DIR)
        self._test_publish_cache(FxPublishTest.TEST_DIR)  # type: ignore[arg-type]


def main() -> None:
    parser = argparse.ArgumentParser(
        description="An integration test for `fx publish`."
    )
    parser.add_argument(
        "--test-dir",
        required=True,
        help="The fake fx publish working directory.",
    )
    parser.add_argument(
        "--expect-cache-packages",
        nargs="*",
        default=[],
        help="A list of cache packages that are expected to be published.",
    )
    args, unknownargs = parser.parse_known_args()
    FxPublishTest.TEST_DIR = args.test_dir
    FxPublishTest.EXPECTED_CACHE_PACKAGES = args.expect_cache_packages
    unittest.main(argv=[sys.argv[0], *unknownargs])


if __name__ == "__main__":
    main()
