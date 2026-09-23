#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Python implementation of 'fint build' as a command wrapper."""

import pathlib
import shlex
import sys

# Find the fuchsia root containing prebuilt/third_party/protobuf-py3.
# This search is necessary because in hermetic test sandboxes (like fx_build_test),
# the directory structure is flattened to 3 levels deep instead of the usual 4,
# so a fixed relative path lookup (parent.parent.parent.parent) would fail.
PREBUILT_PROTOBUF_DIR = pathlib.Path("prebuilt/third_party/protobuf-py3")

fint_dir = pathlib.Path(__file__).resolve().parent
fuchsia_root = fint_dir
found_root = False
for _ in range(5):
    if (fuchsia_root / PREBUILT_PROTOBUF_DIR).exists():
        found_root = True
        break
    fuchsia_root = fuchsia_root.parent

if not found_root:
    raise FileNotFoundError(
        f"Could not find valid Fuchsia root containing {PREBUILT_PROTOBUF_DIR}"
    )

# Append fuchsia_root to sys.path so we can import tools.integration.fint.proto
if fuchsia_root.exists() and str(fuchsia_root) not in sys.path:
    sys.path.insert(0, str(fuchsia_root))

protobuf_wheel = fuchsia_root / PREBUILT_PROTOBUF_DIR
if protobuf_wheel.exists() and str(protobuf_wheel) not in sys.path:
    sys.path.insert(0, str(protobuf_wheel))

# Bypass strict protobuf gencode/runtime version check to align prebuilts at build-time (see b/537501139)
try:
    import google.protobuf.runtime_version as rv  # type: ignore

    rv.ValidateProtobufRuntimeVersion = lambda *args, **kwargs: None
except ImportError:
    pass

import argparse
import functools
import json
import platform
import subprocess
import tempfile
import time
from contextlib import contextmanager
from dataclasses import dataclass
from typing import Any, Generator, Iterable, TextIO

from build.scripts import signal_utils
from google.protobuf import json_format, text_format
from tools.integration.fint.proto import (
    build_artifacts_pb2,
    context_pb2,
    static_pb2,
)

JSONObject = dict[str, Any]
JSONArray = list[Any]

# Module-scope constants for file names
BUILD_ARTIFACTS_JSON = "build_artifacts.json"
NINJA_ERRORS_JSON = "ninja_errors.json"
TOOL_PATHS_JSON = "tool_paths.json"
TESTS_JSON = "tests.json"
GENERATED_SOURCES_JSON = "generated_sources.json"
PREBUILT_BINARY_SETS_JSON = "prebuilt_binary_sets.json"
FORCE_NONHERMETIC_REBUILD_SENTINEL = "force_nonhermetic_rebuild"
LAST_NINJA_BUILD_SUCCESS_STAMP = "last_ninja_build_success.stamp"


@dataclass
class BuildExecution:
    """Represents the parameters and result of a wrapped build command run."""

    command: list[str]
    exit_code: int = 0


class Timer:
    """Context manager to measure elapsed duration in seconds."""

    def __init__(self) -> None:
        self.duration: float = 0.0
        self._start: float = 0.0

    def __enter__(self) -> "Timer":
        self._start = time.time()
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: Any,
    ) -> None:
        self.duration = time.time() - self._start


_SCRIPT_NAME = pathlib.Path(__file__).name


def msg(msg: str, file: TextIO = sys.stdout) -> None:
    """Standardized logger that prefixes messages with the script name."""
    print(f"[{_SCRIPT_NAME}] {msg}", file=file)


def ts_msg(msg: str, verbose: bool = False, file: TextIO = sys.stderr) -> None:
    """Logs high-precision timestamps to stderr in verbose mode."""
    if verbose:
        print(f"[{time.time():.9f}] [{_SCRIPT_NAME}]: {msg}", file=file)


