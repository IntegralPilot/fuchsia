# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from dataclasses import dataclass
from datetime import datetime
import enum
import os
import pathlib
import re
import sys
import typing

import agents.agents as agents_lib
import termout

import selection_action

LOG_TO_STDOUT_OPTION = "-"


class PrevOption(enum.StrEnum):
    LOG = "log"
    PATH = "path"
    REPLAY = "replay"
    ARTIFACT_PATH = "artifact-path"
    FAILED_TESTS = "failed-tests"
    HELP = "help"
    STATS = "stats"
    STATS_ANALYTICS = "stats-analytics"

    def help(self) -> str:
        """Get the help string for this option.

        Raises:
            RuntimeError: If an invalid option is passed.

        Returns:
            str: Human-readable information about how this options is used.
        """
        if self is PrevOption.LOG:
            return "Print all test logs from the previous run. Logs are grouped by test suite."
        elif self is PrevOption.PATH:
            return "Print the path of the log from the previous run."
        elif self is PrevOption.REPLAY:
            return "Replay the previous run, using new display options."
        elif self is PrevOption.ARTIFACT_PATH:
            return "Print the path where artifacts were stored in the previous run."
        elif self is PrevOption.FAILED_TESTS:
            return "Print the previously failed tests and how to run them."
        elif self is PrevOption.HELP:
            return "Print this help output."
        elif self is PrevOption.STATS:
            return "Print statistics from the previous run"
        elif self is PrevOption.STATS_ANALYTICS:
            return "Print statistics from the previous run formatted for analytics (minified JSON)."
        else:
            raise RuntimeError("BUG: Invalid prev option")


