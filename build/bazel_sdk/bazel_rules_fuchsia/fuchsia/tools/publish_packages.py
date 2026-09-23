#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path
from shutil import rmtree

from fuchsia.tools.fuchsia_task_lib import *


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
        raise TaskExecutionException(f"Command {command} failed.")


class FuchsiaTaskPublish(FuchsiaTask):
    def __init(self):
        self.prompt_repo_cleanup = True

    def parse_args(self, parser: ScopedArgumentParser) -> argparse.Namespace:
        """Parses arguments."""

        parser.add_argument(
            "--ffx",
            type=parser.path_arg(),
            help="A path to the ffx tool.",
            required=True,
        )
        parser.add_argument(
            "--packages",
            help="Paths to far files to package.",
            nargs="+",
            type=parser.path_arg(),
            required=True,
        )
        parser.add_argument(
            "--repo_name",
            help="Optionally specify the repository name.",
            scope=ArgumentScope.GLOBAL,
        )
        parser.add_argument(
            "--target",
            help="Optionally specify the target fuchsia device.",
            required=False,
            scope=ArgumentScope.GLOBAL,
        )

        parser.add_argument(
            "--publish_only",
            help="Only publish packages to an existing repo, not modifying device state",
            action="store_true",
            required=False,
        )

        # Private arguments.
        # Whether to make the repo default.
        parser.add_argument(
            "--make_repo_default",
            action="store_true",
            help=argparse.SUPPRESS,
            required=False,
        )
        # The effective repo path to publish to.
        parser.add_argument(
            "--repo_path",
            help=argparse.SUPPRESS,
            required=False,
        )
        return parser.parse_args()

    def ensure_target_device(self, args):
        if args.publish_only:
            return
        args.target = args.target or run(args.ffx, "target", "default", "get")
        print(f"Waiting for {args.target} to come online (60s)")
        ffx_cmd = [args.ffx]
        if args.target.strip():
            ffx_cmd += ["--target", args.target.strip()]
        ffx_cmd += ["target", "wait", "-t", "60"]
        run(*ffx_cmd)

    def resolve_repo(self, args):
        if args.publish_only:
            return

        # Determine the repo name we want to use, in this order:
        # 1. User specified argument.
        def user_specified_repo_name():
            if args.repo_name:
                print(f"Using manually specified --repo_name: {args.repo_name}")
                # We shouldn't ask to delete a user specified --repo_name.
                self.prompt_repo_cleanup = False
            return args.repo_name

        # 2. ffx default repository.
        def ffx_default_repo():
            default = run(
                args.ffx,
                "repository",
                "default",
                "get",
            )
            if default:
                print(f"Using ffx default repository: {default}")
                # We shouldn't ask to delete the ffx default repo.
                self.prompt_repo_cleanup = False
            return default

        # 3. A user prompt.
        def prompt_repo():
            print(
                "--repo_name was not specified and there is no default ffx repository set."
            )
            repo_name = input("Please specify a repo name to publish to: ")
            if args.make_repo_default is None:
                args.make_repo_default = (
                    input(
                        "Would you make this repo your default ffx repo? (y/n): "
                    ).lower()
                    == "y"
                )
            # We shouldn't ask to delete a repo that we're going to make default.
            self.prompt_repo_cleanup = not args.make_repo_default
            return repo_name

        args.repo_name = (
            user_specified_repo_name() or ffx_default_repo() or prompt_repo()
        )

    def _get_running_repo(self, args):
        """Returns the repository server information that matches args.repo_name or None."""
        # Determine the package repo path (use the existing one from ffx, or create a new one).
        #  {
        #    "ok": {
        #      "data": [
        #        {
        #          "name": "default",
        #          "address": "[::]:8083",
        #          "repo_path": {
        #            "file": "/usr/local/google/home/wilkinsonclay/tq/fuchsia/out/default/amber-files"
        #          },
        #          "registration_aliases": [],
        #          "registration_storage_type": "ephemeral",
        #          "registration_alias_conflict_mode": "replace",
        #          "server_mode": "background",
        #          "pid": 2935703
        #        }
        #      ]
        #    }
        #  }

        existing_repos = json.loads(
            run(
                args.ffx,
                "--machine",
                "json",
                "repository",
                "server",
                "list",
            )
        )
        if "ok" in existing_repos:
            repo_data = existing_repos["ok"]["data"]
        else:
            raise ValueError(
                f"Invalid response from ffx repository list: {existing_repos}"
            )

        for repo in repo_data:
            if repo["name"] == args.repo_name:
                return repo
        return None

    def ensure_repo(self, args):
        """Ensures that the repository is initialized and starts the repo server if needed."""
        if args.publish_only:
            return

        # Check for a running repository with the same name.
        existing_repo = self._get_running_repo(args)
        print(f"_get_running_repo returned: {existing_repo}")
        if existing_repo and "repo_path" in existing_repo:
            args.repo_path = Path(existing_repo["repo_path"]["file"])
            print(f"Using existing repo: {args.repo_name}: {args.repo_path}")
        else:
            args.repo_path = Path(tempfile.mkdtemp())
            print(f"Using new repo: {args.repo_name}: {args.repo_path}")

        # Ensure repository.
        if (args.repo_path / "repository").is_dir():
            print(f"Using existing repo: {args.repo_path}")
        else:
            print(f"Creating a new repository: {args.repo_path}")
            run(
                args.ffx,
                "repository",
                "create",
                "--time-versioning",
                args.repo_path,
            )

        print(
            "Running repo servers:",
            run(
                args.ffx,
                "--machine",
                "json-pretty",
                "repository",
                "server",
                "list",
            ),
        )

        # Start the repository server if it is not already running.
        if not existing_repo:
            run(
                args.ffx,
                "repository",
                "server",
                "start",
                "--no-device",
                "--address",
                "[::]:0",
                "--background",
                "--repository",
                args.repo_name,
                "--repo-path",
                args.repo_path,
            )

        # Ensure ffx target repository. This has to be done after starting the server.
        # We don't add the typical aliases here, since we use the repo_name to construct
        # the package URL.
        print(f"Registering {args.repo_name} to target device {args.target}")
        ffx_cmd = [args.ffx]
        if args.target.strip():
            ffx_cmd += ["--target", args.target.strip()]
        ffx_cmd += ["target", "repository", "register", "-r", args.repo_name]
        run(*ffx_cmd)

        # Optionally make the ffx repository default.
        if args.make_repo_default:
            old_default = run(args.ffx, "repository", "default", "get")
            print(
                f'Setting default ffx repository "{old_default}" => "{args.repo_name}"'
            )
            run(
                args.ffx,
                "repository",
                "default",
                "set",
                args.repo_name,
            )

    def publish_packages(self, args):
        print(f"Publishing packages: {args.packages}")
        cmd = [
            args.ffx,
            "repository",
            "publish",
            "--time-versioning",
        ]
        for package in args.packages:
            cmd.append("--package-archive")
            cmd.append(package)

        cmd.append(args.repo_path)

        run(*cmd)
        print(f"Published {len(args.packages)} packages")

    def teardown(self, args):
        if args.publish_only:
            return
        if self.prompt_repo_cleanup:
            input("Press enter to delete this repository, or ^C to quit.")

            # stop the repo server
            run(args.ffx, "repository", "server", "stop", args.repo_name)

            # Delete the package repository.
            rmtree(args.repo_path)
            print(f"Deleted {args.repo_path}")

    def run(self, parser: ScopedArgumentParser) -> None:
        # Parse arguments.
        args = self.parse_args(parser)

        # Check environment and gather information for publishing.

        self.ensure_target_device(args)
        self.resolve_repo(args)
        self.ensure_repo(args)
        # Perform the publishing.
        self.publish_packages(args)

        self.workflow_state["environment_variables"][
            "FUCHSIA_REPO_NAME"
        ] = args.repo_name

        # Optionally cleanup the repo.
        self.teardown(args)


if __name__ == "__main__":
    FuchsiaTaskPublish.main()
