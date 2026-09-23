#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from terminal import Terminal

DESCRIPTION = """
Publishes a given set of packages.

TODO(https://fxbug.dev/42078455): Currently only cache packages are supported by this
tool.
"""


@dataclass
class PackageDiffs:
    added: list[str]
    modified: list[str]
    removed: list[str]

    @classmethod
    def snapshot(cls, repo: Path) -> dict[str, str]:
        repo_targets = repo / "repository" / "targets.json"
        # New empty repositories do not have targets.json.
        if not repo_targets.is_file():
            return {}
        targets = json.loads(repo_targets.read_text())
        return {
            (package[:-2] if package.endswith("/0") else package): props[
                "custom"
            ]["merkle"]
            for package, props in targets["signed"]["targets"].items()
        }

    @classmethod
    def diff(cls, old: dict[str, str], new: dict[str, str]) -> "PackageDiffs":
        return cls(
            added=[package for package in new if package not in old],
            modified=[
                package
                for package in old.keys() & new.keys()
                if old[package] != new[package]
            ],
            removed=[package for package in old if package not in new],
        )

    def __len__(self) -> int:
        return len(self.added) + len(self.modified) + len(self.removed)

    def __bool__(self) -> bool:
        return len(self) > 0


def run(*command: str, failure_message: str, **kwargs: Any) -> None:
    str_command = shlex.join([str(part) for part in command])
    try:
        subprocess.check_call(str_command, shell=True, **kwargs)
    except subprocess.SubprocessError as e:
        Terminal.error(f"Command failed: {str_command}")
        Terminal.fatal(failure_message)


def fx_command(*command: str) -> list[str]:
    return [
        "bash",
        "-c",
        " ".join(
            [
                ".",
                str(
                    (Path(__file__).parent.parent / "lib" / "vars.sh").resolve()
                ),
                "&&",
                "cd $FUCHSIA_DIR",
                "&&",
                "fx-config-read",
                "&&",
                "fx-command-run",
                shlex.join([str(part) for part in command]),
            ]
        ),
    ]


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="fx publish",
        formatter_class=argparse.RawTextHelpFormatter,
        description=DESCRIPTION,
    )
    parser.add_argument(
        "packages",
        help="Specify packages to publish. Currently, only cache packages are supported.",
        action="store",
        nargs="+",
    )
    parser.add_argument(
        "--repo",
        help="Publish to this repo directory (relative to the build directory); defaults to `amber-files`.",
        default="amber-files",
    )
    parser.add_argument(
        "--quiet",
        help="Suppress messages.",
        action="store_true",
    )
    args = parser.parse_args()

    Terminal.suppress = args.quiet

    build_dir = Path(".").resolve()

    package_tool = build_dir / "host-tools" / "package-tool"
    repo = build_dir / args.repo
    publish_tool_opts = (
        (repo / "publish_tool_opts").read_text().strip().split("\n")
    )

    # Collect ninja targets.
    ninja_targets = ["//build/images/updates:prepare_publish"]
    packages_to_publish = []
    for packages in set(args.packages):
        if packages == "cache":
            ninja_targets.append(
                "//build/images/updates:assembly_cache_packages.list"
            )
            packages_to_publish.append(
                build_dir / "assembly_cache_packages.list"
            )
        else:
            Terminal.fatal(f'Unrecognized packages "{packages}".')

    # Build step.
    run(
        *fx_command("build", *ninja_targets),
        failure_message="Build failures!",
    )

    # Publish the packages.
    initial = PackageDiffs.snapshot(repo)
    run(
        str(package_tool),
        "repository",
        "publish",
        repo,
        *publish_tool_opts,
        *[
            str(arg)
            for package_list in packages_to_publish
            for arg in ["--package-list", package_list]
        ],
        failure_message="An internal error occured while publishing.",
    )
    final = PackageDiffs.snapshot(repo)

    # Report package publishing.
    package_diff = PackageDiffs.diff(initial, final)
    if package_diff:
        Terminal.info(f"Updated {len(package_diff)} packages!")
        if package_diff.added:
            Terminal.info(
                f'Added packages: {", ".join(map(Terminal.bold, package_diff.added))}'
            )
        if package_diff.modified:
            Terminal.info(
                f'Modified packages: {", ".join(map(Terminal.bold, package_diff.modified))}'
            )
        if package_diff.removed:
            Terminal.fatal("`fx publish` should never remove packages.")
    else:
        Terminal.warn("No packages were updated.")

    # Check if package server is running.
    subprocess.run(
        shlex.join(fx_command("is-package-server-running")),
        shell=True,
        env=os.environ
        | {
            # Suppress extra warning message about incremental package serving.
            "FUCHSIA_DISABLED_incremental": "1",
        },
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