@dataclass
class Flags:
    """Command line flags for fx test.

    See `parse_args` for documentation.
    """

    dry: bool
    show_affected_tests: bool
    run_affected_tests: bool
    affected_since: str | None
    list: bool
    list_runtime_deps: bool
    previous: PrevOption | None
    remote_suggestions: bool
    remote_suggestion_builder: typing.List[str]

    gemini_analysis: bool
    gemini_model: str
    build: bool
    updateifinbase: bool
    build_updates: bool

    host: bool
    device: bool
    exact: bool
    e2e: bool
    only_e2e: bool
    selection: typing.List[str]
    fuzzy: int
    allow_empty_selection: bool
    show_suggestions: bool
    suggestion_count: int

    parallel: int
    parallel_cases: int
    random: bool
    count: int
    limit: int | None
    offset: int
    min_severity_logs: typing.List[str]
    timeout: float | None
    timeout_grace_period: float
    test_filter: typing.List[str]
    fail: bool
    fail_by_group: bool
    use_package_hash: bool
    restrict_logs: bool
    also_run_disabled_tests: bool
    show_full_moniker_in_logs: bool
    capture_syslog: bool
    break_on_failure: bool
    breakpoints: typing.List[str]
    use_existing_debugger: bool
    enable_debug_adapter: bool
    debug_adapter_port: int | None
    agent_debugging_mode: bool
    use_test_pilot: bool
    extra_args: typing.List[str]
    env: typing.List[str]
    allow_temporary_package_server: bool
    allow_temporary_emulator: bool
    ffx_usb_socket_path: str | None

    output: bool
    simple: bool
    style: bool
    log: bool
    logpath: str | None
    json: bool
    status: bool | None
    verbose: bool
    status_lines: int
    status_delay: float
    timestamp_artifacts: bool
    artifact_output_directory: str | None
    slow: float
    quiet: bool
    replay_speed: float
    summary_json: str | None
    summary_to_stdout: bool
    agent_output: bool
    save_log_path_to_file: str | None = None

    def validate(self) -> None:
        """Validate incoming flags, raising an exception on failure.

        Raises:
            FlagError: If the flags are invalid.
        """
        if self.agent_debugging_mode:
            if self.break_on_failure is False:
                raise FlagError(
                    "--no-break-on-failure is incompatible with --agent-debugging-mode"
                )
            if self.enable_debug_adapter is False:
                raise FlagError(
                    "--no-enable-debug-adapter is incompatible with --agent-debugging-mode"
                )
            self.break_on_failure = True
            self.enable_debug_adapter = True

        if self.break_on_failure is None:
            self.break_on_failure = False
        if self.enable_debug_adapter is None:
            self.enable_debug_adapter = False

        if self.simple and self.status:
            raise FlagError("--simple is incompatible with --status")
        if self.simple and self.style:
            raise FlagError("--simple is incompatible with --style")
        if self.agent_output and self.status:
            raise FlagError("--agent-output is incompatible with --status")
        if self.agent_output and self.style:
            raise FlagError("--agent-output is incompatible with --style")
        if self.device and self.host:
            raise FlagError("--device is incompatible with --host")
        if self.status_delay < 0.005:
            raise FlagError("--status-delay must be at least 0.005 (5ms)")
        if self.timeout and self.timeout < 0:
            raise FlagError("--timeout must be non-negative")
        if self.timeout_grace_period and self.timeout_grace_period <= 0:
            raise FlagError("--timeout-grace-period must be greater than 0")
        if self.count < 1:
            raise FlagError("--count must be a positive number")
        if self.suggestion_count < 0:
            raise FlagError("--suggestion-count must be non-negative")
        if (
            self.artifact_output_directory is not None
            and pathlib.Path(self.artifact_output_directory).is_file()
        ):
            raise FlagError("--artifact-output-directory cannot be a file")
        if self.parallel < 0:
            raise FlagError("--parallel must be non-negative")
        if self.parallel_cases < 0:
            raise FlagError("--parallel-cases must be non-negative")
        if self.debugger_will_attach() and self.host:
            raise FlagError(
                "--break-on-failure and --breakpoint flags are not supported with host tests."
            )
        if self.use_existing_debugger and self.breakpoints:
            raise FlagError(
                "--breakpoint does not support --use-existing-debugger."
            )
        if self.use_existing_debugger and self.enable_debug_adapter:
            raise FlagError(
                "--enable-debug-adapter and --use-existing-debugger are mutually exclusive."
            )
        if self.use_existing_debugger and not self.break_on_failure:
            raise FlagError(
                "--break-on-failure must be set when passing --use-existing-debugger."
            )
        if self.enable_debug_adapter and not self.debugger_will_attach():
            raise FlagError(
                "--enable-debug-adapter must be used with either --breakpoint or --break-on-failure."
            )
        if self.debug_adapter_port and not self.enable_debug_adapter:
            raise FlagError(
                "--enable-debug-adapter must be set when passing --debug-adapter-port."
            )
        if self.replay_speed <= 0:
            raise FlagError("--replay-speed must be a positive number")

        if not termout.is_valid() and self.status:
            raise FlagError(
                "Refusing to output interactive status to a non-TTY."
            )

        if (
            self.artifact_output_directory is not None
            and self.timestamp_artifacts
        ):
            self.artifact_output_directory = os.path.join(
                self.artifact_output_directory,
                datetime.now().strftime("%Y-%m-%d-%H:%M:%S"),
            )

        if self.remote_suggestion_builder and not self.remote_suggestions:
            raise FlagError(
                "--remote-suggestion-builder is only effective when --remote-suggestions is enabled"
            )

        # Compute environment and check it is valid.
        self.computed_env()

        if self.json:
            self.logpath = LOG_TO_STDOUT_OPTION

        if self.only_e2e:
            self.e2e = True

        if self.simple or self.agent_output:
            self.style = False
            self.status = False
        else:
            if self.style is None:
                self.style = termout.is_valid()
            if self.status is None:
                self.status = termout.is_valid()

        if self.agent_output:
            self.quiet = True

    def update_artifacts_directory_with_out_path(self, path: str) -> None:
        if (
            self.artifact_output_directory is not None
            and "FUCHSIA_OUT" in self.artifact_output_directory
        ):
            self.artifact_output_directory = re.sub(
                r"(\$FUCHSIA_OUT|\$\{FUCHSIA_OUT\})",
                path,
                self.artifact_output_directory,
            )

    def debugger_will_attach(self) -> bool:
        """Determine if this set of flags enables debugging features.

        This is distinct from `Flags.debugger_should_spawn`, which indicates that debugger should be
        spawned.

        Returns:
            bool: True if a debugger will be attached, False otherwise.
        """
        return bool(self.break_on_failure or self.breakpoints)

    def debugger_should_spawn(self) -> bool:
        """Determine if this set of flags warrants the launch of a debugger.

        This is distinct from `Flags.debugger_will_attach`, which indicates that a debugger will be
        attached to the test upon execution.

        Returns:
            bool: True if a debugger needs to be spawned, False otherwise.
        """
        return bool(
            self.debugger_will_attach() and not self.use_existing_debugger
        )

    def is_replay(self) -> bool:
        """Determine if these flags specify that replay mode is active.

        Returns:
            bool: True if replay mode is active, False otherwise.
        """
        return self.previous == PrevOption.REPLAY

    def computed_env(self) -> dict[str, str]:
        """Compute and return the environment denoted by --env flags.

        Warning: This method recomputes the environment on each call, if this
        operation is too expensive we will need to memoize the return value.

        Raises:
            FlagError: If an environment variable is not formatted correctly.

        Returns:
            dict[str, str]: Mapping from key to value of environment
                variables for tests executed by this invocation of the
                command.
        """
        ret = {}
        for val in self.env:
            split = val.split("=", maxsplit=1)
            if len(split) != 2:
                raise FlagError(
                    f'Environment variable "{val}" must be formatted as "name=value"'
                )
            ret[split[0]] = split[1]
        return ret