@dataclass(frozen=True)
class HostProperties:
    """Represents host system properties like OS and CPU architecture."""

    os: str
    cpu: str

    @classmethod
    def detect(cls) -> "HostProperties":
        """Auto-detects the host system's OS and CPU architecture."""
        host_os = platform.system().lower()
        if host_os == "darwin":
            host_os = "mac"
        host_cpu = platform.machine()
        if host_cpu == "x86_64":
            host_cpu = "x64"
        elif host_cpu in ["aarch64", "arm64"]:
            host_cpu = "arm64"
        return cls(os=host_os, cpu=host_cpu)

    @property
    def is_mac(self) -> bool:
        """Returns True if the host is running macOS."""
        return self.os == "mac"

    @property
    def is_linux(self) -> bool:
        """Returns True if the host is running Linux."""
        return self.os == "linux"

    @property
    def platform_dir(self) -> str:
        """Returns the prebuilt platform directory name (e.g. linux-x64, mac-x64)."""
        return f"{self.os}-{self.cpu}"

    @property
    def gn_relative_path(self) -> pathlib.Path:
        """Returns the relative path to the prebuilt GN tool."""
        return (
            pathlib.Path("prebuilt")
            / "third_party"
            / "gn"
            / self.platform_dir
            / "gn"
        )

    @property
    def ninja_relative_path(self) -> pathlib.Path:
        """Returns the relative path to the prebuilt Ninja tool."""
        return (
            pathlib.Path("prebuilt")
            / "third_party"
            / "ninja"
            / self.platform_dir
            / "ninja"
        )

    def matches_tool(self, tool: JSONObject) -> bool:
        """Returns True if the tool's OS and CPU match this host."""
        return tool.get("os") == self.os and tool.get("cpu") == self.cpu


def load_static_spec(path: pathlib.Path) -> static_pb2.Static:
    """Loads and unmarshals the static spec textproto."""
    spec = static_pb2.Static()
    text_format.Merge(path.read_text(), spec)
    return spec


def load_context_spec(path: pathlib.Path) -> context_pb2.Context:
    """Loads and unmarshals the context spec textproto."""
    spec = context_pb2.Context()
    text_format.Merge(path.read_text(), spec)
    return spec


def load_json_list(path: pathlib.Path) -> JSONArray:
    """Loads a JSON list file.

    Raises FileNotFoundError, JSONDecodeError, or ValueError if the file is
    missing, malformed, or does not contain a list.
    """
    data = json.loads(path.read_text())
    if not isinstance(data, list):
        raise ValueError(
            f"Expected JSON list in file {path}, but got: {type(data).__name__}"
        )
    return data


def produce_build_artifacts(
    artifact_dir: pathlib.Path,
    duration_seconds: int,
    failure_summary: str | None = None,
) -> None:
    """Serializes and writes the build_artifacts.json manifest to the artifact directory."""
    artifacts = build_artifacts_pb2.BuildArtifacts()
    artifacts.ninja_duration_seconds = duration_seconds
    if failure_summary:
        artifacts.failure_summary = failure_summary

    json_manifest_path = artifact_dir / BUILD_ARTIFACTS_JSON
    # MessageToJson formats with nice spacing/indentation
    json_data = json_format.MessageToJson(
        artifacts, always_print_fields_with_no_presence=True
    )
    json_manifest_path.write_text(json_data)
    msg(f"Successfully wrote build artifacts manifest to {json_manifest_path}")


@dataclass(frozen=True)
class NinjaFailure:
    """Represents a single action failure recorded in ninja_errors.json."""

    artifacts: list[str]
    exit_code: int
    output: str

    @classmethod
    def from_dict(cls, data: JSONObject) -> "NinjaFailure":
        """Constructs a NinjaFailure from a JSONObject."""
        artifacts = data.get("artifacts")
        exit_code = data.get("exit_code")
        output = data.get("output", "")

        # Coerce/validate fields
        if not isinstance(artifacts, list):
            artifacts = [str(artifacts)] if artifacts is not None else []
        try:
            exit_code_int = int(exit_code) if exit_code is not None else -1
        except (ValueError, TypeError):
            exit_code_int = -1

        return cls(
            artifacts=[str(a) for a in artifacts],
            exit_code=exit_code_int,
            output=str(output).strip(),
        )

    @property
    def is_eligible_for_deduplication(self) -> bool:
        """Returns True if the failure output is eligible for deduplication (>= 5 lines)."""
        return bool(self.output and self.output.count("\n") >= 5)

    def format(self, include_output: bool = True) -> str:
        """Formats the failure into a high-signal error block."""
        artifacts_str = shlex.join(self.artifacts)
        header = f"FAILED: [code={self.exit_code}] {artifacts_str}"
        if include_output and self.output:
            return f"{header}\n\n{self.output}"
        return header


