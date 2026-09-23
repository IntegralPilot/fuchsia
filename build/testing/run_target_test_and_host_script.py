#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import pathlib
import subprocess
import sys
import unittest

# These are variables will be substituted with the real values at run time.
# We do not know where the test artifacts are stored until after `ffx test`
# completes. So we offer this place holder to substitute the values once they
# are evaluated.
VARIABLE_SUBSTITUTION = {"{test_artifact_dir}": ""}


def run_target_test(
    ffx_bin: str, test_url: str, outdir: str, ffx_test_args: list[str]
) -> subprocess.CompletedProcess:
    """Runs 'ffx test run <url> --output-directory outdir [args]'"""

    # use the same configuration in //src/developer/ffx/build/ffx_action.gni
    base_config = [
        "ffx.analytics.disabled=true",
        "daemon.autostart=false",
        "log.enabled=false",
    ]

    args = [ffx_bin]
    for c in base_config:
        args += ["--config", c]
    args += [
        "--env",
        "./.ffx.env",
        "test",
        "run",
        test_url,
        "--output-directory",
        outdir,
    ]
    args += ffx_test_args
    return subprocess.run(args, text=True, stderr=subprocess.PIPE)


def get_test_suite_artifact_dir(
    output_root: pathlib.Path, test_suite_name: str
):
    """Parses run_summary.json and reads back 'artifact_dir'.

    Raises:
       KeyError: if json keys for parsing test artifact directory does not exist.

    Returns:
       subdir path where test suite artifact path is stored.
    """
    summary_json = os.path.join(output_root, "run_summary.json")
    with open(summary_json, "r") as f:
        test_summary = json.load(f)
        test_summary = test_summary["data"]
        for suite in test_summary["suites"]:
            if test_suite_name in suite["name"]:
                return suite["artifact_dir"]
    return ""


def do_variable_substitution(input: str) -> str:
    """Substitute input with values defined in VARIABLE_SUBSTITUTION."""
    for var in VARIABLE_SUBSTITUTION:
        input = input.replace(var, VARIABLE_SUBSTITUTION[var])
    return input


def run_host_script(
    script_bin: str, script_args: list[str]
) -> subprocess.CompletedProcess:
    """Runs the host script."""
    substituted_args = []
    for arg in script_args:
        substituted_args.append(do_variable_substitution(arg))
    return subprocess.run(
        [script_bin] + substituted_args, text=True, stderr=subprocess.PIPE
    )


class RunTargetTestWithHostScript(unittest.TestCase):
    """Executes the target test via `ffx test`, then executes the host script in tearDown."""

    def __init__(self, args=None, test_output_dir=None):
        super().__init__()
        self.args = args
        self.test_output_dir = test_output_dir
        self.test_status = None

    def runTest(self):
        """Runs test on target"""
        self.test_status = run_target_test(
            self.args.ffx_bin,
            self.args.test_url,
            self.test_output_dir,
            self.args.ffx_test_args,
        )
        self.assertEqual(
            self.test_status.returncode,
            0,
            "test returned non-zero status, stderr: %s"
            % self.test_status.stderr,
        )

    def tearDown(self):
        if self.args.host_script_bin and (
            self.test_status.returncode == 0
            or self.args.run_host_script_on_fail
        ):
            artifact_subdir = get_test_suite_artifact_dir(
                self.test_output_dir, self.args.test_url
            )
            VARIABLE_SUBSTITUTION["{test_artifact_dir}"] = os.path.join(
                self.test_output_dir, artifact_subdir
            )
            script_status = run_host_script(
                self.args.host_script_bin, self.args.host_script_args
            )
            self.assertEqual(
                script_status.returncode,
                0,
                "host script returned non-zero status, stderr: %s"
                % script_status.stderr,
            )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--ffx-bin",
        type=pathlib.Path,
        required=True,
        help="Path to ffx binary.",
    )
    parser.add_argument("--test-url", type=str, required=True, help="Test URL")
    parser.add_argument(
        "--test-outdir", type=pathlib.Path, help="Dir to store test outputs"
    )
    parser.add_argument(
        "--ffx-test-args",
        action="append",
        default=[],
        help="List of args to be forwarded to ffx test",
    )
    parser.add_argument(
        "--host-script-bin",
        type=pathlib.Path,
        help="Path to host script, optional",
    )
    parser.add_argument(
        "--host-script-args",
        action="append",
        type=str,
        default=[],
        help="List of args to be forwarded to host script",
    )
    parser.add_argument(
        "--run-host-script-on-fail",
        help="whether to run the host script if test fails",
    )
    args = parser.parse_args()
    test_output_dir = os.getenv("FUCHSIA_TEST_OUTDIR")
    if not test_output_dir:
        test_output_dir = args.test_outdir
    print("storing test output to: ", test_output_dir)
    test = RunTargetTestWithHostScript(
        args=args, test_output_dir=test_output_dir
    )
    result = unittest.TextTestRunner().run(test)
    sys.exit(not result.wasSuccessful())


if __name__ == "__main__":
    main()
