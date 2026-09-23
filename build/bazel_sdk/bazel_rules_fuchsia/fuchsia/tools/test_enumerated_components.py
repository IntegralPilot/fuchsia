#!/usr/bin/env python3
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import re
import subprocess
from collections.abc import Iterable
from enum import Enum

from fuchsia.tools.fuchsia_task_lib import *


class TestingResult(Enum):
    PASS = 1
    FLAKE = 2
    FAIL = 3


class FuchsiaTaskTestEnumeratedComponents(FuchsiaTask):
    def parse_args(self, parser: ScopedArgumentParser) -> argparse.Namespace:
        """Parses arguments."""

        parser.add_argument(
            "--ffx-package",
            type=parser.path_arg(),
            help="A path to the ffx-package subtool.",
            required=True,
        )
        parser.add_argument(
            "--ffx-test",
            type=parser.path_arg(),
            help="A path to the ffx-test subtool.",
            required=True,
        )
        parser.add_argument(
            "--url",
            type=str,
            help="The full component url.",
            required=True,
        )
        parser.add_argument(
            "--package-manifest",
            type=parser.path_arg(),
            help="A path to the package manifest json file.",
            required=True,
        )
        parser.add_argument(
            "--package-archive",
            type=parser.path_arg(),
            help="A path to the package archive (.far) file.",
            required=True,
        )
        parser.add_argument(
            "--match-component-name",
            help="The regex allowlist used to filter for component names used for testing.",
            default=r".+",
        )
        parser.add_argument(
            "--target",
            help="Optionally specify the target fuchsia device.",
            required=False,
            scope=ArgumentScope.GLOBAL,
        )
        parser.add_argument(
            "--realm",
            help="Optionally specify the target realm to run this test.",
            required=False,
            scope=ArgumentScope.GLOBAL,
        )
        parser.add_argument(
            "--retries",
            help="An additional amount of test attempts per test failure.",
            type=int,
            default=0,
        )
        parser.add_argument(
            "--disable-retries-on-failure",
            help="""Turns off test retrying for all tests once a test actually fails by reaching the `--retries` cap.

            Useful for preventing an excessive amount of test retries for real caught regressions.""",
            action="store_true",
        )
        return parser.parse_args()

    def run_cmd_with_retries(
        self, retries: int, cmd: Iterable[str]
    ) -> TestingResult:
        for i in range(retries + 1, 0, -1):
            retries_left = i - 1
            is_first_try = retries_left == retries
            try:
                # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
                # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
                # inheritance which can confuse child Python processes.
                env = dict(os.environ)
                env.pop("RUNFILES_DIR", None)
                env.pop("RUNFILES_MANIFEST_FILE", None)

                subprocess.check_call(cmd, env=env)
                return (
                    TestingResult.PASS if is_first_try else TestingResult.FLAKE
                )
            except subprocess.CalledProcessError as e:
                if e.returncode != 1:
                    raise e
                if retries_left:
                    print(
                        Terminal.warn(
                            f"Test failed, retrying {retries_left} more times."
                        )
                    )
        return TestingResult.FAIL

    def run(self, parser: ScopedArgumentParser) -> None:
        args = self.parse_args(parser)
        target_args = ["--target", args.target] if args.target else []
        url_template = (
            args.url.replace(
                "{{PACKAGE_NAME}}",
                json.loads(args.package_manifest.read_text())["package"][
                    "name"
                ],
            )
            if args.package_manifest
            else args.url
        )

        try:
            # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
            # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
            # inheritance which can confuse child Python processes.
            env = dict(os.environ)
            env.pop("RUNFILES_DIR", None)
            env.pop("RUNFILES_MANIFEST_FILE", None)

            contents = subprocess.check_output(
                [
                    args.ffx_package,
                    *target_args,
                    "package",
                    "archive",
                    "list",
                    args.package_archive,
                ],
                env=env,
                text=True,
            )
            component_names = re.findall(r"\bmeta/(\S+)\.cm\b", contents)
        except subprocess.CalledProcessError as e:
            raise TaskExecutionException(
                f"Failed to enumerate components for testing!"
            )

        test_component_names = []
        skipped_component_names = []
        for component_name in component_names:
            if re.match(args.match_component_name, component_name):
                test_component_names.append(component_name)
            else:
                skipped_component_names.append(component_name)

        if skipped_component_names:
            print(
                Terminal.warn(
                    f"Skipping {len(skipped_component_names)} enumerated components: {', '.join(skipped_component_names)}"
                )
            )
        else:
            print("No enumerated components were skipped.")

        print(
            f"Testing {len(test_component_names)} enumerated components: {', '.join(test_component_names)}"
        )

        retry_budget = args.retries
        failing_tests = []
        flaking_tests = []
        for i, component_name in enumerate(test_component_names):
            url = url_template.replace(
                "{{META_COMPONENT}}", f"meta/{component_name}.cm"
            )
            print(
                f"Testing enumerated component {i + 1} of {len(test_component_names)}: {url}"
            )
            test_result = self.run_cmd_with_retries(
                retry_budget,
                [
                    args.ffx_test,
                    *target_args,
                    "test",
                    "run",
                    *(["--realm", args.realm] if args.realm else []),
                    url,
                ],
            )
            if test_result == TestingResult.FLAKE:
                flaking_tests.append(url)
            elif test_result == TestingResult.FAIL:
                failing_tests.append(url)
                if args.disable_retries_on_failure:
                    print(
                        Terminal.warn(
                            f"Disabling retries since {url} failed. Subsequent failures may be flaky false positives."
                        )
                    )
                    retry_budget = 0

        if flaking_tests:
            print(
                Terminal.warn(
                    f"There were {len(flaking_tests)} flaking tests:\n"
                    + "\n".join(flaking_tests)
                )
            )

        if failing_tests:
            raise TaskExecutionException(
                "Test Failures!\nFailing Tests:\n" + "\n".join(failing_tests),
                is_caught_failure=True,
            )


if __name__ == "__main__":
    FuchsiaTaskTestEnumeratedComponents.main()