def parse_ninja_failures(errors_json_path: pathlib.Path) -> str | None:
    """Parses ninja_errors.json file to construct a precise, formatted FailureSummary.

    Args:
        errors_json_path: Path to the ninja_errors.json file.

    Returns:
        The formatted FailureSummary string, or None if malformed, empty, or missing.
    """
    if not errors_json_path.exists():
        return None

    try:
        with open(errors_json_path, "r") as f:
            data = json.load(f)
        return format_ninja_failures(data)
    except (json.JSONDecodeError, FileNotFoundError):
        return None


def format_ninja_failures(data: JSONObject) -> str | None:
    """Formats already loaded ninja_errors.json data into a precise FailureSummary.

    Args:
        data: The decoded JSON dictionary.

    Returns:
        The formatted FailureSummary string, or None if malformed or unsupported.
    """
    if not isinstance(data, dict) or data.get("version") != 1:
        return None

    raw_failures = data.get("failures", [])
    if not isinstance(raw_failures, list) or not raw_failures:
        return None

    failures = []
    for raw in raw_failures:
        if isinstance(raw, dict):
            failures.append(NinjaFailure.from_dict(raw))

    if not failures:
        return None

    msg_lines = []
    seen_outputs = set()
    for failure in failures:
        # Check deduplication eligibility
        if failure.is_eligible_for_deduplication:
            if failure.output in seen_outputs:
                # Still output the failure header, but omit the redundant compiler logs
                msg_lines.append(failure.format(include_output=False))
                msg_lines.append("")
                continue
            seen_outputs.add(failure.output)

        msg_lines.append(failure.format(include_output=True))
        msg_lines.append("")

    return "\n".join(msg_lines).strip()


def run_gn_check(
    checkout_dir: pathlib.Path,
    build_dir: pathlib.Path,
    host: HostProperties,
    verbose: bool = False,
) -> int:
    """Runs 'gn check' to verify header dependency rules inside the build directory.

    Args:
        checkout_dir: Path to the Fuchsia checkout root.
        build_dir: Path to the active build directory.
        host: Resolved properties of the host platform.
        verbose: Enable verbose logging.

    Returns:
        The exit code of the 'gn check' subprocess.
    """
    gn_bin = checkout_dir / host.gn_relative_path
    cmd = [
        str(gn_bin),
        "check",
        str(build_dir),
        f"--root={checkout_dir}",
        "--check-generated",
        "--check-system",
    ]
    msg("Running gn check...")
    if verbose:
        msg(f"Command: {shlex.join(cmd)}")
    res = subprocess.run(cmd)
    return res.returncode


def check_ninja_noop(
    checkout_dir: pathlib.Path,
    build_dir: pathlib.Path,
    host: HostProperties,
    targets: list[str],
    verbose: bool = False,
) -> int:
    """Verifies that the Ninja build converges to a no-op state.

    This function does a dry-run invocation of Ninja with 'explain' and
    '--dirty_sources_list' arguments to verify that there are no pending dirty
    rebuild targets. If Ninja identifies any dirty source files, they are
    logged and displayed to help diagnose the non-no-op build state.

    Args:
        checkout_dir: Path to the Fuchsia checkout root.
        build_dir: Path to the active build directory.
        host: Resolved properties of the host platform.
        targets: Concrete list of Ninja targets to verify.
        verbose: Enable verbose logging.

    Returns:
        0 if the build successfully converges to a no-op, non-zero if Ninja
        diverges or fails.
    """
    ninja_bin = checkout_dir / host.ninja_relative_path

    with tempfile.TemporaryDirectory() as td:
        dirty_sources_path = pathlib.Path(td) / "dirty_sources.txt"

        cmd = [
            str(ninja_bin),
            "-C",
            str(build_dir),
            "-n",
            "-v",
            "-d",
            "explain",
            "--dirty_sources_list",
            str(dirty_sources_path),
        ] + targets

        msg("Verifying ninja build converges to no-op...")
        if verbose:
            msg(f"Command: {shlex.join(cmd)}")

        res = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
        )

        if res.returncode != 0:
            return res.returncode

        if "ninja: no work to do." in res.stdout:
            return 0

        # Handle non-noop build
        msg(
            "Error: Ninja build did not converge to no-op.",
            file=sys.stderr,
        )
        print(res.stdout[:1000], file=sys.stderr)

        # Print the dirty sources list if any were captured
        if dirty_sources_path.exists():
            dirty_content = dirty_sources_path.read_text().strip()
            if dirty_content:
                msg(
                    f"Identified dirty source files:\n{dirty_content}",
                    file=sys.stderr,
                )
        return 1


