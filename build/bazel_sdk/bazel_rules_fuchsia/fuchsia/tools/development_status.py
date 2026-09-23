#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import subprocess
from pathlib import Path

from fuchsia.tools.fuchsia_task_lib import Terminal


def run(*command):
    try:
        # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
        # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
        # inheritance which can confuse child Python processes.
        env = dict(os.environ)
        env.pop("RUNFILES_DIR", None)
        env.pop("RUNFILES_MANIFEST_FILE", None)

        return subprocess.check_output(
            command,
            env=env,
            text=True,
        ).strip()
    except subprocess.CalledProcessError as e:
        print(e.stdout)
        raise e


def run_checked(*command):
    try:
        # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
        # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
        # inheritance which can confuse child Python processes.
        env = dict(kwargs.get("env", os.environ))
        env.pop("RUNFILES_DIR", None)
        env.pop("RUNFILES_MANIFEST_FILE", None)
        kwargs["env"] = env

        subprocess.run(
            command,
            env=env,
            stderr=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
        ).check_returncode()
        return True
    except subprocess.CalledProcessError as e:
        return False


def print_title(msg):
    print(f'\n{Terminal.bold("-- {} --".format(msg))}\n')


def parse_args():
    """Parses arguments."""
    parser = argparse.ArgumentParser()

    def path_arg(type="file"):
        def arg(path):
            path = Path(path)
            if path.is_file() != (type == "file") or path.is_dir() != (
                type == "directory"
            ):
                parser.error(f'Path "{path}" is not a {type}!')
            return path

        return arg

    parser.add_argument(
        "--ffx",
        type=path_arg(),
        help="A path to the ffx tool.",
        required=True,
    )

    parser.add_argument(
        "--name",
        type=str,
        help="The name of the configuration.",
        required=True,
    )

    parser.add_argument(
        "--expected_emulator",
        type=str,
        help="The expected name of the running emulator",
        required=False,
    )

    parser.add_argument(
        "--expected_package_repo",
        type=str,
        help="The expected package repository",
        required=False,
    )

    parser.add_argument(
        "--expected_product_bundle",
        type=str,
        help="The expected product bundle url",
        required=False,
    )

    parser.add_argument(
        "--expected_product_bundle_repo",
        type=str,
        help="The expected product bundle repo name",
        required=False,
    )

    parser.add_argument(
        "--expected_sdk_version",
        type=str,
        help="The expected sdk version",
        required=False,
    )

    parser.add_argument(
        "--expected_product_name",
        type=str,
        help="The expected product name",
        required=False,
    )

    return parser.parse_args()


class Emulator:
    def __init__(self, active, name):
        self.active = active
        self.name = name


class BuildInfo:
    def __init__(self, sdk_version, product_config):
        self.sdk_version = sdk_version
        self.product_config = product_config


def all_emulators(args):
    raw_emulators = run(args.ffx, "emu", "list")
    if not raw_emulators:
        return []

    emulators = []
    for emulator in raw_emulators.split("\n"):
        emulator_entries = [v for v in emulator.split(" ") if v]
        (active, name) = (emulator_entries[0], emulator_entries[1])
        emulators.append(Emulator(active == "[Active]", name))
    return emulators


def all_package_repo_names(args):
    repos = json.loads(run(args.ffx, "--machine", "JSON", "repository", "list"))
    return [repo["name"] for repo in repos]


def target_reachable(args, target):
    return run_checked(
        args.ffx, "--target", target, "target", "wait", "-t", "5"
    )


def get_current_default_target(args):
    return run(args.ffx, "target", "default", "get")


def is_repo_registered_with_target(args, repo, target):
    # target repository list does not support JSON output so we have to parse the output.
    # this is fragile so migrate once json is supported.
    output = run(args.ffx, "target", "repository", "list")
    output_rows = output.split("\n")
    for i, row in enumerate(output_rows):
        if row.find(repo) > 0:
            for j in range(i, len(output_rows)):
                if output_rows[j].startswith("+"):
                    break
                if output_rows[j].find(target) > 0:
                    return True
    return False


def build_info_for_target(args, target):
    if not target_reachable(args, target):
        return ""

    result = json.loads(
        run(args.ffx, "--machine", "json", "--target", target, "target", "show")
    )

    sdk_version = result["build"]["version"]
    product_config = result["build"]["product"]

    return BuildInfo(sdk_version=sdk_version, product_config=product_config)


def is_product_bundle_downloaded(args):
    # product list does not support JSON output so we have to parse the output.
    # this is fragile so migrate once json is supported.
    output = run(args.ffx, "product", "list")
    pb_url = args.expected_product_bundle
    for line in output.split("\n"):
        if line.find(pb_url) >= 0 and line.startswith("*"):
            return True
    return False


def show_header(args):
    sdk_version = run(args.ffx, "sdk", "version")
    print("")
    print(
        f'{Terminal.bold("Status of development environment: {}".format(args.name))}'
    )
    print(f"  -  Current SDK Version: {sdk_version}")


