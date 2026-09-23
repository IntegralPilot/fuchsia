#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Runs multiple builds and gathers their edge weights for merging

This script configures and builds the following:
  - core.x64-release
  - core.x64-asan
  - core.arm64-release
  - core.arm64-hwasan

with all buildbot test bundles included, and saves the ninja logs
as edge weights, and then produces a merged set of ninja edge
weights that can be checked in as 'merged_measured_weights.csv'

This script should be run from the root fuchsia dir.
"""

import dataclasses
import os
import shutil
import subprocess
import sys
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
from merge_weights_files import merge_weights_files_finding_max
from ninjalog_to_weights import convert_log_to_weights


@dataclasses.dataclass
class BuildConfig:
    product: str
    board: str
    compilation_mode: str
    optimize: str | None = None
    variants: list[str] | None = None

    def name(self) -> str:
        variant_str = ("-" + "-".join(self.variants)) if self.variants else ""
        return (
            f"{self.product}.{self.board}-{self.compilation_mode}{variant_str}"
        )


# Labels to add to the respectively named groups
HOST_LABELS = [
    "//bundles/buildbot/host",
    "//bundles/infra/build",
    "//bundles/infra/test",
    "//tools/gn_desc",
]

TEST_LABELS = [
    "//bundles/buildbot/core",
    "//bundles/buildbot/core:hermetic_tests",
    "//bundles/buildbot/core:e2e_tests",
]


def main() -> int:
    if os.environ.get("FUCHSIA_DIR") != os.getcwd():
        print(
            "ERROR: This should be run from the Fuchsia checkout directory.",
            file=sys.stderr,
        )
        return -1

    build_configs = [
        BuildConfig("core", "x64", "release"),
        BuildConfig(
            "core",
            "x64",
            "debug",
            "sanitizer",
            ["asan-ubsan", "host_asan-ubsan"],
        ),
        BuildConfig("core", "arm64", "release"),
        BuildConfig(
            "core",
            "arm64",
            "debug",
            "sanitizer",
            ["hwasan-ubsan", "host_asan-ubsan"],
        ),
    ]

    weights_files: list[Path] = []
    for build_config in build_configs:
        # Prepare a clean, empty build dir using the config
        out_dir = Path("out/edge_weights")
        setup_build_dir(out_dir, build_config)

        # Run the build
        subprocess.run(["fx", "build"], check=True)

        # Convert the logfile to a csv full of weights (in ms)
        weights_file = Path(build_config.name() + ".weights.csv")
        weights_files.append(weights_file)
        convert_log_to_weights(out_dir / ".ninja_log", weights_file, 60000)

    # Merge all the weights together and replace the checked-in merged
    # file with the result.
    merge_weights_files_finding_max(
        weights_files,
        Path("build/weights/merged_measured_weights.csv"),
    )

    return 0


def setup_build_dir(outdir: Path, build_config: BuildConfig) -> None:
    """Run fx set for a given configuration

    Runs fx set to prepare a build dir with the given configuration.
    The `outdir` must be a Path object, typically a subdirectory of the
    Fuchsia checkout root, such as 'out/edge_weights'.
    """
    print(f"    preparing outdir: {outdir}")

    print(f"      emptying...")
    if outdir.exists():
        shutil.rmtree(outdir)

    # setup the build environment
    cmd: list[str] = [
        "fx",
        "--dir",
        str(outdir),
        "set",
        f"{build_config.product}.{build_config.board}",
        f"--{build_config.compilation_mode}",
        "--rbe-mode",
        "off",
    ]

    if build_config.variants:
        cmd.extend(
            [
                "--include-clippy=false",
                '--args=optimize="sanitizer"',
                "--variant",
                ",".join(build_config.variants),
            ]
        )

    cmd.extend(
        [
            "--with-host",
            ",".join(HOST_LABELS),
            "--with-test",
            ",".join(TEST_LABELS),
        ]
    )

    print(f"      setting... {' '.join(cmd)}")
    subprocess.run(cmd, check=True)


if __name__ == "__main__":
    sys.exit(main())