def collect_failure_diagnostics(build_dir: pathlib.Path) -> None:
    msg("Build failed. Diagnostic logs would be collected here.")


@dataclass(frozen=True)
class BuildContext:
    """Cohesive grouping of the static spec, context spec, and host properties."""

    static_spec: static_pb2.Static
    context_spec: context_pb2.Context
    host: HostProperties
    verbose: bool = False

    @property
    def build_dir(self) -> pathlib.Path:
        """Returns the path to the build directory."""
        return pathlib.Path(self.context_spec.build_dir)

    @property
    def checkout_dir(self) -> pathlib.Path:
        """Returns the path to the checkout directory."""
        return pathlib.Path(self.context_spec.checkout_dir)

    @functools.cached_property
    def tool_paths(self) -> JSONArray:
        """Loads and returns the tool paths config list."""
        return load_json_list(self.build_dir / TOOL_PATHS_JSON)

    @functools.cached_property
    def test_specs(self) -> JSONArray:
        """Loads and returns the test specs list."""
        path = self.build_dir / TESTS_JSON
        if not path.exists():
            return []
        return load_json_list(path)

    @functools.cached_property
    def generated_sources(self) -> JSONArray:
        """Loads and returns the generated sources list."""
        path = self.build_dir / GENERATED_SOURCES_JSON
        if not path.exists():
            return []
        return load_json_list(path)

    @functools.cached_property
    def prebuilt_binary_sets(self) -> JSONArray:
        """Loads and returns the prebuilt binary sets list."""
        path = self.build_dir / PREBUILT_BINARY_SETS_JSON
        if not path.exists():
            return []
        return load_json_list(path)

    def _default_and_host_test_targets(self) -> Iterable[str]:
        """Yields default targets or host test targets if configured."""
        if self.static_spec.include_default_ninja_target:
            yield ":default"
        elif self.static_spec.include_host_tests:
            for spec in self.test_specs:
                test_spec = spec.get("test", {})
                if test_spec.get("os") != "fuchsia":
                    path = test_spec.get("path")
                    if path:
                        yield path

    def _generated_source_targets(self) -> Iterable[str]:
        """Yields generated C++ source targets if configured."""
        if self.static_spec.include_generated_sources:
            for f in self.generated_sources:
                if f.endswith(".cc") or f.endswith(".h"):
                    yield f

    def _prebuilt_binary_manifests(self) -> Iterable[str]:
        """Yields prebuilt binary manifest targets if configured."""
        if self.static_spec.include_prebuilt_binary_manifests:
            for item in self.prebuilt_binary_sets:
                manifest = item.get("manifest")
                if manifest:
                    yield manifest

    def _tool_targets(self) -> Iterable[str]:
        """Yields prebuilt host tool targets if configured."""
        if self.static_spec.tools:
            for tool in self.static_spec.tools:
                path = lookup_tool_path(self.tool_paths, tool, self.host)
                if path:
                    yield path

    def _custom_ninja_targets(self) -> Iterable[str]:
        """Yields custom static Ninja targets if configured."""
        if self.static_spec.ninja_targets:
            yield from self.static_spec.ninja_targets

    def _stream_all_targets(self) -> Iterable[str]:
        """Streams all configured and resolved targets from all sources."""
        yield from self._default_and_host_test_targets()
        yield from self._generated_source_targets()
        yield from self._prebuilt_binary_manifests()
        yield from self._tool_targets()
        yield from self._custom_ninja_targets()

    def _get_targets(self) -> list[str]:
        """Resolves Ninja build targets based on specifications and build API JSON files."""
        return sorted(list(set(self._stream_all_targets())))

    def _build_bazel_host_tests(self) -> None:
        """Builds Bazel host tests if any are present in tests.json."""
        bazel_labels = []
        for spec in self.test_specs:
            test_spec = spec.get("test", {})
            label = test_spec.get("label", "")
            if label.startswith("@"):
                bazel_labels.append(label)

        if not bazel_labels:
            return

        top_dir_config_path = (
            self.checkout_dir / "build" / "bazel" / "config" / "bazel_top_dir"
        )
        bazel_top_dir = top_dir_config_path.read_text().strip()
        bazel_launcher = self.build_dir / bazel_top_dir / "bazel"

        cmd = [
            str(bazel_launcher),
            "build",
            "--config=host",
            "--build_runfile_links=true",
            "--enable_runfiles=true",
        ] + bazel_labels

        msg(f"Building Bazel host tests: {bazel_labels}")
        subprocess.run(cmd, check=True)

    @contextmanager
    def wrap_ninja(
        self,
        base_command: list[str],
    ) -> Generator[BuildExecution, None, None]:
        """Context manager wrapping the Ninja pre-build and post-build events."""
        if not self.context_spec.checkout_dir:
            raise ValueError(
                "checkout_dir is required in the Context specification"
            )

        rebuild_sentinel_path = (
            self.build_dir / FORCE_NONHERMETIC_REBUILD_SENTINEL
        )
        success_stamp_path = self.build_dir / LAST_NINJA_BUILD_SUCCESS_STAMP

        # Pre-build: Touch rebuild sentinel if incremental
        if self.static_spec.incremental:
            rebuild_sentinel_path.write_text("")

        # Clear previous success stamp
        if success_stamp_path.exists():
            success_stamp_path.unlink()

        # Target expansion and handling
        targets = self._get_targets()
        full_command = base_command + targets

        result = BuildExecution(command=full_command)
        success = False
        try:
            yield result
            if result.exit_code != 0:
                return

            self._run_post_ninja_checks(result, targets, success_stamp_path)
            success = result.exit_code == 0
        finally:
            if not success:
                collect_failure_diagnostics(self.build_dir)

    def _run_post_ninja_checks(
        self,
        result: BuildExecution,
        targets: list[str],
        success_stamp_path: pathlib.Path,
    ) -> None:
        """Runs post-build tests and validation checks for Ninja, modifying result.exit_code if any fail."""
        self._build_bazel_host_tests()

        # Post-build success stamp
        success_stamp_path.write_text("")

        # Post-build verification checks
        gn_status = run_gn_check(
            checkout_dir=self.checkout_dir,
            build_dir=self.build_dir,
            host=self.host,
            verbose=self.verbose,
        )
        if gn_status != 0:
            result.exit_code = gn_status
            return

        if not self.context_spec.skip_ninja_noop_check:
            noop_status = check_ninja_noop(
                checkout_dir=self.checkout_dir,
                build_dir=self.build_dir,
                host=self.host,
                targets=targets,
                verbose=self.verbose,
            )
            if noop_status != 0:
                result.exit_code = noop_status
                return

    @contextmanager
    def wrap_bazel(
        self,
        base_command: list[str],
    ) -> Generator[BuildExecution, None, None]:
        """Context manager wrapping the Bazel pre-build and post-build events."""
        if not self.context_spec.checkout_dir:
            raise ValueError(
                "checkout_dir is required in the Context specification"
            )

        result = BuildExecution(command=base_command)
        success = False
        try:
            yield result
            if result.exit_code == 0:
                # TODO: Implement Bazel-specific post-build checks and success actions
                pass
            success = True
        finally:
            if not success:
                # TODO: Implement Bazel-specific post-build failure/diagnostic collections
                pass