class FlagError(Exception):
    """Raised if there was a problem parsing command line flags."""


def parse_args(
    cli_args: typing.List[str] | None = None, defaults: Flags | None = None
) -> Flags:
    """Parse command line flags.

    Args:
        cli_args (List[str], optional): Arguments to parse. If
            unset, read arguments from actual command line.
        defaults (Flags, optional): Default set of flags. If set,
            overrides the defaults from the command line.

    Returns:
        Flags: Typed representation of the command line for this program.
    """

    extra_args: typing.List[str] = []

    cli_args = cli_args if cli_args is not None else sys.argv[1:]

    if cli_args is not None and "--" in cli_args:
        extra_index = cli_args.index("--")
        (cli_args, extra_args) = (
            cli_args[:extra_index],
            cli_args[extra_index + 1 :],
        )

    parser = argparse.ArgumentParser(
        "fx test",
        description="Test Executor for Humans",
        exit_on_error=False,
    )
    utility = parser.add_argument_group("Utility Options")
    utility.add_argument(
        "--show-affected-tests",
        action="store_true",
        help="Do not actually run tests. Instead print affected tests spanning from the current dirty repository state.",
    )
    utility.add_argument(
        "--run-affected-tests",
        action="store_true",
        help="Automatically add and run tests affected by the current dirty repository state.",
    )
    utility.add_argument(
        "--affected-since",
        type=str,
        default=None,
        help="Get affected files by comparing against this commit/branch instead of checking uncommitted files in the working area.",
    )
    utility.add_argument(
        "--dry",
        action="store_true",
        help="Do not actually run tests. Instead print out the tests that would have been run.",
    )
    utility.add_argument(
        "--list",
        action="store_true",
        help="Do not actually run tests. Instead print out the list of test cases each test contains.",
    )
    utility.add_argument(
        "--gemini-analysis",
        nargs="?",
        type=int,
        const=1,
        default=None,
        choices=range(1, 4),
        help="""If specified, requests an AI-powered analysis of stack traces from test failures.
        Requires the GEMINI_API_KEY environment variable to be set.
        The Gemini model can be specified using the `--gemini-model` flag.
        An optional verbosity level from 1-3 can be provided (eg., --gemini-analysis=3).
        If no level is provided, it defaults to 1.
        Level 1: Key lines from the stack trace.
        Level 2: Key lines and a potential error from the git diff.
        Level 3: Full analysis with file contents and code snippets.
        """,
        dest="gemini_analysis",
    )
    utility.add_argument(
        "--gemini-model",
        type=str,
        default="gemini-2.5-flash-lite-preview-09-2025",
        help="The Gemini model to use for the analysis.",
        dest="gemini_model",
    )
    utility.add_argument(
        "--list-runtime-deps",
        action="store_true",
        help="Do not actually run tests. Instead print out the contents of the `runtime_deps` for each test. This can be useful for debugging whether the correct artifacts are being uploaded to test runners",
    )
    utility.add_argument(
        "--remote-suggestions",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Whether to use remote tests.json files for tests suggestions.",
    )
    parser.add_argument(
        "--remote-suggestion-builder",
        type=str,
        action="append",
        help="Add additional builder to query. May be specified multiple times.",
        default=[],
    )
    utility.add_argument(
        "-pr",
        "--prev",
        "--previous",
        dest="previous",
        type=PrevOption,
        choices=list(PrevOption),
        help=f"Do not actually run tests. Instead print information from the previous run. Input is read from the last log file, and it respects the value of --logpath.",
    )
    build = parser.add_argument_group("Build Options")
    build.add_argument(
        "--build",
        action=argparse.BooleanOptionalAction,
        help="Invoke `fx build` before running the test suite (defaults to on)",
        default=True,
    )
    build.add_argument(
        "--updateifinbase",
        action=argparse.BooleanOptionalAction,
        help="Invoke `fx update-if-in-base` before running device tests (defaults to on)",
        default=True,
    )
    build.add_argument(
        "--build-updates",
        action=argparse.BooleanOptionalAction,
        help="Build the updates package if there are device tests (defaults to on)",
        default=True,
    )

    selection = parser.add_argument_group("Test Selection Options")
    selection.add_argument(
        "--host",
        action="store_true",
        default=False,
        help="Only run host tests. The opposite of `--device`",
    )
    selection.add_argument(
        "-d",
        "--device",
        action="store_true",
        default=False,
        help="Only run device tests. The opposite of `--host`",
    )
    selection.add_argument(
        "--exact",
        action="store_true",
        default=False,
        help="""Only match tests whose name exactly matches the selection.
        Cannot be specified along with --host or --device.""",
    )
    selection.add_argument(
        "--e2e",
        action=argparse.BooleanOptionalAction,
        help="Run selected end to end tests. Default is to not run e2e tests.",
        default=False,
    )
    selection.add_argument(
        "--only-e2e",
        action="store_true",
        default=False,
        help="Only run end to end tests. Implies --e2e.",
    )
    selection.add_argument(
        "-p",
        "--package",
        action=selection_action.InvalidAction,
        nargs=0,
        dest="selection",
        help="Match tests against their Fuchsia package name",
    )
    selection.add_argument(
        "-c",
        "--component",
        action=selection_action.InvalidAction,
        nargs=0,
        dest="selection",
        help="Match tests against their Fuchsia component name",
    )
    selection.add_argument(
        "-a",
        "--and",
        action=selection_action.InvalidAction,
        nargs=0,
        dest="selection",
        help="Add requirements to the preceding filter",
    )
    selection.add_argument(
        "selection",
        action=selection_action.SelectionAction,
        nargs="*",
    )
    selection.add_argument(
        "--fuzzy",
        type=int,
        default=3,
        help="The Damerau-Levenshtein distance threshold for fuzzy matching tests",
    )
    selection.add_argument(
        "--allow-empty-selection",
        action=argparse.BooleanOptionalAction,
        help="If True and multiple selections are provided, allow individual selections that match no tests as long as at least one test is selected. Default is True.",
        default=True,
    )
    selection.add_argument(
        "--show-suggestions",
        action=argparse.BooleanOptionalAction,
        help="If True and no tests match, suggest matching tests from the build directory. Default is True.",
        default=True,
    )
    selection.add_argument(
        "--suggestion-count",
        type=int,
        help="Show this number of suggestions if no tests match. Default is 6.",
        default=6,
    )

    execution = parser.add_argument_group("Execution Options")
    execution.add_argument(
        "--use-package-hash",
        action=argparse.BooleanOptionalAction,
        help="Use the package Merkle root hash from the build artifacts to ensure you are running the most recently built device test code.",
        default=True,
    )
    execution.add_argument(
        "--parallel",
        type=int,
        help="Maximum number of test suites to run in parallel. Does not affect per-suite parallelism.",
        default=4,
    )
    execution.add_argument(
        "--parallel-cases",
        type=int,
        help="Instruct on-device test runners to prefer running this number of cases in parallel.",
        default=0,
    )
    execution.add_argument(
        "-r",
        "--random",
        action="store_true",
        help="Randomize test execution order",
        default=False,
    )
    execution.add_argument(
        "--timeout",
        type=float,
        help="Terminate tests that take longer than this number of seconds to complete. By default, uses test default timeout values. A zero timeout value disables the timeout.",
    )
    execution.add_argument(
        "--timeout-grace-period",
        type=float,
        help="Number of seconds following timeout after which the test process will be killed. Default 15 seconds.",
        default=15.0,
    )
    execution.add_argument(
        "--test-filter",
        type=str,
        action="append",
        default=[],
        help="Run specific test cases in a test suite. Can be specified multiple times to pass in multiple patterns.",
    )
    execution.add_argument(
        "--count",
        type=int,
        help="Execute each test this many times. If any iteration of a test times out, no further iterations will be executed",
        default=1,
    )
    execution.add_argument(
        "--limit",
        type=int,
        help="Stop execution after this many tests",
        default=None,
    )
    execution.add_argument(
        "--offset",
        type=int,
        help="Skip this many tests at the beginning of the test list. Combine with --limit to deterministically select a subrange of tests.",
        default=0,
    )
    execution.add_argument(
        "-f",
        "--fail",
        action="store_true",
        help="Stop running tests after the first failed test suite. This will abort all tests in progress and end with a failure code.",
        default=False,
    )
    execution.add_argument(
        "--fail-by-group",
        action=argparse.BooleanOptionalAction,
        help="When repeating tests with --count, stop repeating if any execution fails. If --no-fail-by-group is sets, continue repeating test executions despite failures to identify flakes.",
        default=True,
    )
    execution.add_argument(
        "--restrict-logs",
        action=argparse.BooleanOptionalAction,
        help="If False, do not limit maximum log severity regardless of the test's configuration. Default is True.",
        default=True,
    )
    execution.add_argument(
        "--min-severity-logs",
        nargs="*",
        help="""Modifies the minimum log severity level emitted by components during the test execution.
        Specify using the format <component-selector>#<log-level>, or just <log-level> (in which
        case the severity will apply to all components under the test, including the test component
        itself) with level as one of FATAL|ERROR|WARN|INFO|DEBUG|TRACE.""",
        default=[],
    )
    execution.add_argument(
        "--also-run-disabled-tests",
        action="store_true",
        help="If True, also run tests that are disabled by the test author. This only affects test components. Default is False.",
        default=False,
    )
    execution.add_argument(
        "--show-full-moniker-in-logs",
        action=argparse.BooleanOptionalAction,
        help="""If set, show the full moniker in log output for on-device tests.
        Otherwise only the last segment of the moniker is displayed.
        Default is False.""",
        default=False,
    )
    execution.add_argument(
        "-e",
        "--env",
        action="append",
        type=str,
        help="Add an environment variable to each test invocation. May be specified multiple times.",
        default=[],
    )
    execution.add_argument(
        "--break-on-failure",
        action=argparse.BooleanOptionalAction,
        help="""If set and supported by the test runner, any test case failures will stop test
        execution. zxdb is automatically launched and attached to the failed test case unless
        `--use-existing-debugger` is set.""",
        default=None,
    )
    execution.add_argument(
        "--breakpoint",
        metavar="BREAKPOINT",  # This is to make the help text singular.
        dest="breakpoints",
        action="append",
        help="""Run the test with zxdb attached and set the given breakpoint. For example,
        `--breakpoint my_source_file.cc:37` will insert a breakpoint at line 37 of any file
        named my_source_file.cc. May be specified multiple times to add multiple breakpoints.""",
        default=[],
    )
    execution.add_argument(
        "--use-existing-debugger",
        action="store_true",
        help="""If set, suppresses the automatic launch and attach of zxdb when `--break-on-failure`
        is set. Incompatible with `--breakpoint`.""",
        default=False,
    )
    execution.add_argument(
        "--enable-debug-adapter",
        help="""If set, spawns zxdb in Debug Adapter mode. This will not spawn a zxdb console
        session. Instead, zxdb will be acting as a Debug Adapter server. This option is incompatible
        with --use-existing-debugger.""",
        action=argparse.BooleanOptionalAction,
        default=None,
    )
    execution.add_argument(
        "--debug-adapter-port",
        type=int,
        help="""Specifies the PORT to use for the Debug Adapter Server. Must be used in conjunction
        with --enable-debug-adapter. An open port is randomly assigned if not specified.""",
        metavar="PORT",
        default=None,
    )
    execution.add_argument(
        "--agent-debugging-mode",
        action="store_true",
        help="""If set, automatically begins an fx debug cli session in the background, which should
        be used by Agents for test debugging workflows. Implies --break-on-failure and
        --enable-debug-adapter.""",
        default=False,
    )
    execution.add_argument(
        "--allow-temporary-package-server",
        action=argparse.BooleanOptionalAction,
        help="Allow this script to start a temporary package server if one is not already running. Default is True.",
        default=True,
    )
    execution.add_argument(
        "--ffx-usb-socket-path",
        help="""If set, explicitly sets the path to the socket used to communicate with the FFX USB
        driver. Otherwise the default is used from your ffx config.""",
        default=None,
    )

    execution.add_argument(
        "--allow-temporary-emulator",
        action=argparse.BooleanOptionalAction,
        help="Allow this script to start a temporary emulator if no active device is detected. Default is True.",
        default=True,
    )

    execution.add_argument(
        "--use-test-pilot",
        action=argparse.BooleanOptionalAction,
        help="""Run test components using test-pilot. Note: this flag is experimental""",
        default=False,
    )

    output = parser.add_argument_group("Output Options")
    output.add_argument(
        "-o",
        "--output",
        action=argparse.BooleanOptionalAction,
        help="Display the output from passing tests. Some test arguments may be needed.",
    )
    output.add_argument(
        "--simple",
        action=argparse.BooleanOptionalAction,
        default=True if agents_lib.is_invoked_by_agent() else False,
        help="Remove any color or decoration from output. Disable pretty status printing. Implies --no-style",
    )
    output.add_argument(
        "--style",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Remove color and decoration from output. Does not disable pretty status printing. Default is to only style for TTY output.",
    )
    output.add_argument(
        "--log",
        action=argparse.BooleanOptionalAction,
        help="Emit command events to a file. Turned on when running real tests unless `--no-log` is passed.",
        default=True,
    )
    output.add_argument(
        "--logpath",
        help="If passed and --log is enabled, customizes the destination of the target log.",
        default=None,
    )
    output.add_argument(
        "--save-log-path-to-file",
        help="Write the path of the log file to this file. Used for scripting so a script will know where the log path is without needing to intercept output.",
        default=None,
    )
    output.add_argument(
        "--json",
        action="store_true",
        help="Format output as JSON logs, alias for --logpath -",
        default=False,
    )
    output.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        default=False,
        help="Print verbose logs to the console",
    )
    output.add_argument(
        "--status",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Toggle interactive status printing to console. Default is to vary behavior depending on if output is to a TTY. Setting to True on a non-TTY is an error.",
    )
    output.add_argument(
        "--status-lines",
        default=8,
        type=int,
        help="Number of lines used to display status output.",
    )
    output.add_argument(
        "--status-delay",
        default=0.033,
        type=float,
        help="Control how frequently the status output is updated. Default is every 0.033s, but you can increase the number for calmer output on slower connections.",
    )
    output.add_argument(
        "--timestamp-artifacts",
        default=False,
        action=argparse.BooleanOptionalAction,
        help="If set, output artifacts in a timestamped directory under the given output directory. Default is False.",
    )
    output.add_argument(
        "--outdir",
        "--ffx-output-directory",  # For compatibility
        "--artifact-output-directory",
        default=None,
        dest="artifact_output_directory",
        help="If set, write test artifact output to this directory for post processing.",
    )
    output.add_argument(
        "-s",
        "--slow",
        type=float,
        default=0,
        help="If non-zero, automatically show output for tests taking longer than this many seconds.",
    )
    output.add_argument(
        "-q",
        "--quiet",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Silence INFO and INSTRUCTION messages from the tool",
    )
    output.add_argument(
        "--replay-speed",
        type=float,
        default=1,
        help="Speed up replays by this amount. Can be less than 1 for slow motion.",
    )
    output.add_argument(
        "--capture-syslog",
        action=argparse.BooleanOptionalAction,
        help="""If set, include captured syslog in output. Set by default.""",
        default=True,
    )
    output.add_argument(
        "--summary-json",
        default=None,
        help="If set, write a structured JSON summary of the test run to this path.",
    )
    output.add_argument(
        "--summary-to-stdout",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="If set, print the structured JSON summary to stdout on completion.",
    )
    output.add_argument(
        "--agent-output",
        action=argparse.BooleanOptionalAction,
        default=True if agents_lib.is_invoked_by_agent() else False,
        help="Format output for AI agents with isolated logs and JSON summary output.",
    )

    if defaults is not None:
        actions = parser._actions.copy()
        groups_to_process: typing.List[
            argparse._ArgumentGroup
        ] = parser._action_groups.copy()

        # Recursively find all actions.
        while groups_to_process:
            group = groups_to_process.pop()
            actions.extend(group._actions)
            groups_to_process.extend(group._action_groups)

        # Apply defaults for all identified actions from the given defaults.
        for action in actions:
            if hasattr(defaults, action.dest):
                action.default = getattr(defaults, action.dest)

    cli_args = selection_action.SelectionAction.preprocess_args(cli_args)
    namespace = parser.parse_intermixed_args(cli_args)
    flags: Flags = Flags(**vars(namespace), extra_args=extra_args)
    return flags