def show_target_summary(args, current_default_target, print_pass, print_fail):
    print_title("Checking Target Status")
    if current_default_target:
        print_pass(f'default target set to "{current_default_target}"')
    else:
        print_fail("no default target specified")

    print(f'\n{Terminal.bold("Known Targets:")}')
    try:
        targets = json.loads(
            run(args.ffx, "--machine", "json", "target", "list")
        )
    except:
        targets = []

    if len(targets) > 0:
        [print(f'- {t["nodename"]} ({t["target_type"]})') for t in targets]
    else:
        print_fail("no known targets")

    if args.expected_emulator:
        show_emulator_status(
            args,
            args.expected_emulator,
            current_default_target,
            print_pass,
            print_fail,
        )

    if current_default_target != args.expected_emulator:
        show_reachability_of_target(
            args, current_default_target, print_pass, print_fail
        )


def show_emulator_status(
    args, emulator, default_target, print_pass, print_fail
):
    print_title('Checking status of emulator "{}"'.format(emulator))
    if emulator == default_target:
        print_pass(f'expected emulator "{emulator}" is the default target')
    else:
        print_fail(
            f'expected "{emulator}" to be the default target but it is not'
        )

    show_reachability_of_target(args, emulator, print_pass, print_fail)


def show_reachability_of_target(args, target, print_pass, print_fail):
    is_reachable = target_reachable(args, target)
    if is_reachable:
        print_pass(f"{target} is running and reachable")
        show_build_info_status(args, target, print_pass, print_fail)
    else:
        print_fail(f"{target} is not running")


def show_build_info_status(args, target, print_pass, print_fail):
    build_info = build_info_for_target(args, target)
    if args.expected_sdk_version:
        try:
            datetime.datetime.fromisoformat(build_info.sdk_version)
            print(f'Cannot determine SDK version for "{target}"')
            print(f"{target} is running a build from {build_info.sdk_version}")
        except:
            if build_info.sdk_version == args.expected_sdk_version:
                print_pass(
                    f'{target} is running the expected sdk version of "{args.expected_sdk_version}"'
                )
            else:
                print_fail(
                    f'{target} is running sdk version "{build_info.sdk_version}" which does not match the expected "{args.expected_sdk_version}"'
                )

    if args.expected_product_name:
        expected_product_config = args.expected_product_name.split(".")[0]
        if build_info.product_config == expected_product_config:
            print_pass(
                f'{target} is running the expected product "{expected_product_config}"'
            )
        else:
            print_fail(
                f'{target} is running product "{build_info.product_config}" which does not match the expected "{expected_product_config}"'
            )


def show_package_repo_status(args, default_target, print_pass, print_fail):
    print_title("Checking status of package repositories")
    print(f'{Terminal.bold("Known Package Repositories:")}')

    # FIX IF NOT HERE

    repos = all_package_repo_names(args)
    if repos:
        [print(f" - {r}") for r in repos]
    else:
        print_fail("No active repositories")

    current_default_repo = run(args.ffx, "repository", "default", "get")
    if args.expected_package_repo:
        print("")
        if current_default_repo == args.expected_package_repo:
            print_pass(
                f'package repository "{current_default_repo}" is the default repository'
            )
        else:
            print_fail(
                f'expected repository "{args.expected_package_repo}" to be the default repository but it is not'
            )

    show_status_of_repository_registration(
        args, args.expected_package_repo, default_target, print_pass, print_fail
    )


def show_status_of_repository_registration(
    args, repo, default_target, print_pass, print_fail
):
    if is_repo_registered_with_target(args, repo, default_target):
        print_pass(
            f'package repository "{repo}" is registered with the default target'
        )
    else:
        print_fail(
            f'package repository "{repo}" is not registered with the default target'
        )

    if args.expected_emulator:
        if is_repo_registered_with_target(args, repo, args.expected_emulator):
            print_pass(
                f'package repository "{repo}" is registered with emulator "{args.expected_emulator}"'
            )
        else:
            print_fail(
                f'package repository "{repo}" is not registered with emulator "{args.expected_emulator}"'
            )


def show_product_bundle_status(args, default_target, print_pass, print_fail):
    if not args.expected_product_bundle:
        return

    print_title("Checking status of product bundle")
    if is_product_bundle_downloaded(args):
        print_pass(f"{args.expected_product_bundle} is downloaded.")
    else:
        print_fail(f"{args.expected_product_bundle} is not downloaded.")

    if args.expected_product_bundle_repo:
        repos = all_package_repo_names(args)
        if args.expected_product_bundle_repo in repos:
            print_pass(
                f'package repository hosting the product bundle packages "{args.expected_product_bundle_repo}" exists.'
            )
        else:
            print_fail(
                f'product bundle does not have a package repository "{args.expected_product_bundle_repo}"'
            )

    show_status_of_repository_registration(
        args,
        args.expected_product_bundle_repo,
        default_target,
        print_pass,
        print_fail,
    )


def main():
    args = parse_args()
    default_target = get_current_default_target(args)
    failures = []

    def print_pass(msg):
        print(f' {Terminal.green("PASS")} - {msg}')

    def print_fail(msg):
        failures.append(msg)
        print(f' {Terminal.red("FAIL")} - {msg}')

    show_header(args)
    show_target_summary(args, default_target, print_pass, print_fail)
    show_package_repo_status(args, default_target, print_pass, print_fail)
    show_product_bundle_status(args, default_target, print_pass, print_fail)

    if len(failures) > 0:
        print("")
        print(
            f'{Terminal.bold("Some checks failed. To fix these problems run:")}'
        )
        print(" bazel run {}".format(args.name))


if __name__ == "__main__":
    main()