def lookup_tool_path(
    tool_paths: list[JSONObject], tool_name: str, host: HostProperties
) -> str | None:
    """Looks up the relative path of a host tool in tool_paths."""
    for tool in tool_paths:
        if tool.get("name") == tool_name and host.matches_tool(tool):
            return tool.get("path")
    return None


def make_build_context(
    static_path: pathlib.Path,
    context_path: pathlib.Path | None,
    verbose: bool = False,
) -> BuildContext:
    """Creates a BuildContext by loading and parsing specifications, auto-detecting host properties."""
    static_spec = load_static_spec(static_path)

    if context_path:
        context_spec = load_context_spec(context_path)
    else:
        context_spec = context_pb2.Context(
            checkout_dir=str(fuchsia_root.resolve()),
            build_dir=str(pathlib.Path.cwd().resolve()),
        )

    host = HostProperties.detect()
    return BuildContext(static_spec, context_spec, host, verbose)


def _main_arg_parser() -> argparse.ArgumentParser:
    """Constructs and returns the command line argument parser."""
    parser = argparse.ArgumentParser(
        description="Python-based fint build command wrapper"
    )
    parser.add_argument(
        "--static",
        required=False,
        default=None,
        type=pathlib.Path,
        help="Path to static spec",
    )
    parser.add_argument(
        "--context",
        required=False,
        default=None,
        type=pathlib.Path,
        help="Path to context spec. If omitted, it will be automatically generated.",
    )
    parser.add_argument(
        "--print-artifact-dir",
        action="store_true",
        help="Print the resolved artifact directory from the context spec and exit.",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Enable verbose output logging.",
    )
    parser.add_argument(
        "--mode",
        choices=["ninja", "bazel"],
        default="ninja",
        help="Build system wrapper mode (ninja or bazel). Default is ninja.",
    )
    parser.add_argument(
        "--ninja-error-logging-output",
        dest="ninja_error_logging_output",
        type=pathlib.Path,
        default=None,
        help="Path where Ninja should write its error logs (ninja_errors.json).",
    )
    parser.add_argument(
        "wrapped_cmd",
        nargs="*",
        default=None,
        help="The wrapped command to execute (optionally after '--')",
    )
    return parser


