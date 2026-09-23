# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common utilities supporting Bazel query and build actions.

This module will be imported directly from other scripts, including some in
this directory, as well as ones in //build/api/. Its purpose is to provide
utility functions to perform Bazel operations in the Fuchsia workspace. It should
not depend on any other non-standard module, and the logic to create that workspace
should stay in workspace_utils.bzl instead.
"""

import argparse
import dataclasses
import errno
import hashlib
import json
import os
import shlex
import subprocess
import sys
import time
import typing as T
from collections.abc import Sequence
from pathlib import Path

if T.TYPE_CHECKING:
    import argparse


# A type that describes either a path string of a Path instance.
FilePath: T.TypeAlias = str | os.PathLike[T.Any]


# LINT.IfChange(bazel_topdir_config_file)
_BAZEL_TOPDIR_CONFIG_FROM_FUCHSIA_DIR = "build/bazel/config/bazel_top_dir"
# LINT.ThenChange(//build/bazel/bazel_workspace.gni:bazel_topdir_config_file)


def gn_arch_to_bazel(gn_arch: str) -> str:
    """Convert GN arch to Bazel arch, following Fuchsia conventions."""
    if gn_arch == "arm64":
        return "aarch64"
    elif gn_arch == "x64":
        return "x86_64"
    elif gn_arch == "riscv64":
        return "riscv64"
    else:
        raise ValueError(f"Unexpected GN arch: {gn_arch}")


def get_host_platform() -> str:
    """Return host platform name, following Fuchsia conventions."""
    if sys.platform == "linux":
        return "linux"
    elif sys.platform == "darwin":
        return "mac"
    else:
        return os.uname().sysname


def get_host_arch() -> str:
    """Return host CPU architecture, following Fuchsia conventions."""
    host_arch = os.uname().machine
    if host_arch == "x86_64":
        return "x64"
    elif host_arch.startswith(("armv8", "aarch64")):
        return "arm64"
    else:
        return host_arch


def get_host_tag() -> str:
    """Return host tag, following Fuchsia conventions."""
    return "%s-%s" % (get_host_platform(), get_host_arch())


def is_fuchsia_dir(path: FilePath) -> bool:
    """Return true if input path is a Fuchsia source directory."""
    return (Path(path) / ".jiri_manifest").exists()


def find_fuchsia_dir(from_path: FilePath | None = None) -> Path:
    """Find the Fuchsia checkout from a specific path.

    Args:
        from_path: Optional starting path for search. Defaults to the current directory.
    Returns:
        Path to the Fuchsia checkout directory (absolute).
    Raises:
        ValueError if the path could not be found.
    """
    start_path = Path(from_path).resolve() if from_path else Path.cwd()
    cur_path = start_path
    while True:
        if is_fuchsia_dir(cur_path):
            return cur_path
        prev_path = cur_path
        cur_path = cur_path.parent
        if cur_path == prev_path:
            raise ValueError(
                f"Could not find Fuchsia checkout directory from: {start_path}"
            )


def find_fx_build_dir(fuchsia_dir: FilePath) -> Path | None:
    """Find the build directory set through 'fx set' or 'fx use'.

    Args:
       fuchsia_dir: Path to Fuchsia checkout directory.
    Returns:
       Path to build directory if found, of None if none
       is available (e.g. fresh checkout or infra build).
    """
    fuchsia_dir_path = Path(fuchsia_dir)
    fx_build_dir_file = fuchsia_dir_path / ".fx-build-dir"
    if fx_build_dir_file.exists():
        build_dir_relative = fx_build_dir_file.read_text().strip()
        if build_dir_relative:
            build_dir = fuchsia_dir_path / build_dir_relative
            if build_dir.exists():
                return build_dir
    return None


def find_host_binary_path(program: str) -> Path | None:
    """Find the absolute path of a given program. Like the UNIX `which` command.

    Args:
        program: Program name.
    Returns:
        program's absolute path, found by parsing the content of $PATH.
        Or None if nothing is found.
    """
    for path in os.environ.get("PATH", "").split(":"):
        # According to Posix, an empty path component is equivalent to ´.'.
        if path == "" or path == ".":
            path = os.getcwd()
        candidate = os.path.realpath(os.path.join(path, program))
        if os.path.isfile(candidate) and os.access(
            candidate, os.R_OK | os.X_OK
        ):
            return Path(candidate)

    return None


def get_bazel_relative_topdir(fuchsia_dir: FilePath) -> tuple[str, set[Path]]:
    """Return Bazel topdir, relative to Ninja output dir.

    Args:
        fuchsia_dir: Fuchsia source directory path.
    Returns:
        A (topdir, input_files) pair, where input_files is a set of Path
        values corresponding to the file(s) read by this function.
    """
    input_file = Path(fuchsia_dir) / _BAZEL_TOPDIR_CONFIG_FROM_FUCHSIA_DIR
    assert input_file.exists(), f"Missing input file: {input_file}"
    return input_file.read_text().strip(), {input_file}


def find_bazel_launcher_path(
    fuchsia_dir: FilePath, build_dir: FilePath
) -> Path | None:
    """Find the path of the Bazel launcher script.

    Args:
        fuchsia_dir: Path to Fuchsia checkout directory.
        build_dir: Path to Fuchsia build directory.

    Returns:
        Path to bazel launcher script, or empty Path() value if the file
        does not exist.
    """
    bazel_topdir, _ = get_bazel_relative_topdir(fuchsia_dir)
    result = Path(build_dir) / bazel_topdir / "bazel"
    return result if result.exists() else None


def find_bazel_workspace_path(
    fuchsia_dir: FilePath, build_dir: FilePath
) -> Path | None:
    """Find the path of the Bazel workspace.

    Args:
        fuchsia_dir: Path to Fuchsia checkout directory.
        build_dir: Path to Fuchsia build directory.

    Returns:
        Path to bazel workspace, or None if the directory does not exists.
    """
    bazel_topdir, _ = get_bazel_relative_topdir(fuchsia_dir)
    result = Path(build_dir) / bazel_topdir / "workspace"
    return result if result.exists() else None


def force_symlink(dst_path: FilePath, target_path: FilePath) -> None:
    """Create a symlink at |dst_path| that points to |target_path|.

    The generated symlink target will always be a relative path.

    Args:
        dst_path: path to symlink file to write or update.
        target_path: path to actual symlink target.
    """
    dst_dir = os.path.dirname(dst_path)
    target_path = os.path.relpath(target_path, dst_dir)
    return force_raw_symlink(dst_path, target_path)


def force_raw_symlink(dst_path: FilePath, target_path: FilePath) -> None:
    """Create a symlink at |dst_path| that points to |target_path|.

    The generated symlink target will always be the unmodified |target_path|
    value, which can be absolute or relative, and/or point to a
    non-existing file.

    Args:
        dst_path: path to symlink file to write or update.
        target_path: raw symlink target path.
    """
    if os.path.lexists(dst_path):
        os.remove(dst_path)  # Remove previous symlink if it exists.
    dst_dir = os.path.dirname(dst_path)
    os.makedirs(dst_dir, exist_ok=True)
    try:
        os.symlink(target_path, dst_path)
    except OSError as e:
        if e.errno == errno.EEXIST:
            os.remove(dst_path)
            os.symlink(target_path, dst_path)
        else:
            raise


def force_symlink_to_ninja_artifact(
    link_path: FilePath, target_path: FilePath
) -> None:
    """Create a symlink pointing to a Ninja artifact.

    If the target path doesn't exist, create it with a timestamp of 0.
    This ensures that Bazel doesn't error when performing a query over the
    repository, even if the target artifact hasn't been built yet.

    Bazel builds still require a Ninja invocation, which will overwrite the
    target with an up-to-date artifact on the next invocation.

    Args:
        link_path: Path to the symlink to create.
        target_path: Path to the target artifact.
    """
    force_symlink(link_path, target_path)
    if not os.path.exists(target_path):
        os.makedirs(os.path.dirname(target_path), exist_ok=True)
        with open(target_path, "w") as f:
            f.write("")
        os.utime(target_path, (0, 0))


_HEXADECIMAL_SET = set("0123456789ABCDEFabcdef")


def is_hexadecimal_string(s: str) -> bool:
    """Return True if input string only contains hexadecimal characters."""
    return bool(s) and all([c in _HEXADECIMAL_SET for c in s])


_BUILD_ID_PREFIX = ".build-id/"


def is_likely_build_id_path(path: str) -> bool:
    """Return True if path is a .build-id/xx/yyyy* file name."""
    # Look for .build-id/XX/ where XX is an hexadecimal string.
    pos = path.find(_BUILD_ID_PREFIX)
    if pos < 0:
        return False

    if pos > 0 and path[pos - 1] != "/":
        return False

    path = path[pos + len(_BUILD_ID_PREFIX) :]
    if len(path) < 3 or path[2] != "/":
        return False

    return is_hexadecimal_string(path[0:2])


def is_likely_content_hash_path(path: str) -> bool:
    """Return True if file path is likely based on a content hash.

    Args:
        path: File path
    Returns:
        True if the path is a likely content-based file path, False otherwise.
    """
    # Look for .build-id/XX/ where XX is an hexadecimal string.
    if is_likely_build_id_path(path):
        return True

    # Look for a long hexadecimal sequence filename.
    filename = os.path.basename(path)
    return len(filename) >= 16 and is_hexadecimal_string(filename)


assert is_likely_content_hash_path("/src/.build-id/ae/23094.so")
assert not is_likely_content_hash_path("/src/.build-id/log.txt")


@dataclasses.dataclass(frozen=True)
class BazelBuildInvocation:
    """Models a single `bazel build` invocation from the Fuchsia build."""

    bazel_targets: list[str]  # Bazel target labels, cannot be empty.

    # Command-line arguments for build configuration. Can be empty.
    build_args: list[str] = dataclasses.field(default_factory=list)

    gn_label: str | None = None  # Optional GN label for the corresponding bazel_action() target, if any.

    gn_targets_dir: str | None = None
    # Optional path to @gn_targets repository content

    bazel_action_timings: dict[str, float] | None = None
    # Optional, dictionary describing the duration of bazel_gn_target_action.py steps
    # keys are step names, values are durations in seconds.

    def __post_init__(self) -> None:
        if not self.bazel_targets:
            raise ValueError(f"Empty bazel_targets list")

    def to_json(self) -> dict[str, T.Any]:
        """Convert instance to JSON object.

        Returns:
            JSON object as a dictionary.
        """
        result: dict[str, T.Any] = {
            "bazel_targets": self.bazel_targets,
            "build_args": self.build_args,
        }
        if self.gn_label:
            result["gn_label"] = self.gn_label
        if self.gn_targets_dir:
            result["gn_targets_dir"] = self.gn_targets_dir
        if self.bazel_action_timings:
            result["bazel_action_timings"] = self.bazel_action_timings
        return result

    @staticmethod
    def from_json(d: dict[str, T.Any]) -> "BazelBuildInvocation":
        """Convert JSON object into new instance value.

        Args:
            json: A JSON object as a Python dictionary.
        Returns:
            new instance value.
        Raises:
            ValueError if the input is malformed.
        """
        if not isinstance(d, dict):
            raise ValueError(f"Input JSON is not an object: {d}")
        try:
            return BazelBuildInvocation(
                bazel_targets=d["bazel_targets"],
                build_args=d["build_args"],
                gn_label=d.get("gn_label"),
                gn_targets_dir=d.get("gn_targets_dir"),
                bazel_action_timings=d.get("bazel_action_timings"),
            )
        except KeyError as e:
            raise ValueError(f"Missing JSON object key {e}")
        except TypeError as e:
            raise ValueError(f"Missing JSON object key {e}")


class LastBazelBuildInvocations(object):
    """Models the list of last Bazel build invocations from the Fuchsia build.

    An `fx build` or `fint build` command will populate the file
    at $BUILD_DIR/{LastBazelBuildInvocations.FILENAME} with details about
    each `bazel build` command that was invoked through `bazel_action()`
    targets. This can later be used by //build/api/client to perform
    queries to extract related information, such as debug symbols.

    Its content is a JSON array of objects describing BazelBuildInvocation
    instances.
    """

    # Name of the file created / updated by `fx build` and `fint build`.
    # LINT.IfChange(last_bazel_build_invocations_file)
    FILENAME = "last_bazel_build_invocations.json"
    # LINT.ThenChange(//tools/devshell/build:last_bazel_build_invocations_file,//tools/integration/fint/build.go:last_bazel_build_invocations_file,//tools/artifactory/cmd/up.go:last_bazel_build_invocations_file)

    def __init__(self, invocations: list[BazelBuildInvocation] = []):
        """Constructor. Do not use directly, see new_from_xxxx() methods."""
        self._invocations = invocations

    @property
    def invocations(self) -> list[BazelBuildInvocation]:
        return self._invocations

    def append(self, invocation: BazelBuildInvocation) -> None:
        """Append new BazelBuildInvocation instance."""
        self._invocations.append(invocation)

    @staticmethod
    def get_build_file_path(build_dir: FilePath) -> Path:
        """Retrieve the path of the corresponding file in the Ninja build directory."""
        return Path(build_dir) / LastBazelBuildInvocations.FILENAME

    @staticmethod
    def new_from_build(build_dir: FilePath) -> "LastBazelBuildInvocations":
        """Create new instance from the Ninja build directory.

        Args:
            build_dir: Path to Ninja build directory.
        Returns:
            new instance value.
        Raises:
            ValueError if file is malformed
        """
        (
            file_path,
            last_invocations_json,
        ) = LastBazelBuildInvocations._load_json_from_build_dir(build_dir)

        return LastBazelBuildInvocations.new_from_json(last_invocations_json)

    @staticmethod
    def append_to_build_dir(
        build_dir: FilePath, invocation: BazelBuildInvocation
    ) -> None:
        """Append a new BazelBuildInvocation instance to the Ninja build directory

        Args:
            build_dir: Path to the Ninja build directory.
            invocation: BazelBuildInvocation value to add.
        """
        (
            file_path,
            last_invocations_json,
        ) = LastBazelBuildInvocations._load_json_from_build_dir(build_dir)

        last_invocations_json.append(invocation.to_json())

        with open(file_path, "wt") as f:
            json.dump(last_invocations_json, f)

    @staticmethod
    def _load_json_from_build_dir(
        build_dir: FilePath,
    ) -> tuple[Path, list[dict[str, T.Any]]]:
        """Load JSON data from the build directory."""
        file_path = LastBazelBuildInvocations.get_build_file_path(build_dir)
        with open(file_path, "rt") as f:
            last_invocations_json = json.load(f)
        return file_path, last_invocations_json

    def write_to_build_dir(self, build_dir: FilePath) -> None:
        """Write instance to the Ninja build directory.

        Args:
            build_dir: Path to Ninja build directory.
        """
        file_path = self.get_build_file_path(build_dir)
        with open(file_path, "wt") as f:
            json.dump([i.to_json() for i in self._invocations], f)

    @staticmethod
    def new_from_json(
        json: list[dict[str, T.Any]],
    ) -> "LastBazelBuildInvocations":
        """Create new instance from JSON array

        Args:
            json: A JSON array as a Python list.
        Returns:
            new instance value.
        Raises:
            ValueError if input is malformed.
        """
        if not isinstance(json, list):
            raise ValueError(
                f"Input is not a JSON array, got {type(json)} instead!"
            )

        invocations: list[BazelBuildInvocation] = [
            BazelBuildInvocation.from_json(item) for item in json
        ]
        return LastBazelBuildInvocations(invocations)

    def to_json(self) -> list[dict[str, T.Any]]:
        """Convert instance to JSON array value."""
        return [invocation.to_json() for invocation in self._invocations]


# Callable object that takes log messages as input.
LogFunc = T.Callable[[str], None]


class TimeProfile(object):
    """Track duration of generation/build steps' start and end times.

    Usage is:
      1) Create instance.

      2) Call start() when starting a new step. Repeat as many times as needed.

      3) Optionally call stop() when a step has completed. Useful if some
         unrelated work needs to happen after the next start() call.

      4) Call print() to print a table detailing the timings of all
         steps over a given threshold.
    """

    def __init__(
        self,
        log: None | LogFunc = None,
        now: None | T.Callable[[], float] = None,
    ) -> None:
        """Constructor.

        Args:
            log: An optional callable that can be used to print step descriptions
               when start() is called.
            now: An optional callable that can be used to return the current time
               in seconds. Default is to use time.time. Only used for tests.
        """
        self._now = now if now else time.time
        self._start_time = self._now()
        self._steps: list[tuple[float, float, str]] = []
        self._log = log

    def start(self, name: str, description: str = "") -> None:
        """Start a new regeneration step (and stop the current one if any)

        Args:
            name: Step name (used in final print() output)
            description: Optional step description. Will be sent to the log
               if one was provided in the constructor.
        """
        if description and self._log:
            self._log(description)
        cur_time = self._close_last_step()
        self._steps.append((cur_time, 0, name))

    def stop(self) -> None:
        """Stop the current step (record its end time)."""
        self._close_last_step()

    def _close_last_step(self) -> float:
        cur_time = self._now()
        if self._steps:
            start_time, end_time, name = self._steps[-1]
            if end_time == 0:
                end_time = cur_time
                self._steps[-1] = (start_time, end_time, name)
        return cur_time

    def to_json_timings(self) -> dict[str, float]:
        """Generate a JSON object detailing the durtaion of each step.

        Returns:
           A dictionary mapping step names to durations in seconds.
           Keys are ordered according to step execution.
        """
        self._close_last_step()
        return {
            name: end_time - start_time
            for start_time, end_time, name in self._steps
        }

    def print(self, short_step_threshold: float = 0.0) -> None:
        """Print timings results for all recorded steps.

        Args:
            short_step_threshold: A threshold in seconds. Any step
                that was faster than this will be omitted from the
                output.
        """
        self._close_last_step()
        if short_step_threshold:
            print(
                "Timing results for regeneration steps slower than %.3f seconds:"
                % short_step_threshold
            )
        else:
            print("Timing results for all regeneration steps:")
        for step in self._steps:
            start_time, end_time, name = step
            duration = end_time - start_time
            if duration < short_step_threshold:
                continue
            print("%5.3fs   %s" % (end_time - start_time, name))


def log_stderr(msg: str) -> None:
    """A LogFunc implementation that prints messages to stderr."""
    print(msg, file=sys.stderr)


def cmd_args_to_string(cmd_args: Sequence[FilePath]) -> str:
    """Convert a list of command arguments to a printable string.

    Args:
        cmd_args: A list of either strings or file paths.
    Returns:
        A single string representing the shell-quoted command.
    """
    return " ".join(shlex.quote(str(c)) for c in cmd_args)


class BuildPaths(object):
    """Convenience class used to access important build-related paths."""

    @staticmethod
    def add_parser_arguments(
        parser: argparse.ArgumentParser,
        add_fuchsia_dir: bool = True,
        add_build_dir: bool = True,
    ) -> None:
        """Add path-related arguments to an ArgumentParser instance.

        Best used with the from_parser_args() method to create a new
        BuildPaths from the result. Example usage:

        ```
        def main():
            parser = argparse.ArgumentParser()
            BuildPaths.add_parser_arguments(parser)
            ...
            args = parser.parse_args()
            paths = BuildPaths.from_parser_args(args)
        ```

        Args:
            parser: An argparse.ArgumentParser instance.
            add_fuchsia_dir: Optional, set to False to omit a --fuchsia-dir argument.
            add_build_dir: Optional, set to False to omit a --build-dir argument.
        """
        if add_fuchsia_dir:
            parser.add_argument(
                "--fuchsia-dir",
                type=Path,
                help="Path to Fuchsia source directory (auto-detected).",
            )
        if add_build_dir:
            parser.add_argument(
                "--build-dir",
                type=Path,
                help="Path to Ninja build directory (auto-detected for local development only).",
            )

    @staticmethod
    def from_parser_args(args: argparse.Namespace) -> "BuildPaths":
        """Create new instance from a parse_args() value.

        Args:
            args: An argparse.Namespace value returned by
                argparse.ArgumentParser.parse_args()
        Returns:
            A new BuildPaths instance.
        Raises:
            ValueError if auto-detection didn't work or if one of the
            provided paths is invalid.
        """
        fuchsia_dir = args.fuchsia_dir if hasattr(args, "fuchsia_dir") else None
        build_dir = args.build_dir if hasattr(args, "build_dir") else None
        return BuildPaths(fuchsia_dir, build_dir)

    def __init__(
        self,
        fuchsia_dir: None | FilePath = None,
        build_dir: None | FilePath = None,
    ) -> None:
        """Create new instance.

        Arguments are optional and will be auto-detected according to the
        behavior described below. This is done mostly for the benefit of
        invoking build scripts manually during development, without having
        to add too many command-line arguments. All build-time actions are
        expected to provide explicit values.

        Args:
           fuchsia_dir: Optional Path to the Fuchsia source directory.
             If None, auto-detected by walking up from build_dir, or from
             the current work directory if the latter is None.

           build_dir: Optional Path to Ninja build directory.
             If None, auto-detected from fuchsia_dir using the .fx-build
             file (and if it doesn't exist, such as for infra builds, ValueError
             will be raised).

        Raises:
          ValueError if auto-detection failed to find fuchsia_dir or
          build_dir when these are not provided explicitly.
        """
        if fuchsia_dir:
            self._fuchsia_dir = Path(fuchsia_dir).resolve()
            if not is_fuchsia_dir(self._fuchsia_dir):
                raise ValueError(
                    f"Not a Fuchsia source directory: {fuchsia_dir}"
                )
        else:
            self._fuchsia_dir = find_fuchsia_dir(from_path=build_dir)

        final_build_dir = (
            Path(build_dir).resolve()
            if build_dir
            else find_fx_build_dir(self._fuchsia_dir)
        )
        if not final_build_dir:
            raise ValueError(
                f"Could not detect current build directory from Fuchsia directory: {self._fuchsia_dir}"
            )
        if not final_build_dir.is_dir():
            raise ValueError(
                f"Not a valid Fuchsia build directory: {build_dir}"
            )
        self._build_dir: Path = final_build_dir

        self._host_os = get_host_platform()
        self._host_cpu = get_host_arch()
        self._host_tag = f"{self._host_os}-{self._host_cpu}"

    @property
    def fuchsia_dir(self) -> Path:
        """Return absolute Path to Fuchsia source directory."""
        return self._fuchsia_dir

    @property
    def build_dir(self) -> Path:
        """Return absolute Path to Ninja build directory."""
        return self._build_dir

    @property
    def ninja_path(self) -> Path:
        """Return absolute path to Ninja binary."""
        return (
            self._fuchsia_dir
            / f"prebuilt/third_party/ninja/{self._host_tag}/ninja"
        )

    @property
    def gn_path(self) -> Path:
        """Return absolute path to GN binary."""
        return (
            self._fuchsia_dir / f"prebuilt/third_party/gn/{self._host_tag}/gn"
        )

    @property
    def python3_path(self) -> Path:
        """Return absolute path to python3 binary."""
        return (
            self._fuchsia_dir
            / f"prebuilt/third_party/python3/{self._host_tag}/bin/python3"
        )

    @property
    def bazel_binary_path(self) -> Path:
        """Return absolute path to Bazel binary.

        As opposed to BazelPaths.launcher which returns the path to a wrapper script
        that injects critical Fuchia-specific flags into the call.
        """
        return (
            self._fuchsia_dir
            / f"prebuilt/third_party/bazel/{self._host_tag}/bazel"
        )


class BazelPaths(BuildPaths):
    """Convenience class used to access important Bazel-related paths."""

    WORKSPACE_FROM_TOP_DIR = "workspace"
    OUTPUT_BASE_FROM_TOP_DIR = "output_base"
    OUTPUT_USER_ROOT_FROM_TOP_DIR = "output_user_root"
    LAUNCHER_FROM_TOP_DIR = "bazel"

    # The name of the root Bazel workspace as it appears in ${output_base}/execroot/.
    # When BzlMod is enabled, this is always "_main", independent from the
    # name of the root workspace / module name.
    WORKSPACE_NAME_FROM_EXECROOT = "_main"

    @staticmethod
    def new(
        fuchsia_dir: Path | None = None, build_dir: Path | None = None
    ) -> "BazelPaths":
        """Create new instance.

        Args:
            fuchsia_dir: Optional Path to the Fuchsia source directory.
            build_dir: Optional Path to the Ninja build directory.
        Returns:
            A new BazelPaths instance.
        Raises:
            ValueError if the fuchsia or build directories cannot be
            properly auto-detected or do not exist.
        """
        if fuchsia_dir:
            fuchsia_dir = fuchsia_dir.resolve()
        else:
            fuchsia_dir = find_fuchsia_dir(build_dir)

        if build_dir:
            build_dir = build_dir.resolve()
        else:
            build_dir = find_fx_build_dir(fuchsia_dir)
            if not build_dir:
                raise ValueError(
                    f"Could not detect current build-directory from Fuchsia directory: {fuchsia_dir}"
                )

        return BazelPaths(fuchsia_dir, build_dir)

    def __init__(self, fuchsia_dir: Path, build_dir: Path) -> None:
        super().__init__(fuchsia_dir, build_dir)
        """Construct new instance. Requires explicit fuchsia_dir and build_dir paths."""
        bazel_topdir, self._input_files = get_bazel_relative_topdir(
            self._fuchsia_dir
        )
        self._top_dir = self._build_dir / bazel_topdir

    @property
    def top_dir(self) -> Path:
        """Return the Bazel top directory."""
        return self._top_dir

    @staticmethod
    def write_topdir_config_for_test(fuchsia_dir: Path, content: str) -> Path:
        """Write a Bazel topdir configuration file. Useful for tests.

        Args:
           fuchsia_dir: Path to Fuchsia source tree.
           content: Config file content, i.e. path of the Bazel topdir relative
              to the Ninja build directory.
        Returns:
           Path of the config file that was written.
        """
        config_file = fuchsia_dir / _BAZEL_TOPDIR_CONFIG_FROM_FUCHSIA_DIR
        config_file.parent.mkdir(parents=True, exist_ok=True)
        config_file.write_text(content)
        return config_file

    @property
    def ninja_build_dir(self) -> Path:
        return self._build_dir

    @property
    def execroot(self) -> Path:
        """Return the Bazel execroot path."""
        return (
            self._top_dir
            / self.OUTPUT_BASE_FROM_TOP_DIR
            / "execroot"
            / self.WORKSPACE_NAME_FROM_EXECROOT
        )

    @property
    def output_base(self) -> Path:
        """Return Path to the Bazel output base."""
        return self._top_dir / self.OUTPUT_BASE_FROM_TOP_DIR

    @property
    def workspace(self) -> Path:
        """Return Path to the Bazel workspace root directory."""
        return self._top_dir / self.WORKSPACE_FROM_TOP_DIR

    @property
    def launcher(self) -> Path:
        """Return Path to the Bazel launcher script."""
        return self._top_dir / self.LAUNCHER_FROM_TOP_DIR

    @property
    def input_files(self) -> set[Path]:
        """Return list of input file paths that were read in the constructor.

        Useful for Ninja depfiles or //build/regenerator.py.
        """
        return self._input_files


@dataclasses.dataclass
class CommandResult:
    """The result of invoking CommandRunner.run_command().

    This is similar to subprocess.CompletedProcess[str], but it doesn't
    hold file descriptors open, and can be trivially instantiated for tests
    or mock CommandRunner instances.
    """

    returncode: int = 0
    stdout: str = ""
    stderr: str = ""
    args: list[str] = dataclasses.field(default=list)  # type: ignore


class CommandRunner(object):
    """Convenience class to run commands.

    A small wrapper around subprocess.run(), which allows logging invocations
    commands and results as well as allow mock implementations for tests to
    override the run_command_internal() method in derived classes.
    """

    # The following constants are convenience argument keyword dictionaries
    # that can be expanded in run_command() calls to specify how to capture outputs.
    # This also prevents the caller from depending on the subprocess module.
    # Capture both stdout and stderr separately.
    CAPTURE_KWARGS = {
        "stdout": subprocess.PIPE,
        "stderr": subprocess.PIPE,
    }

    # Capture both stdout and stderr into ret.stdout, while ret.stderr will be empty.
    CAPTURE_COMBINED_KWARGS = {
        "stdout": subprocess.PIPE,
        "stderr": subprocess.STDOUT,
    }

    # Capture neither stdout nor stderr. Both ret.stdout and ret.stderr will be empty.
    CAPTURE_NONE_KWARGS = {
        "stdout": None,
        "stderr": None,
    }

    def __init__(
        self,
        log_cmd: None | LogFunc = None,
        log_result: None | T.Callable[[CommandResult], None] = None,
    ) -> None:
        """Create instance.

        Args:
            log_cmd: Optional LogFunc to record each invocation.
            log_result: Optional callable used to log invocation results.
        """
        self._log_cmd = log_cmd
        self._log_result = log_result

    def run_command_internal(
        self, cmd_args: Sequence[FilePath], **subprocess_run_kwargs: T.Any
    ) -> CommandResult:
        """Internal implementation for run_command().

        Mock implementations can override this to avoid calling
        external commands during unit-tests.
        """
        # Enforce text mode by default.
        subprocess_run_kwargs.setdefault("text", True)
        subprocess_run_kwargs.setdefault("encoding", "utf-8")
        ret = subprocess.run(
            [str(c) for c in cmd_args], **subprocess_run_kwargs
        )
        return CommandResult(ret.returncode, ret.stdout, ret.stderr, ret.args)

    def run_command(
        self,
        cmd_args: Sequence[FilePath],
        **subprocess_run_kwargs: T.Any,
    ) -> CommandResult:
        """Run a command.

        Note that this sets text=True and encoding="UTF-8" by default for
        convenience, though it can be overriden when calling the method.

        Args:
            cmd_args: Command as a list of string or Path items.
            **kwargs: Other arguments for subprocess.run().
        Returns:
            A CommandResult value.
        """
        if self._log_cmd:
            self._log_cmd(cmd_args_to_string(cmd_args))

        try:
            ret = self.run_command_internal(cmd_args, **subprocess_run_kwargs)
        except KeyboardInterrupt:
            # As a special case, handle Ctrl-C as command failure,
            # Using exit code 130 (i.e. 128 + SIGSGEV). This is consistent
            # with what Ninja and other tools to, and avoids printing
            # a huge Python stack trace when the user interrupted the
            # build manually.
            ret = CommandResult(
                130, "", "KeyboardInterrupt", [str(c) for c in cmd_args]
            )

        if self._log_result:
            self._log_result(ret)

        return ret


# A MockCommandFilter is a callable that takes a list of command arguments
# as input, and returns a CommandResult as output. It may also raise
# ValueError if the input command arguments are invalid / unexpected.
#
# Note that the input is a list of strings, while CommandRunner.run_command()
# takes a list of FilePath values.
MockCommandFilter: T.TypeAlias = T.Callable[[list[str]], CommandResult]


class MockCommandRunner(CommandRunner):
    """A mock CommandRunner that can be used in unit-tests.

    Usage is:
        1) Create instance.

        2) Before running the test that uses the MockCommandRunner instance,
           push one or more CommandResult with push_result(). At least one per
           expected call to CommandRunner.run_command().

        3) After the test completes, look at self.commands and self.results to
           see what commands and results were actually recorded.
    """

    def __init__(self) -> None:
        """Create instance."""

        self.commands: list[str] = []

        def _log_command(cmd: str) -> None:
            self.commands.append(cmd)

        self.results: list[CommandResult] = []

        def _log_result(result: CommandResult) -> None:
            self.results.append(result)

        super().__init__(log_cmd=_log_command, log_result=_log_result)
        self._result_queue: list[CommandResult] = []
        self._command_filter: MockCommandFilter | None = None

    def set_command_filter(self, command_filter: MockCommandFilter) -> None:
        """Set a command filter.

        The command filter is called automatically by run_command_internal()
        if there are no more results in the CommandResult FIFO (which were
        previously added with push_result()).

        Args:
            command_filter: A MockCommandFilter values.
        """
        self._command_filter = command_filter

    @staticmethod
    def new_command_filter_from_list(
        cmd_list: list[tuple[str, str]],
    ) -> MockCommandFilter:
        """Create a command filter value that will return the given results in order.

        Args:
            cmd_list: A list of (command, stdout) pairs, where |command| is the
                space-separated command string, and |stdout| is the corresponding
                stdout.
        Returns:
            A new MockCommandFilter value.
        """
        command_index = 0

        def _command_filter(cmd_args: list[str]) -> CommandResult:
            nonlocal command_index
            if command_index >= len(cmd_list):
                raise ValueError("Too many command invocations")
            actual_args = " ".join(arg for arg in cmd_args)
            expected_cmd_args, stdout = cmd_list[command_index]
            if actual_args != expected_cmd_args:
                raise ValueError(
                    f"Unexpected command arguments (command index {command_index}:\n  Actual:   {actual_args}\n  Expected: {expected_cmd_args}"
                )
            command_index += 1
            return CommandResult(0, stdout, "")

        return _command_filter

    def push_result(
        self,
        returncode: int = 0,
        stdout: str = "",
        stderr: str = "",
    ) -> None:
        """Add one result value to the CommandResult FIFO.

        Args:
            returncode: Optional process return code. default to 0.
            stdout: Optional process stdout, as a string, default to empty.
            stderr: Optional process stderr, as a string, default to empty.
            args: Optional list of command arguments.
        """
        self._result_queue.append(
            CommandResult(
                returncode=returncode,
                stdout=stdout,
                stderr=stderr,
            )
        )

    def _pop_result(self) -> CommandResult:
        """Pop one result value from the CommandResult FIFO."""
        assert (
            self._result_queue
        ), f"Result queue is empty, did you forget to call MockCommandRunner.push_result()"
        result = self._result_queue[0]
        self._result_queue = self._result_queue[1:]
        return result

    def run_command_internal(
        self, cmd_args: Sequence[FilePath], **subprocess_run_kwargs: T.Any
    ) -> CommandResult:
        """Simulate command invocation by popping one value from the FIFO.

        Args:
            cmd_args: Command arguments, these are simply saved into
                self.commands for later inspection.
            **subprocess_run_kwargs: Extra subprocess.run() arguments (ignored).
        Returns:
            The CommandResult at the start of the FIFO.
        Raises:
            AssertionError if there are no results in the FIFO.
        """
        result_args = [str(c) for c in cmd_args]
        if self._command_filter:
            result = self._command_filter(result_args)
        else:
            result = self._pop_result()
        result.args = result_args
        return result


class NinjaRunner(object):
    """Wrapper class to invoke Ninja.

    Usage is:
       1) Create instance

       2) Call run_and_extract_output() to launch a command and
          get its output.

       3) Alternatively, use generate_subprocess_run_command() to
          generate the full command to use for a subprocess.run() call.
    """

    def __init__(
        self,
        ninja: Path,
        build_dir: Path,
        command_runner: CommandRunner | None = None,
    ):
        """Create instance.

        Args:
            ninja: Path to Ninja binary.
            build_dir: Path to Ninja build directory.
            command_runner: Optional CommandRunner instance. If None, a default instance will be created.
        """
        self._ninja = ninja.resolve()
        self._build_dir = build_dir.resolve()
        self._cmd_runner = command_runner or CommandRunner()

    @property
    def ninja(self) -> Path:
        """Absolute Path to Ninja binary."""
        return self._ninja

    @property
    def build_dir(self) -> Path:
        """Absolute path to Ninja build directory."""
        return self._build_dir

    def generate_subprocess_run_command(self, cmd: list[str]) -> list[str]:
        """Generate full command for invocation.

        This is useful to call subprocess.run() directly, for example to
        handle stdout / stderr differently.

        Args:
            cmd: list of Ninja options and command flags, without
               "-C <build_dir> which will be prepended to the result.
        Returns:
            A new list of command-line arguments to pass to subprocess.run()
        """
        return [str(self.ninja), "-C", str(self.build_dir)] + cmd

    def run_and_extract_output(self, cmd: list[str]) -> str:
        """Run a given Ninja command and return its output.

        Args:
            cmd: list of Ninja options, note that "-C <build_dir>" will be
               prepended to it before invoking Ninja.

        Returns:
           The command's stdout in case of success. stderr is captured but never returned
           unless the command fails (in which case it will be available from the corresponding
           exception object).

        Raises:
           subprocess.CalledProcessError if the command failed.
        """
        ret = self._cmd_runner.run_command(
            self.generate_subprocess_run_command(cmd),
            **self._cmd_runner.CAPTURE_KWARGS,
            check=True,
        )
        return ret.stdout


class MockNinjaRunner(NinjaRunner):
    """A mock NinjaRunner instance that can be used in tests."""

    def __init__(self, build_dir: Path, mock_output: str) -> None:
        self._mock_runner = MockCommandRunner()
        super().__init__(Path("/mock-ninja"), build_dir, self._mock_runner)
        self._mock_runner.push_result(0, mock_output, "")

    def last_ninja_args(self) -> list[str]:
        last_args = self._mock_runner.results[-1].args
        assert last_args[0:3] == ["/mock-ninja", "-C", str(self.build_dir)]
        return [str(c) for c in last_args[3:]]


class BazelLauncher(object):
    """Convenience class to launch Bazel invocations.

    A small wrapper around subprocess.run(), which allows tests to override
    the run_command() method in derived classes.
    """

    # Forward these for caller's convenience.
    CAPTURE_KWARGS = CommandRunner.CAPTURE_KWARGS
    CAPTURE_COMBINED_KWARGS = CommandRunner.CAPTURE_COMBINED_KWARGS
    CAPTURE_NONE_KWARGS = CommandRunner.CAPTURE_NONE_KWARGS

    def __init__(
        self,
        launcher_script: FilePath,
        bazel_prefix_args: Sequence[FilePath] = [],
        runner: None | CommandRunner = None,
        log_err: None | LogFunc = None,
    ) -> None:
        """Create BazelLauncher instance.

        Args:
            launcher_script: Path to the bazel launcher script.
            bazel_prefix_args: Optional list of command arguments that may appear before the
                launcher script during invocation.
            runner: An optional CommandRunner instance. If None, one will be created automatically.
            log_err: Optional LogFunc function used to record error messages in run_xxx() method calls.
        """
        self._launcher_script = launcher_script
        self._prefix_args = bazel_prefix_args
        self._runner = runner if runner else CommandRunner()
        self._log_err = log_err

    @property
    def script(self) -> Path:
        """Return path to Bazel launcher script."""
        return Path(self._launcher_script)

    def run_bazel_command(
        self,
        bazel_args: T.Iterable[FilePath],
        **subprocess_kwargs: T.Any,
    ) -> CommandResult:
        """Run a Bazel command.

        Note that this captures both standard and error outputs by default.
        Set stdout and stderr explicitly to change this behavior.

        Args:
            bazel_args: Bazel command-line arguments.

            **subprocess_kwargs: Extra arguments passed to subprocess.run(), see
                CommandRunner.run_command() documentation.
        Returns:
            A CommandResult value.
        """
        # Capture outputs by default.
        subprocess_kwargs.setdefault("stdout", subprocess.PIPE)
        subprocess_kwargs.setdefault("stderr", subprocess.PIPE)

        cmd_args: Sequence[FilePath] = [
            *self._prefix_args,
            self.script,
            *bazel_args,
        ]

        ret = self._runner.run_command(cmd_args, **subprocess_kwargs)

        # Log `stdout` and `stderr` on failure when `_log_err` is defined.
        # Note that `stderr` is only printed when there is an error since it may
        # contain progress or other status messages we don't want to log in
        # normal circumstances.
        if ret.returncode != 0 and self._log_err:
            self._log_err(
                "Error when invoking command: %s\n%s\n%s\n"
                % (cmd_args_to_string(cmd_args), ret.stderr, ret.stdout)
            )
        elif (
            os.environ.get("FUCHSIA_BAZEL_PRINT_COMMANDS") == "1" and ret.stderr
        ):
            # `stderr` is not being logged, but printing Bazel commands is
            # enabled. Print only the Bazel command lines.
            # LINT.IfChange(bazel_command_prefix)
            _BAZEL_COMMAND_PREFIX = "[bazel-command]"
            # LINT.ThenChange(//build/bazel/wrapper.bazel.sh:bazel_command_prefix)
            for line in ret.stderr.splitlines():
                if line.startswith(_BAZEL_COMMAND_PREFIX):
                    print(line, file=sys.stderr)

        return ret

    def run_query(
        self, query_type: str, query_args: list[str], ignore_errors: bool
    ) -> CommandResult:
        """Run a Bazel query, potentially ignoring errors.

        Args:
            query_type: Type of query ("query", "cquery" or "aquery").
            query_args: Other query arguments.
            ignore_errors: Set to True to allow queries that ignore errors.
               This adds "--keep_going" to the launched command.
        Returns:
            A CommandResult value.
        """
        query_cmd: list[FilePath] = []
        query_cmd.append(query_type)
        query_cmd.extend(query_args)
        stderr = subprocess.PIPE
        if ignore_errors:
            query_cmd += ["--keep_going"]
            stderr = subprocess.DEVNULL

        return self.run_bazel_command(query_cmd, stderr=stderr)


class MockBazelLauncher(BazelLauncher):
    """A mock BazelLauncher instance that can be used in tests."""

    def __init__(self) -> None:
        """Create new instance."""
        self.command_runner = MockCommandRunner()
        super().__init__(launcher_script="bazel", runner=self.command_runner)

    def push_expected_outputs(self, outputs: list[str]) -> None:
        """Push a sequence of expected command outputs to the mock  command runner."""
        for output in outputs:
            self.command_runner.push_result(0, output, "")

    @staticmethod
    def new_with_empty_outputs() -> "MockBazelLauncher":
        """Create a new MockBazelLauncher instance with empty outputs by default."""
        launcher = MockBazelLauncher()
        launcher.command_runner.set_command_filter(
            lambda args: CommandResult(0, "", "")
        )
        return launcher


class BazelQueryCache(object):
    def __init__(
        self,
        cache_dir: FilePath,
    ) -> None:
        self._cache_dir = cache_dir

    def get_query_output(
        self,
        query_type: str,
        query_args: list[str],
        launcher: BazelLauncher,
        log: None | LogFunc = None,
    ) -> list[str] | None:
        """Run a bazel query and return its output as a series of lines.

        Args:
            query_type: One of 'query', 'cquery' or 'aquery'
            query_args: Extra query arguments.
            launcher: A BazelLauncher instance.
            log: Optional LogFunc value. If not provided, uses launcher.log.

        Returns:
            On success, a list of output lines. On failure return None.
        """
        # The result of queries does not change between incremental builds,
        # as their outputs only depend on the shape of the Bazel graph, not
        # the content of the artifacts. Due to this, it is possible to cache
        # the outputs to save several seconds per bazel_action() invocation.
        #
        # The data is simply stored under $WORKSPACE/fuchsia_build_generated/bazel_query_cache/
        # which will be removed by each regenerator call, since it may change the Bazel
        # graph dependencies, and thus the query results.

        cache_key, cache_key_args = self.compute_cache_key_and_args(
            query_type, query_args
        )
        cache_file = os.path.join(self._cache_dir, f"{cache_key}.json")
        if os.path.exists(cache_file):
            try:
                with open(cache_file, "rt") as f:
                    cache_value = json.load(f)
                assert cache_value["key_args"] == cache_key_args
                if log:
                    log(
                        f"Found cached values for query {cache_key}: {cache_key_args}"
                    )
                return cache_value["output_lines"]
            except Exception as e:
                print(
                    f"WARNING: Error when reading cached values for query {cache_key}: {cache_key_args}:\n{e}",
                    file=sys.stderr,
                )

        if log:
            query_start_time = time.time()

        ret = launcher.run_query(query_type, query_args, ignore_errors=False)
        # Log any query errors (which may have a return code of 0), if logging enabled.
        if log and ret.stderr:
            log(f"Query error:\n{ret.stderr}")
        if ret.returncode != 0:
            return None

        result = ret.stdout.splitlines()

        # Write the result to the cache.
        new_cache_value = {
            "key_args": cache_key_args,
            "output_lines": result,
        }
        if log:
            log(
                "Query took %.1f seconds for query %s"
                % (
                    time.time()
                    - query_start_time,  # pyright: ignore[reportPossiblyUnboundVariable]
                    cache_key_args,
                )
            )
            log(f"Writing query values to cache for query {cache_key}\n")

        os.makedirs(os.path.dirname(cache_file), exist_ok=True)
        with open(cache_file, "wt") as f:
            json.dump(new_cache_value, f)

        return result

    @staticmethod
    def compute_cache_key_and_args(
        query_type: str, query_args: list[str]
    ) -> tuple[str, list[str]]:
        """Compute the cache key and arguments. Exposed for tests.

        Args:
            query_type: query type (e.g. "query", "cquery" or "aquery")
            query_args: query arguments.
        Returns:
            a (cache_key, cache_key_args), where cache_key is a unique
            hexadecimal cache key value, and cache_key_args encode the
            input query in a human readable way. This will be stored in
            the cache value for debugging.
        """
        cache_key_args = [query_type] + query_args
        cache_key_inputs = cache_key_args[:]

        # If "--starlark:file=FILE" or "--starlark:file FILE" is used, add the
        # content of FILE to compute the cache key. This ensure stale cache
        # entries are not reused when only this file changes during development.
        starlark_file_option = "--starlark:file"
        for n, arg in enumerate(query_args):
            input_path = None
            if arg == starlark_file_option and n + 1 < len(query_args):
                # For --starlark:file FILE
                input_path = query_args[n + 1]
            elif arg.startswith(f"{starlark_file_option}="):
                # For --starlark:file=FILE
                input_path = arg[len(starlark_file_option) + 1 :]
            if input_path:
                with open(input_path, "rt") as f:
                    cache_key_inputs.append(f.read())

        cache_key = hashlib.sha256(
            repr(cache_key_inputs).encode("utf-8")
        ).hexdigest()

        return (cache_key, cache_key_args)
