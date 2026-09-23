# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from dataclasses import dataclass
import datetime
import os
import subprocess
import typing

import args
from dataparse import dataparse


class EnvironmentError(Exception):
    """There was an error loading the execution environment."""


@dataparse
@dataclass
class ExecutionEnvironment:
    """Contains the parsed environment for this invocation of fx test.

    The environment provides paths to the Fuchsia source directory, output
    directory, input files, and output files.
    """

    # The Fuchsia source directory, from the FUCHSIA_DIR environment variable.
    fuchsia_dir: str

    # The output build directory for compiled Fuchsia code.
    out_dir: str

    # Path to the input tests.json file.
    test_json_file: str

    # Path to //sdk/ctf/disabled_tests.json
    disabled_ctf_tests_file: str

    # The Gemini API key, from the GEMINI_API_KEY environment variable.
    gemini_api_key: str | None = None

    # Path to the log file to write to. If unset, do not log.
    log_file: str | None = None

    # Path to the input test-list.json file.
    test_list_file: str | None = None

    # Path to the package-repositories.json file.
    package_repositories_file: str | None = None

    # Path to the USB Driver socket.
    usb_socket_path: str | None = None

    @classmethod
    def initialize_from_args(
        cls: typing.Type[typing.Self],
        flags: args.Flags,
        create_log_file: bool = True,
    ) -> typing.Self:
        """Initialize an execution environment from the given flags.

        Args:
            flags (args.Flags): Parsed command line flags.
            create_log_file (bool): If not set, do not log if
                the log file does not already exist.

        Raises:
            EnvironmentError: If the environment is not valid for some reason.

        Returns:
            ExecutionEnvironment: The processed environment for execution.
        """
        fuchsia_dir = os.getenv("FUCHSIA_DIR")
        if not fuchsia_dir or not os.path.isdir(fuchsia_dir):
            raise EnvironmentError(
                "Expected a directory in environment variable FUCHSIA_DIR"
            )

        # Get the build directory.
        out_dir: str
        if dir_from_fx := os.getenv("FUCHSIA_BUILD_DIR_FROM_FX"):
            # We were passed a build directory path from fx itself, use
            # that one.
            out_dir = dir_from_fx
        else:
            # Use the FUCHSIA_DIR to find the build directory.
            # We could use fx status, but it's slow to execute now. We
            # don't actually need all of the status contents to find the
            # build directory, it is stored at this file path in the root
            # Fuchsia directory during build time.
            build_dir_file = os.path.join(fuchsia_dir, ".fx-build-dir")
            if not os.path.isfile(build_dir_file):
                raise EnvironmentError(
                    f"Expected file .fx-build-dir at {build_dir_file}"
                )
            with open(build_dir_file) as f:
                out_dir = os.path.join(fuchsia_dir, f.readline().strip())
        if not os.path.isdir(out_dir):
            raise EnvironmentError(
                f"Expected directory at {out_dir}. Ensure you have set up your build directory correctly using 'fx set'."
            )

        usb_socket_path = flags.ffx_usb_socket_path
        if not usb_socket_path:
            # This looks heavy but we're just snagging a config value out of the
            # caller's config. No special subprocess wrapper since this is the
            # one place we *want* to leak the caller's config into our context.
            cmd = [
                "fx",
                "--dir",
                out_dir,
                "ffx",
                "config",
                "get",
                "connectivity.usb_socket_path",
            ]
            env = os.environ
            if (
                "XDG_RUNTIME_DIR" not in env
                and "FUCHSIA_XDG_RUNTIME_DIR_FROM_FX" in env
            ):
                env["XDG_RUNTIME_DIR"] = env["FUCHSIA_XDG_RUNTIME_DIR_FROM_FX"]
            try:
                result = subprocess.run(
                    cmd,
                    text=True,
                    stderr=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    env=env,
                )
                if result.returncode == 0:
                    got = result.stdout.replace('"', "").strip()
                    if got:
                        usb_socket_path = got
            except Exception:
                # TODO(512908834): Log something and/or narrow the exception class.
                pass

        # Either disable logging, log to the given path, or format
        # a default path in the output directory.
        # We will write gzipped logs since they can get a bit large
        # and compress very well.
        log_file = (
            None
            if not flags.log
            else (
                flags.logpath
                if flags.logpath
                else os.path.join(
                    out_dir,
                    f"fxtest-{datetime.datetime.now().isoformat()}.log.json.gz",
                )
            )
        )
        if not create_log_file and log_file and not os.path.isfile(log_file):
            log_file = None

        # Get the input files from their expected locations directly
        # under the output directory.
        tests_json_file = os.path.join(out_dir, "tests.json")
        disabled_ctf_tests_file = os.path.join(
            fuchsia_dir, "sdk/ctf/disabled_tests.json"
        )
        package_repositories_file = os.path.join(
            out_dir, "package-repositories.json"
        )
        for expected_file in [
            tests_json_file,
            disabled_ctf_tests_file,
        ]:
            if not os.path.isfile(expected_file):
                raise EnvironmentError(f"Expected a file at {expected_file}")
        return cls(
            fuchsia_dir,
            out_dir,
            tests_json_file,
            disabled_ctf_tests_file,
            gemini_api_key=os.getenv("GEMINI_API_KEY"),
            log_file=log_file,
            package_repositories_file=(
                package_repositories_file
                if os.path.isfile(package_repositories_file)
                else None
            ),
            usb_socket_path=usb_socket_path,
        )

    def relative_to_root(self, path: str) -> str:
        """Return the path to a file relative to the Fuchsia directory.

        This is used to format paths like "/home/.../fuchsia/src/my_lib" as
        "//src/my_lib".

        Args:
            path (str): Absolute path under the Fuchsia directory.

        Returns:
            str: Relative path from the Fuchsia directory to the
                same destination.
        """
        return os.path.relpath(path, self.fuchsia_dir)

    def get_most_recent_log(self) -> str:
        """Get the most recent log file for this environment.

        If this environment specifies a log file, return that one, otherwise
        search the output directory for log files and return the most recent
        one by name.

        Raises:
            EnvironmentError: If no log file could be found.

        Returns:
            str: Path to the most recent log file.
        """
        if self.log_file and not self.log_to_stdout():
            return self.log_file

        matching = [
            name
            for name in os.listdir(self.out_dir)
            if name.startswith("fxtest-") and name.endswith(".json.gz")
        ]

        matching.sort()
        if not matching:
            raise EnvironmentError(f"No log files found in {self.out_dir}")
        return os.path.join(self.out_dir, matching[-1])

    def log_to_stdout(self) -> bool:
        return self.log_file == args.LOG_TO_STDOUT_OPTION

    def fx_cmd_line(self, *args: str) -> list[str]:
        """Format the given arguments into a command line for `fx`.

        Returns:
            list[str]: The full command line to use.
        """

        cmd = ["fx", "--dir", self.out_dir]
        if (target := os.environ.get("FUCHSIA_NODENAME")) and {
            "-t",
            "--target",
        }.isdisjoint(args):
            cmd.extend(["-t", target])
        return cmd + list(args)

    def __hash__(self) -> int:
        return hash(self.fuchsia_dir)


@dataclass
class DeviceEnvironment:
    """Environment for connecting to a Fuchsia Device"""

    # IP address of the device
    address: str

    # SSH port for the device
    port: str

    # Name of the device
    name: str

    # Path to the private key used to SSH to the device
    private_key_path: str