def main(argv: list[str]) -> int:
    # Verify that we are running in a valid Fuchsia checkout environment when executed.
    if not (fuchsia_root / ".jiri_manifest").exists():
        msg(
            f"INTERNAL ERROR: Could not find valid Fuchsia root: {fuchsia_root}",
            file=sys.stderr,
        )
        return 1

    parser = _main_arg_parser()
    args = parser.parse_args(argv)

    if args.print_artifact_dir:
        if not args.context:
            msg(
                "Error: --context is required with --print-artifact-dir",
                file=sys.stderr,
            )
            return 1
        context_spec = load_context_spec(args.context)
        if context_spec.artifact_dir:
            print(context_spec.artifact_dir)
        return 0

    # Ensure build execution requirements are satisfied if not querying
    is_static_valid = args.static and str(args.static) not in (".", "")
    required_build_args = [
        (is_static_valid, "the following arguments are required: --static"),
        (args.wrapped_cmd, "wrapped_cmd is required for build execution"),
    ]
    for val, err_msg in required_build_args:
        if not val:
            msg(f"Error: {err_msg}", file=sys.stderr)
            return 2

    ts_msg("Fint build wrapper starting up...", args.verbose)

    ctx = make_build_context(args.static, args.context, verbose=args.verbose)

    # Select Build Strategy
    wrappers = {
        "ninja": ctx.wrap_ninja,
        "bazel": ctx.wrap_bazel,
    }
    wrapper = wrappers[args.mode]

    with wrapper(args.wrapped_cmd) as run:
        if args.verbose:
            msg(f"Delegated command: {shlex.join(run.command)}")
            ts_msg(
                f"Executing delegated build command: {shlex.join(run.command)}",
                args.verbose,
            )

        with Timer() as t:
            try:
                # Wrap the delegated command execution in SignalManagedProcess to gracefully
                # handle and relay process signals (such as Ctrl+C / SIGINT) to Ninja/Bazel.
                managed = signal_utils.SignalManagedProcess(
                    run.command, verbose=args.verbose
                )
                exit_code = managed.run()
            except signal_utils.BuildInterruptedError as e:
                # If interrupted, propagate the signal-derived exit code (128 + signum)
                exit_code = e.return_code
                msg(f"Build interrupted by signal {e.signum}")
        duration_seconds = round(t.duration)

        run.exit_code = exit_code
        ts_msg(
            f"Delegated build command completed with status {run.exit_code} (duration: {duration_seconds}s)",
            args.verbose,
        )

        # If artifact_dir is specified, serialize and write build_artifacts.json
        # unconditionally on both success and failure.
        if ctx.context_spec.artifact_dir:
            ts_msg(
                f"Serializing build artifacts manifest to {ctx.context_spec.artifact_dir}...",
                args.verbose,
            )
            failure_summary = None
            if run.exit_code != 0:
                if args.ninja_error_logging_output:
                    failure_summary = parse_ninja_failures(
                        args.ninja_error_logging_output
                    )

                if not failure_summary:
                    failure_summary = (
                        f"Fuchsia build failed: delegated command "
                        f"'{shlex.join(run.command)}' exited with status {run.exit_code}"
                    )
            produce_build_artifacts(
                pathlib.Path(ctx.context_spec.artifact_dir),
                duration_seconds,
                failure_summary=failure_summary,
            )

    return run.exit_code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
