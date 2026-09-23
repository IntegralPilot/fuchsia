# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


# `bazel test` incorrectly handles stdout, so logging to stderr will keep our
# log statements in order w.r.t test/subprocess output.
# See https://github.com/bazelbuild/bazel/issues/7388.
def log(*kwargs):
    print(*kwargs, file=sys.stderr)


def parse_args() -> tuple[argparse.Namespace, list[str]]:
    """Separates relevant arguments from unknown arguments."""
    parser = argparse.ArgumentParser()

    def path_arg(type="file"):
        def arg(path):
            path = Path(path)
            assert (
                type == "file"
                and path.is_file()
                or type == "directory"
                and path.is_dir()
            ), f'Path "{path}" is not a {type}!'
            return path

        return arg

    parser.add_argument(
        "--name",
        help="The test's name.",
        required=True,
    )
    parser.add_argument(
        "--test-binary",
        type=path_arg(),
        help="A path to the prebuilt lacewing test binary.",
        required=True,
    )
    parser.add_argument(
        "--ffx",
        type=path_arg(),
        help="A path to the ffx tool.",
        required=True,
    )
    parser.add_argument(
        "--target",
        help="Optionally specify the target to run these tests against. Defaults to the default target device.",
    )
    return parser.parse_known_args()


def write_mobly_config(test_bed: str, ffx: Path, target: str | None) -> Path:
    target = (
        target
        or subprocess.check_output(
            [ffx, "target", "default", "get"],
            text=True,
        ).strip()
    )
    mobly_config_contents = json.dumps(
        {
            "TestBeds": [
                {
                    "Name": test_bed,
                    "Controllers": {
                        "FuchsiaDevice": [
                            {
                                "name": target,
                                "honeydew_config": {
                                    "transports": {
                                        "ffx": {
                                            "path": str(ffx.resolve()),
                                        }
                                    }
                                },
                            }
                        ]
                    },
                }
            ]
        },
        indent=2,
    )
    mobly_config = Path(tempfile.mktemp(".json", "mobly_config-"))
    mobly_config.write_text(mobly_config_contents)
    log(f"DEBUG: {str(mobly_config)}:\n{mobly_config_contents}")
    return mobly_config


def run_lacewing_test(
    test_binary: Path,
    mobly_config: Path,
    *test_args: str,
) -> int:
    # Run the test.
    command = [
        test_binary.resolve(),
        "-c",
        mobly_config,
        *test_args,
    ]
    log(f"DEBUG: Running subcommand: {shlex.join(map(str, command))}")

    # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
    # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
    # inheritance which can confuse child Python processes.
    env = dict(os.environ)
    env.pop("RUNFILES_DIR", None)
    env.pop("RUNFILES_MANIFEST_FILE", None)

    return subprocess.run(command, env=env).returncode


def copy_lacewing_outputs(test_bed: str, output_path: Path) -> None:
    log(f"DEBUG: Copying test outputs for {test_bed}: {output_path}")
    shutil.copytree(f"/tmp/logs/mobly/{test_bed}/latest", output_path)


def main() -> int:
    args, forward_args = parse_args()

    test_bed = "GeneratedTestbed"

    # Write the mobly config.
    mobly_config = write_mobly_config(test_bed, args.ffx, args.target)

    # Run the lacewing test.
    result = run_lacewing_test(
        test_binary=args.test_binary.resolve(),
        mobly_config=mobly_config,
        *forward_args,
    )

    # Upload lacewing's test output artifacts.
    # See `bazel_build_test_upload` for how this environment variable is used.
    if "TEST_UNDECLARED_OUTPUTS_DIR" in os.environ:
        copy_lacewing_outputs(
            test_bed,
            Path(os.environ["TEST_UNDECLARED_OUTPUTS_DIR"]) / args.name,
        )
    return result


if __name__ == "__main__":
    sys.exit(main())
