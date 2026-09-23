#!/usr/bin/env python3

# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Run the Fuchsia Bazel SDK test suite.

This script can be used to run the test suite against three different
types of IDK or SDK directories:

- Against an export IDK:

  Use `--fuchsia_idk_directory=DIR` to specify its path. This will automatically
  create a temporary @fuchsia_sdk repository before running the test suite.

- Against an SDK directory:

  Use `--fuchsia_sdk_directory=DIR` to specify its path. For now, this assumes
  that the SDK directory is self-contained (i.e. does not depend on @rules_fuchsia).

- Against the in-tree @fuchsia_sdk repository:

  Use the `--fuchsia-in-tree-sdk` flag. This should auto-detect the location of
  the in-tree @fuchsia_sdk and @fuchsia_in_tree_idk repositories.

In all cases, the default --target_cpu value will be guessed by looking at the
Fuchsia build directory.

See //build/bazel/bazel_sdk/README.md for more details about these three
categories.
"""

import argparse
import errno
import json
import os
import platform
import shlex
import subprocess
import sys
import typing as T
from pathlib import Path

_HAS_FX = None

_VERBOSE = False

# The three different types of inputs supported by this script:
# IN_TREE: Used to test the in-tree @fuchsia_sdk and @fuchsia_in_tree_idk repos.
# SDK: Used to test a standalone (OOT) Fuchsia SDK directory.
# IDK: Used to test a standalone Fuchsia IDK directory.
_INPUT_MODE_IN_TREE = "in-tree"
_INPUT_MODE_SDK = "sdk"
_INPUT_MODE_IDK = "idk"

_CANONICAL_FUCHSIA_SDK_REPO_NAME = "rules_fuchsia++fuchsia_sdk_ext+fuchsia_sdk"
_CANONICAL_IN_TREE_IDK_REPO_NAME = "fuchsia_in_tree_idk+"
_CANONICAL_RULES_FUCHSIA_REPO_NAME = "rules_fuchsia+"

# LINT.IfChange(bazel_vendor_dir)
_DEFAULT_BAZEL_VENDOR_DIR = "third_party/bazel_vendor"
# LINT.ThenChange(//build/bazel/enable_vendor_mode.bazelrc:bazel_vendor_dir)

# LINT.IfChange(bazel_registry_dir)
_DEFAULT_BAZEL_REGISTRY_DIR = "third_party/bazel_registries/bcr.bazel.build"
# LINT.ThenChange(//build/bazel/enable_vendor_mode.bazelrc:bazel_registry_dir)

# Maps from apparent repo names to canonical repo names.
_APPARENT_REPO_NAME_TO_CANONICAL = {
    "fuchsia_sdk": _CANONICAL_FUCHSIA_SDK_REPO_NAME,
    "fuchsia_in_tree_idk": _CANONICAL_IN_TREE_IDK_REPO_NAME,
}

# Type alias for string or Path type.
# NOTE: With python 3.10+, it is possible to use 'str | Path' directly.
StrOrPath = T.Union[str, Path]


def _force_symlink(target_path: Path, link_path: Path) -> None:
    link_path.parent.mkdir(parents=True, exist_ok=True)
    target_path = Path(os.path.relpath(target_path, link_path.parent))
    try:
        link_path.symlink_to(target_path)
    except OSError as e:
        if e.errno == errno.EEXIST:
            link_path.unlink()
            link_path.symlink_to(target_path)
        else:
            raise


def _generate_command_string(
    args: T.Sequence[StrOrPath], **kwargs: T.Any
) -> str:
    """Generate a string that prints a command to be run.

    Args:
      args: a list of string or Path items corresponding to the command.
      **kwargs: extra subprocess.run() extra arguments.

    Returns:
      A string that can be printed to a terminal showing the command to
      run.
    """
    output = ""
    margin = ""
    wrap_command = False
    cwd = kwargs.get("cwd")
    if cwd:
        margin = "  "
        output = f"(\n{margin}cd {cwd} &&\n"
        wrap_command = True

    env = kwargs.get("env")
    if env:
        for key, value in sorted(env.items()):
            if os.environ.get(key, None) != value:
                output += "%s%s=%s \\\n" % (margin, key, shlex.quote(value))

    for a in args:
        output += "%s%s \\\n" % (margin, shlex.quote(str(a)))

    if wrap_command:
        output += ")\n"

    return output


def _run_command(
    cmd_args: T.Sequence[StrOrPath],
    check_failure: bool = True,
    **kwargs: T.Any,
) -> "subprocess.CompletedProcess[str]":
    """Run a given command.

    Args:
      cmd_args: a list of string or Path items corresponding to the command.
      check_failure: set to False to ignore command failures. Otherwise the default
        if to print the command's stderr, then raising an exception.
      **kwargs: extra subprocess.run() named arguments.

    Returns:
      a subprocess.CompletedProcess value.
    """
    args = [str(a) for a in cmd_args]
    if _VERBOSE:
        print("RUN_COMMAND: %s" % _generate_command_string(args, **kwargs))

    ret = subprocess.run(args, **kwargs)

    if ret.returncode != 0 and check_failure:
        print(
            "FAILED COMMAND: %s" % _generate_command_string(args, **kwargs),
            file=sys.stderr,
        )
        if ret.stderr:
            print("ERROR: %s" % ret.stderr, file=sys.stderr)
        ret.check_returncode()

    return ret


def _get_command_output_lines(
    args: T.Sequence[StrOrPath],
    extra_env: dict[str, str] | None = None,
    **kwargs: T.Any,
) -> T.Sequence[str]:
    """Run a given command, then return its standard output as text lines.

    Args:
        args: a list of string or Path items corresponding to the command.
        extra_env: a dictionary of optional extra environment variable definitions.
        **kwargs: extra subprocess.run() named arguments.
    Returns:
        A sequence of strings, each one corresponding to one line of the output
        (line terminators are not included).
    """
    if extra_env:
        env = kwargs.get("env")
        if env is None:
            env = os.environ.copy()
        kwargs["env"] = env | extra_env

    ret = _run_command(
        args, capture_output=True, text=True, check_failure=True, **kwargs
    )
    return ret.stdout.splitlines()


def _print_error(msg: str) -> int:
    """Print error message to stderr then return 1."""
    print("ERROR: " + msg, file=sys.stderr)
    return 1


def _find_fuchsia_source_dir_from(path: Path) -> Path | None:
    """Try to find the Fuchsia source directory from a starting location.

    Args:
      path: Path to a file or directory in the Fuchsia source tree.

    Returns:
      Path value for the Fuchsia source directory, or None if not found.
    """
    if path.is_file():
        path = path.parent

    path = path.resolve()
    while True:
        if str(path) == "/":
            return None
        if (path / ".jiri_manifest").exists():
            return path
        path = path.parent


def _find_fuchsia_build_dir(fuchsia_source_dir: Path) -> Path | None:
    """Find the current Fuchsia build directory.

    Args:
      fuchsia_source_dir: Path value for the Fuchsia source directory.

    Returns:
      Path value for the current build directory selected with `fx` or None.
    """
    fx_build_dir = fuchsia_source_dir / ".fx-build-dir"
    if not fx_build_dir.exists():
        return None

    with open(fx_build_dir) as f:
        return fuchsia_source_dir / f.read().strip()


def _relative_path(path: Path) -> Path:
    return Path(os.path.relpath(path))


def _depfile_quote(path: str) -> str:
    """Quote a path properly for depfiles, if necessary.

    shlex.quote() does not work because paths with spaces
    are simply encased in single-quotes, while the Ninja
    depfile parser only supports escaping single chars
    (e.g. ' ' -> '\ ').

    Args:
       path: input file path.
    Returns:
       The input file path with proper quoting to be included
       directly in a depfile.
    """
    return path.replace("\\", "\\\\").replace(" ", "\\ ")


def _flatten_comma_list(items: T.Iterable[str]) -> T.Iterable[str]:
    """Flatten ["a,b", "c,d"] -> ["a", "b", "c", "d"].

    This is useful for merging repeatable flags, which also
    have comma-separated values.

    Yields:
      Elements that were separated by commas, flattened over
      the original sequence..
    """
    for item in items:
        yield from item.split(",")


def build_metadata_flags(siblings_link_template: str) -> T.Sequence[str]:
    """Convert environment variables into build metadata flags."""
    result_flags = []

    # Propagate some build metadata from the environment.
    # Some of these values are set by infra.
    def forward_build_metadata_from_env(var: str) -> str | None:
        env_value = os.environ.get(var)  # set by infra
        if env_value is None:
            return None

        result_flags.append(f"--build_metadata={var}={env_value}")
        return env_value

    bb_id = forward_build_metadata_from_env("BUILDBUCKET_ID")

    # Provide click-able/paste-able link for convenience.
    if bb_id:
        result_flags.append(
            f"--build_metadata=SIBLING_BUILDS_LINK={siblings_link_template}?q=BUILDBUCKET_ID:{bb_id}"
        )
        if "/led/" in bb_id:
            result_flags.append(
                f"--build_metadata=PARENT_BUILD_LINK=go/lucibuild/{bb_id}/+/build.proto"
            )
        else:
            result_flags.append(
                f"--build_metadata=PARENT_BUILD_LINK=go/bbid/{bb_id}"
            )

    forward_build_metadata_from_env("BUILDBUCKET_BUILDER")

    # Developers' builds will have one uuid per `fx build` invocation
    # that can be used to correlate multiple bazel sub-builds.
    fx_build_id = forward_build_metadata_from_env("FX_BUILD_UUID")

    if fx_build_id:
        result_flags.append(
            f"--build_metadata=SIBLING_BUILDS_LINK={siblings_link_template}?q=FX_BUILD_UUID:{fx_build_id}"
        )

    return result_flags


class BazelRepositoryMap(object):
    IGNORED_REPO = Path("IGNORED")

    def __init__(
        self,
        fuchsia_source_dir: Path,
        rules_fuchsia_dir: Path,
        bazel_vendor_dir: Path,
        explicit_fuchsia_sdk: Path | None,
        explicit_fuchsia_in_tree_idk: Path | None,
        workspace_dir: Path,
        output_base: Path,
    ):
        self._fuchsia_source_dir = fuchsia_source_dir
        self._workspace_dir = workspace_dir
        self._output_base = output_base

        # These repository overrides are passed to the Bazel invocation.
        self._overrides: dict[str, Path] = {}

        if explicit_fuchsia_sdk:
            self._overrides[
                _CANONICAL_FUCHSIA_SDK_REPO_NAME
            ] = explicit_fuchsia_sdk.resolve()
        if explicit_fuchsia_in_tree_idk:
            self._overrides[
                _CANONICAL_IN_TREE_IDK_REPO_NAME
            ] = explicit_fuchsia_in_tree_idk.resolve()

        self._overrides[_CANONICAL_RULES_FUCHSIA_REPO_NAME] = rules_fuchsia_dir

        # These repository overrides are used when converting Bazel labels to actual paths.
        # NOTE: Mapping labels to repository inputs is considerably simpler than
        # //build/bazel/scripts/bazel_gn_target_action.py because there are way less edge
        # cases to consider and because ignoring @prebuilt_python and
        # @fuchsia_clang entirely is safe, since these are already populated
        # by the GN build_fuchsia_sdk_repository target which is an input
        # dependency for running this script.
        self._internal_overrides = self._overrides | {
            "rules_cc": bazel_vendor_dir / "rules_cc+",
            "rules_license": bazel_vendor_dir / "rules_license+",
            "bazel_tools+remote_coverage_tools_extension+remote_coverage_tools": bazel_vendor_dir
            / "bazel_tools+remote_coverage_tools_extension+remote_coverage_tools",
            "bazel_skylib": bazel_vendor_dir / "bazel_skylib+",
            "bazel_features+": bazel_vendor_dir / "bazel_features+",
            "rules_python": bazel_vendor_dir / "rules_python+",
            "platforms": bazel_vendor_dir / "platforms",
            "com_google_googletest": fuchsia_source_dir
            / "third_party/googletest/src",
            "com_google_protobuf": fuchsia_source_dir / "third_party/protobuf",
            "rules_fuchsia": rules_fuchsia_dir,
            "zlib": fuchsia_source_dir / "third_party/zlib",
            "prebuilt_python": self.IGNORED_REPO,
            "fuchsia_clang": self.IGNORED_REPO,
            "bazel_features++version_extension+bazel_features_globals": self.IGNORED_REPO,
            "bazel_features++version_extension+bazel_features_version": self.IGNORED_REPO,
            "bazel_tools": self.IGNORED_REPO,
            "bazel_tools+cc_configure_extension+local_config_cc": self.IGNORED_REPO,
            "platforms+host_platform+host_platform": self.IGNORED_REPO,
            "rules_cc++compatibility_proxy+cc_compatibility_proxy": self.IGNORED_REPO,
            "rules_python++internal_deps+rules_python_internal": self.IGNORED_REPO,
            "rules_python++config+rules_python_internal": self.IGNORED_REPO,
            "rules_python++python+pythons_hub": self.IGNORED_REPO,
            "rules_shell+": self.IGNORED_REPO,
            "package_metadata+": self.IGNORED_REPO,
        }

        if not explicit_fuchsia_sdk:
            self._internal_overrides[_CANONICAL_FUCHSIA_SDK_REPO_NAME] = (
                self._fuchsia_source_dir / "build/bazel_sdk/bazel_rules_fuchsia"
            )

    def add_override(self, name: str, path: Path) -> None:
        assert (
            name not in self._overrides
        ), f"Override is already registered for {name}: {self._overrides[name]} vs {path}"
        self._overrides[name] = path
        self._internal_overrides[name] = path

    def get_repository_overrides_flags(self) -> T.Sequence[str]:
        """Return a sequence command-line flags for overriding Bazel external repositories."""
        return [
            f"--override_repository={name}={path}"
            for name, path in self._overrides.items()
        ]

    def resolve_bazel_path(self, bazel_path: str) -> Path | None:
        """Convert a Bazel path label to a real Path or None if it should be ignored."""
        if bazel_path.startswith("//"):
            target_path = bazel_path[2:]
            repo_dir = self._workspace_dir
            repo_name = ""
        elif bazel_path.startswith("@"):
            repo_name, sep, target_path = bazel_path.partition("//")
            assert (
                sep
            ), f"Build file path has invalid repository root: {bazel_path}"
            repo_name = repo_name.removeprefix("@@").removeprefix("@")
            repo_name = _APPARENT_REPO_NAME_TO_CANONICAL.get(
                repo_name, repo_name
            )
            _repo_dir = self._internal_overrides.get(repo_name, None)
            assert _repo_dir, (
                f"Unknown repository name {repo_name} in build file path: {bazel_path}\n"
                + f"Please modify {__file__} to handle it!"
            )
            repo_dir = _repo_dir
            if repo_dir == self.IGNORED_REPO:
                return None
        else:
            assert False, f"Invalid build file path: {bazel_path}"

        package_dir, colon, target_name = target_path.partition(":")
        if colon == ":":
            if package_dir:
                target_path = f"{package_dir}/{target_name}"
            else:
                target_path = target_name
        else:
            target_path = package_dir + "/" + os.path.basename(package_dir)

        final_path = repo_dir / target_path
        if final_path.exists():
            return final_path

        # Sometimes the path will point to a non-existent file, for example
        #
        #   @fuchsia_sdk//:api_version.bzl
        #     corresponds to a file generated by the repository rule that
        #     generates the @fuchsia_sdk repository.
        #
        #   //build/bazel_sdk/bazel_rules_fuchsia/api_version.bzl does
        #     not exist.
        #
        #   $OUTPUT_BASE/external/fuchsia/api_version.bzl is the actual
        #     location of that file.
        #
        if repo_name:
            external_repo_dir = self._output_base / "external" / repo_name
            final_path = external_repo_dir / target_path
            assert (
                final_path.exists()
            ), f"When resolving {bazel_path}: File '{final_path}' does not exist."
            return final_path.resolve()

        # This should not happen, but print an error message pointing to this
        # script in case it really does!
        assert (
            False
        ), f"Unknown input label, please update {__file__} to handle it: {bazel_path}"


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--bazel", help="Specify bazel binary.")
    mutex_group = parser.add_mutually_exclusive_group(required=True)
    mutex_group.add_argument(
        "--fuchsia_sdk_directory",
        type=Path,
        help="Specify Fuchsia SDK directory.",
    )
    mutex_group.add_argument(
        "--fuchsia_idk_directory",
        type=Path,
        help="Specify Fuchsia IDK directory.",
    )
    mutex_group.add_argument(
        "--fuchsia-in-tree-sdk",
        action="store_true",
        help="Run against the in-tree @fuchsia_sdk and @fuchsia_in_tree_idk repositories.",
    )
    parser.add_argument(
        "--fuchsia_source_dir",
        type=Path,
        help="Specify Fuchsia source directory (default is auto-detected).",
    )
    parser.add_argument(
        "--fuchsia_build_dir",
        type=Path,
        help="Specify Fuchsia build directory (default is auto-detected).",
    )
    parser.add_argument(
        "--rules_fuchsia_dir",
        type=Path,
        help="Specify @rules_fuchsia directory (default is auto-detected).",
    )
    parser.add_argument(
        "--in_tree_fuchsia_sdk",
        type=Path,
        help="Specify alternative in-tree @fuchsia_sdk source repository, when using --fuchsia-in-tree-sdk",
    )
    parser.add_argument(
        "--in_tree_fuchsia_idk",
        type=Path,
        help="Specify alternative path to in-tree @fuchsia_in_tree_idk when using --fuchsia-in-tree-sdk",
    )
    parser.add_argument(
        "--output_base",
        type=Path,
        help="Use specific Bazel output base directory.",
    )
    parser.add_argument(
        "--output_user_root",
        type=Path,
        help="Use specific Bazel output user root directory.",
    )
    parser.add_argument(
        "--prebuilt-python-version-file",
        type=Path,
        help="Optional path to version file for prebuilt python toolchain.",
    )
    parser.add_argument(
        "--prebuilt-clang-version-file",
        type=Path,
        help="Optional path to version file for prebuilt Clang toolchain.",
    )
    parser.add_argument(
        "--stamp-file",
        type=Path,
        help="Output stamp file, written on success only.",
    )
    parser.add_argument(
        "--depfile", type=Path, help="Output Ninja depfile file."
    )
    parser.add_argument(
        "--target_cpu",
        help="Target cpu name, using Fuchsia conventions (default is auto-detected).",
    )
    parser.add_argument(
        "--verbose", action="store_true", help="Enable verbose mode."
    )
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="Do not print anything unless there is an error.",
    )
    parser.add_argument(
        "--clean", action="store_true", help="Force clean build."
    )
    parser.add_argument(
        "--command",
        default="test",
        help="Override bazel command, default is 'test'. Use -- to pass extra arguments.",
    )
    parser.add_argument(
        "--bazel-vendor-dir",
        type=Path,
        help=f"Specify Bazel vendor directory to use or write to. Defaults to $FUCHSIA/{_DEFAULT_BAZEL_VENDOR_DIR}.",
    )
    parser.add_argument(
        "--bazel-registry-dir",
        type=Path,
        help=f"Specify Bazel registry directory to use. Ignored in vendor mode. Defaults to $FUCHSIA/{_DEFAULT_BAZEL_REGISTRY_DIR}.",
    )
    parser.add_argument(
        "--test_target",
        default="//:tests",
        help="Which target to invoke with `bazel test` (default is '//:tests')",
    )
    parser.add_argument(
        "--test_output",
        default="",
        help="See `bazel test --help` for the `test_output` argument.",
    )
    parser.add_argument(
        "--bazelrc",
        help="Additional Bazel configuration file to load",
        type=Path,
        default=[],
        metavar="FILE",
        action="append",
    )
    parser.add_argument(
        "--bazel-config",
        help="Additional Bazel --config options, comma-separated, repeatable",
        default=[],
        metavar="CFG",
        action="append",
    )
    parser.add_argument(
        "--bazel-build-events-log-json",
        type=Path,
        help="Output path to JSON-formatted Build Event Protocol log file",
        metavar="LOG",
    )
    parser.add_argument(
        "--bazel-exec-log-compact",
        type=Path,
        help="Output path to zstd-compressed action execution log file (protobuf: spawn.proto)",
        metavar="LOG",
    )
    parser.add_argument("extra_args", nargs=argparse.REMAINDER)

    args = parser.parse_args()

    if args.quiet:
        args.verbose = None

    if args.verbose:
        global _VERBOSE
        _VERBOSE = True

    if args.depfile and not args.stamp_file:
        parser.error(
            "The --depfile option requires a --stamp-file output path!"
        )

    extra_args = []
    if args.extra_args:
        if args.extra_args[0] != "--":
            parser.error(
                'Use "--" to separate  extra arguments passed to the bazel test command.'
            )
        extra_args = args.extra_args[1:]

    # Get Fuchsia source directory.
    if args.fuchsia_source_dir:
        fuchsia_source_dir = args.fuchsia_source_dir
    else:
        _fuchsia_source_dir = _find_fuchsia_source_dir_from(Path(__file__))
        if not _fuchsia_source_dir:
            return _print_error(
                "Cannot find Fuchsia source directory, please use --fuchsia_source_dir=DIR"
            )
        fuchsia_source_dir = _fuchsia_source_dir

    if not fuchsia_source_dir.exists():
        return _print_error(
            f"Fuchsia source directory does not exist: {fuchsia_source_dir}"
        )

    # fuchsia_source_dir must be an absolute path or Bazel will complain
    # when it is used for --override_repository options below.
    fuchsia_source_dir = fuchsia_source_dir.resolve()

    # Get Fuchsia build directory, if possible. Provide a checking function too.
    fuchsia_build_dir = args.fuchsia_build_dir
    if fuchsia_build_dir is None:
        fuchsia_build_dir = _find_fuchsia_build_dir(fuchsia_source_dir)

    rules_fuchsia_dir = args.rules_fuchsia_dir
    if rules_fuchsia_dir is None:
        rules_fuchsia_dir = (
            fuchsia_source_dir / "build/bazel_sdk/bazel_rules_fuchsia"
        )

    rules_fuchsia_dir = rules_fuchsia_dir.resolve()

    def check_fuchsia_build_dir(
        print_error: T.Callable[[str], T.Any] | None = None,
    ) -> bool:
        """Check that fuchsia_build_dir is set and exists. Return True on success."""
        if not fuchsia_build_dir:
            if print_error:
                print_error(
                    "Cannot auto-detect Fuchsia build directory, use --fuchsia_build_dir=DIR"
                )
            return False
        if not fuchsia_build_dir.exists():
            if print_error:
                print_error(
                    f"Fuchsia build directory does not exist: {fuchsia_build_dir}"
                )
            return False
        return True

    # Determine the type of input used.
    # This will take one of the valid _INPUT_MODE_XXX values.
    input_mode = ""

    if args.fuchsia_in_tree_sdk:
        input_mode = _INPUT_MODE_IN_TREE
        if not check_fuchsia_build_dir(print_error=_print_error):
            return 1

    elif args.fuchsia_sdk_directory:
        input_mode = _INPUT_MODE_SDK
        if not args.fuchsia_sdk_directory.exists():
            return _print_error(
                f"Fuchsia SDK directory does not exist: {args.fuchsia_sdk_directory}"
            )

    elif args.fuchsia_idk_directory:
        input_mode = _INPUT_MODE_IDK
        if not args.fuchsia_idk_directory.exists():
            return _print_error(
                f"Fuchsia IDK directory does not exist: {args.fuchsia_idk_directory}"
            )
    else:
        assert False, f"Internal error: Invalid build mode!"
        _find_fuchsia_build_dir(fuchsia_source_dir)

    # Compute Fuchsia host tag
    u = platform.uname()
    host_os = {
        "Linux": "linux",
        "Darwin": "mac",
        "Windows": "win",
    }.get(u.system, u.system)

    host_cpu = {
        "x86_64": "x64",
        "AMD64": "x64",
        "aarch64": "arm64",
    }.get(u.machine, u.machine)

    host_tag = f"{host_os}-{host_cpu}"

    # Find Bazel binary
    if args.bazel:
        bazel = Path(args.bazel)
    else:
        bazel = (
            fuchsia_source_dir
            / "prebuilt"
            / "third_party"
            / "bazel"
            / host_tag
            / "bazel"
        )

    if not bazel.exists():
        return _print_error(f"Bazel binary does not exist: {bazel}")

    bazel = bazel.resolve()

    # The Bazel workspace assumes that the Fuchsia cpu is the host
    # CPU unless --cpu or --platforms is used. Extract the target_cpu
    # from ${fuchsia_build_dir}/args.json and construct the corresponding
    # bazel test argument.
    #
    # Note that there is a subtle issue here: for historical reasons, the default
    # --cpu value is `k8`, an old moniker for the x86_64 cpu architecture. This
    # impacts the location of build artifacts in the default/target build
    # configuration, which would go under bazel-out/k8-fastbuild/bin/...
    # then.
    #
    # However, our --config=fuchsia_x64 argument below changes --cpu to `x86_64`
    # which is also recognized properly, but changes the location of build
    # artifacts to bazel-out/x86_64-fastbuild/ instead, which is important when
    # these paths go into build artifacts (e.g. product bundle manifests) and
    # need to be compared to golden files by the test suite.
    #
    # In short, this test suite does not support invoking `bazel test` without
    # an appropriate `--config=fuchsia_<cpu>` argument.
    #
    if args.target_cpu:
        target_cpu = args.target_cpu
    else:
        target_cpu = ""
        if check_fuchsia_build_dir():
            assert fuchsia_build_dir
            args_json = fuchsia_build_dir / "args.json"
            if args_json.exists():
                with open(args_json) as f:
                    target_cpu = json.load(f)["target_cpu"]

        if not target_cpu:
            parser.error(
                "Cannot auto-detect --target_cpu, use --target_cpu=CPU"
            )

    # Assume this script is under '$WORKSPACE/scripts'
    script_dir = Path(__file__).parent.resolve()
    workspace_dir = script_dir.parent

    downloader_config_file = (
        fuchsia_source_dir / "build/bazel/config/no_downloads_allowed.config"
    )
    bazel_vendor_dir = (
        args.bazel_vendor_dir or fuchsia_source_dir / _DEFAULT_BAZEL_VENDOR_DIR
    ).resolve()

    bazel_registry_dir = (
        args.bazel_registry_dir
        or fuchsia_source_dir / _DEFAULT_BAZEL_REGISTRY_DIR
    ).resolve()

    # If there's a //vendor dir in the fuchsia source dir, we need to symlink to it
    # from the tests directory, so that any hlcpp visibility rules referencing
    # allowlisted vendor targets won't fail.
    vendor_dir = fuchsia_source_dir / "vendor"
    vendor_dir_link = workspace_dir / "vendor"
    if vendor_dir.exists():
        _force_symlink(vendor_dir, vendor_dir_link)
    else:
        # And if //vendor has been removed (ie, integration repo change),
        # then remove the link.
        if os.path.lexists(vendor_dir_link) and vendor_dir_link.is_symlink():
            vendor_dir_link.unlink()

    # To ensure that the repository rules for @fuchsia_clang and
    # @prebuilt_python are re-run properly when the content of the prebuilt
    # toolchain directory changes, use a version file that is symlinked into
    # the workspace, and whose path is passed through environment variables
    # LOCAL_FUCHSIA_CLANG_VERSION_FILE and LOCAL_PREBUILT_PYTHON_VERSION_FILE
    # respectively. The workspace symlinks are necessary to ensure that
    # Bazel will track changes to these files properly, as repository rules
    # cannot track changes to files outside the workspace :-(

    def setup_version_file(name: str, source_path: Path) -> str | None:
        if not source_path.exists():
            return None

        dst_path = f".versions/{name}"
        _force_symlink(source_path, workspace_dir / dst_path)
        return dst_path

    python_prebuilt_dir = (
        fuchsia_source_dir / f"prebuilt/third_party/python3/{host_tag}"
    )
    python_version_file = args.prebuilt_python_version_file
    if not python_version_file:
        python_version_file = (
            python_prebuilt_dir / ".versions/cpython3.cipd_version"
        )

    workspace_python_version_file = setup_version_file(
        "prebuilt_python", python_version_file
    )

    clang_version_file = args.prebuilt_clang_version_file
    if not clang_version_file:
        clang_version_file = (
            fuchsia_source_dir
            / f"prebuilt/third_party/clang/{host_tag}/.versions/clang.cipd_version"
        )

    workspace_clang_version_file = setup_version_file(
        "prebuilt_clang", clang_version_file
    )

    # These argument remove verbose output from Bazel, used in queries.
    bazel_quiet_args = [
        "--noshow_loading_progress",
        "--noshow_progress",
        "--ui_event_filters=-info",
    ]

    # The list of command-line options that appear between the 'bazel' binary path
    # and the bazel command (such as "test", "query", or "build").
    bazel_startup_args = [str(bazel)]

    # The list of command-line options that appear after the bazel command, and
    # are shared by all commands.
    bazel_common_args = []

    # The list of command-line options that are only used for commands that perform
    # analysis (e.g. "cquery", "aquery", "build", "run" and "test", but not "query")
    bazel_config_args = []

    # Command line arguments for the 'test' command only.
    bazel_test_args = []

    # These options must appear before the Bazel command
    bazel_startup_args += [
        # Disable parsing of $HOME/.bazelrc to avoid unwanted side-effects.
        "--nohome_rc",
    ]

    # bazelrc files are passed relative to the current working directory,
    # but need to be adjusted relative to the workspace dir.
    rc_relpath = os.path.relpath(os.curdir, start=workspace_dir)
    for rc in args.bazelrc:
        bazel_startup_args += [
            f"--bazelrc={rc_relpath}/{rc}" for rc in args.bazelrc
        ]

    if args.output_user_root:
        output_user_root = args.output_user_root.resolve()
        output_user_root.mkdir(parents=True, exist_ok=True)
        bazel_startup_args += [f"--output_user_root={output_user_root}"]

    if args.output_base:
        output_base = args.output_base.resolve()
        output_base.mkdir(parents=True, exist_ok=True)
        bazel_startup_args += [f"--output_base={output_base}"]
    else:
        # Get output base from Bazel directly.
        output_base = Path(
            _get_command_output_lines(
                bazel_startup_args + ["info", "output_base"], cwd=workspace_dir
            )[0]
        )

    explicit_fuchsia_sdk = None
    explicit_fuchsia_in_tree_idk = None
    if input_mode == _INPUT_MODE_IN_TREE:
        assert fuchsia_build_dir

        # The in-tree @fuchsia_sdk references labels in @fuchsia_in_tree_idk
        # so both need to be overridden.
        explicit_fuchsia_sdk = (
            fuchsia_build_dir
            / "gen"
            / "build"
            / "bazel"
            / "output_base"
            / "external"
            / _CANONICAL_FUCHSIA_SDK_REPO_NAME
        )
        explicit_fuchsia_in_tree_idk = (
            fuchsia_build_dir
            / "gen"
            / "build"
            / "bazel"
            / "output_base"
            / "external"
            / _CANONICAL_IN_TREE_IDK_REPO_NAME
        )
    elif input_mode == _INPUT_MODE_SDK:
        # Only override @fuchsia_sdk to point to the full SDK directory.
        explicit_fuchsia_sdk = args.fuchsia_sdk_directory

    bazel_repo_map = BazelRepositoryMap(
        fuchsia_source_dir=fuchsia_source_dir,
        rules_fuchsia_dir=rules_fuchsia_dir,
        bazel_vendor_dir=bazel_vendor_dir,
        explicit_fuchsia_sdk=explicit_fuchsia_sdk,
        explicit_fuchsia_in_tree_idk=explicit_fuchsia_in_tree_idk,
        workspace_dir=workspace_dir,
        output_base=output_base,
    )

    is_vendor_mode = args.command == "vendor"

    if is_vendor_mode:
        downloader_config_arg = "--downloader_config=/dev/null"
        registry_url = args.registry or "https://bcr.bazel.build/"
    else:
        downloader_config_arg = f"--downloader_config={downloader_config_file}"
        registry_url = f"file://{bazel_registry_dir}"

    bazel_common_args += [
        # Prevent all downloads through a downloader configuration file unless vendoring.
        downloader_config_arg,
        "--enable_bzlmod=true",
        "--incompatible_use_plus_in_repo_names",
        f"--vendor_dir={bazel_vendor_dir}",
        f"--registry={registry_url}",
    ]

    # Override repositories since all downloads are forbidden.
    # This allows the original WORKSPACE.bazel to work out-of-tree by
    # download repositories as usual, while running the test suite in-tree
    # will use the Fuchsia checkout's versions of these external dependencies.
    bazel_common_args += bazel_repo_map.get_repository_overrides_flags()

    # These options must appear for commands that act on the configure graph (i.e. all except `bazel query`
    bazel_config_args += [
        # Ensure binaries are generated for the right Fuchsia CPU architecture.
        # Without this, @fuchsia_sdk rules assume that target_cpu == host_cpu,
        # and will use an incorrect output path prefix (i.e.
        # bazel-out/k8-fastbuild/ instead of bazel-out/x86_64-fastbuild/ on
        # x64 hosts, leading to golden file comparison failures later.
        f"--config=fuchsia_{target_cpu}",
        # Ensure the embedded JDK that comes with Bazel is always used
        # This prevents Bazel from downloading extra host JDKs from the
        # network, even when a project never uses Java-related  rules
        # (for some still-very-mysterious reasons!)
        "--java_runtime_version=embedded_jdk",
        "--tool_java_runtime_version=embedded_jdk",
        # Ensure outputs are writable (umask 0755) instead of readonly (0555),
        # which prevent removing output directories with `rm -rf`.
        # See https://fxbug.dev/42072059
        "--experimental_writable_outputs",
    ]

    # Forward additional --config's, intended for `bazel test`.
    # These configs should not affect the build graph.
    bazel_config_args += [
        f"--config={cfg}" for cfg in _flatten_comma_list(args.bazel_config)
    ]
    bazel_config_args += [
        # In case of build or test errors, provide more details about the failed
        # command. See https://fxbug.dev/325346878
        "--verbose_failures",
    ]

    if args.quiet:
        bazel_test_args += [
            "--show_result=0",
            "--test_output=errors",
            "--test_summary=none",
        ]
    elif args.test_output:
        bazel_test_args += [f"--test_output={args.test_output}"]

    # Detect when to use remote service endpoint overrides from infra.
    # TODO(https://fxbug.dev/450234102): Deprecate legacy BAZEL_*_socket_path after recipes migrate to main_build.py.
    # LINT.IfChange(bazel_socket_env_vars)
    SERVICE_SOCKET_MAP = [
        (
            ["resultstore", "resultstore_infra"],
            [
                "FX_INTERNAL_BAZEL_RESULTSTORE_SOCKET_PATH",
                "BAZEL_resultstore_socket_path",  # legacy fallback
            ],
            "--bes_proxy",
        ),
        (
            ["remote", "remote_cache_only"],
            [
                "FX_INTERNAL_BAZEL_RBE_SOCKET_PATH",
                "BAZEL_rbe_socket_path",  # legacy fallback
            ],
            "--remote_proxy",
        ),
    ]
    # LINT.ThenChange(//build/scripts/main_build.py:bazel_socket_env_vars)
    for config_args, env_vars, bazel_flag in SERVICE_SOCKET_MAP:
        if any(
            f"--config={config_arg}" in bazel_config_args
            for config_arg in config_args
        ):
            for env_var in env_vars:
                env_value = os.environ.get(env_var)
                if env_value:
                    bazel_config_args += [f"{bazel_flag}=unix://{env_value}"]
                    break

    siblings_link_template: str = ""
    for config_arg in bazel_config_args:
        if "resultstore" in config_arg:
            siblings_link_template = "http://go/fxbtx/"

    jobs = None
    if "--config=remote" in bazel_config_args:
        cpus = os.cpu_count()
        if cpus:
            jobs = 10 * cpus
        # "--config=remote_cache_only" uses local resources to execute on cache-misses,
        # so conservatively leave `jobs` default to `cpus`.
    else:
        # RBE documentation states that --disk_cache 'does not mix'
        # with it, so only support it when remote config is not used.
        disk_cache = os.environ.get("FUCHSIA_BAZEL_DISK_CACHE")
        if disk_cache:
            bazel_common_args += [f"--disk_cache={disk_cache}"]
            disk_cache_size = os.environ.get("FUCHSIA_BAZEL_DISK_CACHE_SIZE")
            if disk_cache_size:
                bazel_common_args += [
                    f"--experimental_disk_cache_gc_max_size={disk_cache_size}"
                ]

    # Pass explicit job count if needed. https://fxbug.dev/351623259
    job_count = os.environ.get("FUCHSIA_BAZEL_JOB_COUNT")
    if job_count:
        jobs = int(job_count)

    # With unrestricted LOAS credentials, the credential helper
    # can renew OAuth tokens automatically.
    is_remote_build = any(
        a.startswith("--config=remote") for a in bazel_config_args
    )
    if (
        is_remote_build
        and os.environ.get("FX_BUILD_LOAS_TYPE", "") == "unrestricted"
    ):
        bazel_config_args += ["--config=gcertauth"]
        # Tell bazel to not look for the app. credential file,
        # because it may not exist.
        os.environ.pop("GOOGLE_APPLICATION_CREDENTIALS", None)

    bazel_config_args += build_metadata_flags(siblings_link_template)

    if args.bazel_build_events_log_json:
        args.bazel_build_events_log_json.parent.mkdir(
            parents=True, exist_ok=True
        )
        bazel_config_args += [
            "--build_event_json_file=%s"
            % args.bazel_build_events_log_json.resolve()
        ]

    # Bazel action execution log.
    # This contains records of local and remote executions.
    # Same as --config=exec_log from template.bazelrc
    if args.bazel_exec_log_compact:
        args.bazel_exec_log_compact.parent.mkdir(parents=True, exist_ok=True)
        bazel_config_args += [
            "--execution_log_compact_file=%s"
            % args.bazel_exec_log_compact.resolve(),
            "--remote_build_event_upload=all",
        ]

    if args.clean:
        # Perform clean build
        ret = _run_command(
            bazel_startup_args + ["clean", "--expunge"],
            check_failure=False,
            cwd=workspace_dir,
        )
        if ret.returncode != 0:
            return _print_error(
                "Could not clean bazel output base?\n%s\n" % ret.stderr
            )

    PATH = os.environ["PATH"]

    bazel_env = {
        # An undocumented, but widely used, environment variable that tells Bazel to
        # not auto-detect the host C++ installation. This makes workspace setup faster
        # and ensures this can be used on containers where GCC or Clang are not
        # installed (Bazel would complain otherwise with an error).
        "BAZEL_DO_NOT_DETECT_CPP_TOOLCHAIN": "1",
        # Ensure our prebuilt Python3 executable is in the PATH to run repository
        # rules that invoke Python programs correctly in containers or jails that
        # do not expose the system-installed one.
        "PATH": f"{python_prebuilt_dir}/bin:{PATH}",
    }

    if workspace_python_version_file:
        bazel_env[
            "LOCAL_PREBUILT_PYTHON_VERSION_FILE"
        ] = workspace_python_version_file

    if workspace_clang_version_file:
        bazel_env[
            "LOCAL_FUCHSIA_CLANG_VERSION_FILE"
        ] = workspace_clang_version_file

    if input_mode == _INPUT_MODE_SDK:
        # Pass the location of the Fuchsia SDK to the
        # @fuchsia_sdk repository rule. Note that using --action_env will
        # not work because this option only affects Bazel actions, and
        # not repository rules.
        bazel_env["LOCAL_FUCHSIA_SDK_DIRECTORY"] = str(
            args.fuchsia_sdk_directory.resolve()
        )
    elif input_mode == _INPUT_MODE_IDK:
        # Pass the location of the Fuchsia IDK archive to the @fuchsia_sdk
        # repository rule.
        bazel_env["LOCAL_FUCHSIA_IDK_DIRECTORY"] = str(
            args.fuchsia_idk_directory.resolve()
        )

    # Setting USER is required to run Bazel, so force it to run on infra bots.
    if "USER" not in os.environ:
        bazel_env["USER"] = "unused-bazel-build-user"

    query_target = f"set({args.test_target})"

    command_kwargs = {"env": os.environ | bazel_env, "cwd": workspace_dir}

    command_args = []

    if args.command == "info":
        command_args = bazel_startup_args + ["info"] + extra_args

    elif args.command == "query":
        command_args = (
            bazel_startup_args + ["query"] + bazel_common_args + extra_args
        )
    elif args.command == "vendor":
        command_args = (
            bazel_startup_args
            + ["vendor"]
            + bazel_common_args
            + bazel_config_args
            + extra_args
        )
    else:
        command_args = (
            bazel_startup_args
            + [args.command]
            + bazel_common_args
            + bazel_config_args
            + bazel_test_args
            + ([args.test_target] if args.command == "test" else [])
            + extra_args
        )

    if jobs and args.command not in ("query", "info"):
        command_args += [f"--jobs={jobs}"]

    # If the test failed, exit early with a non-zero error code, (but don't
    # raise an exception, because the stack trace printed by that will just be
    # noise in the failure output.
    _run_command(command_args, check_failure=True, **command_kwargs)

    # Shutdown the Bazel server daemon immediately to avoid Ninja build timeouts
    # See https://fxbug.dev/498320348
    _run_command(
        bazel_startup_args + ["shutdown"],
        check_failure=False,
        cwd=workspace_dir,
    )

    if args.stamp_file:
        args.stamp_file.write_bytes(b"")

    if args.depfile:

        def find_build_files() -> set[Path]:
            # Perform a query to retrieve all build files.
            build_files = _get_command_output_lines(
                args=(
                    bazel_startup_args
                    + ["query"]
                    + bazel_common_args
                    + bazel_quiet_args
                    + ["buildfiles(deps(%s))" % query_target]
                ),
                extra_env=bazel_env,
                cwd=workspace_dir,
            )
            result = set()
            for b in build_files:
                resolved = bazel_repo_map.resolve_bazel_path(b)
                if resolved:
                    result.add(resolved)

            return result

        def find_source_files() -> set[Path]:
            # Perform a cquery to find all input source files.
            lines = _get_command_output_lines(
                args=(
                    bazel_startup_args
                    + ["cquery"]
                    + bazel_common_args
                    + bazel_config_args
                    + bazel_quiet_args
                    + [
                        "--output=label",
                        'kind("source file", deps(%s))' % query_target,
                    ]
                ),
                extra_env=bazel_env,
                cwd=workspace_dir,
            )
            source_files = set()
            for l in lines:
                path, space, label = l.partition(" ")
                assert (
                    space == " " and label == "(null)"
                ), f"Invalid source file line: {l}"
                resolved = bazel_repo_map.resolve_bazel_path(path)
                if not resolved:
                    continue
                # If the file is a symlink, find its real location
                resolved = resolved.resolve()
                source_files.add(resolved)

            return source_files

        outputs = [args.stamp_file]
        implicit_inputs = [
            _relative_path(p)
            for p in (find_build_files() | find_source_files())
        ]
        with open(args.depfile, "w") as f:
            f.write(
                "%s: %s\n"
                % (
                    " ".join(_depfile_quote(str(p)) for p in outputs),
                    " ".join(_depfile_quote(str(p)) for p in implicit_inputs),
                )
            )

    return 0


if __name__ == "__main__":
    sys.exit(main())
