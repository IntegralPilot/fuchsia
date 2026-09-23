# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import math
import os
import re
import tempfile
import typing

_MOBLY_CLASS_HEADER_REGEX = re.compile(
    r"^==========> (?P<class_name>.*) <==========$"
)

_MOBLY_TEST_OUTPUT_TEMPLATE = (
    'fx test --e2e {test_name} -- --test_cases="{test_case}"'
)

_PYTHON_HOST_TEST_OUTPUT_TEMPLATE = 'fx test {test_name} -- -k "{test_case}"'

_DEFAULT_TEST_OUTPUT_TEMPLATE = (
    'fx test {test_name} --test-filter "{test_case}"'
)

import async_utils.command as command
import statusinfo

import args
import environment
import event
import package_repository
import test_list_file

# The timeout command-line argument can be used to specify a timeout for all tests that are run
# in a given invocation of 'fx 'test'. The default behavior is to use the 'normal' timeout value
# for each test. A few tests have specific 'normal' timeout values specified using test_spec
# values in build files, but most tests use this default timeout of 5 minutes.
DEFAULT_TIMEOUT = 5 * 60

# Bounded timeout for target device discovery, address resolution, and ssh key lookups.
_DEVICE_PROBE_TIMEOUT_SECONDS: float = 30.0


class TestExecutionError(Exception):
    """Base error type for test failures."""

    def __init__(self, message: str, command_output: typing.Any = None):
        super().__init__(message)
        self.command_output = command_output


class TestCouldNotRun(TestExecutionError):
    """The test could not be run at all."""


class TestSkipped(TestExecutionError):
    """The test was skipped for some non-error reason"""


class TestFailed(TestExecutionError):
    """The test ran, but returned a failure error code."""


class TestTimeout(TestExecutionError):
    """The test timed out."""


# Unique number suffix for output subdirectories.
UNIQUE_OUTPUT_SUFFIX = 0


class TestExecution:
    """Represents a single execution for a specific test."""

    def __init__(
        self,
        test: test_list_file.Test,
        exec_env: environment.ExecutionEnvironment,
        flags: args.Flags,
        run_suffix: int | None = None,
        device_env: environment.DeviceEnvironment | None = None,
    ):
        """Initialize the test execution wrapper.

        Args:
            test (test_list_file.Test): Test to run.
            exec_env (environment.ExecutionEnvironment): Execution environment.
            flags (args.Flags): Command flags.
            run_suffix (int, optional): If set, this is the unique
                index of a single run of the referenced test.
            device_env (environment.DeviceEnvironment, optional):
                If set, this contains information on how to connect to
                a device. Otherwise if this is a host test it may not
                connect to a device.
        """
        global UNIQUE_OUTPUT_SUFFIX
        self._test = test
        self._exec_env = exec_env
        self._flags = flags
        self._run_suffix = run_suffix
        self._device_env = device_env
        if self._flags.artifact_output_directory is not None:
            self._outdir: str | None = os.path.join(
                self._flags.artifact_output_directory, str(UNIQUE_OUTPUT_SUFFIX)
            )
            UNIQUE_OUTPUT_SUFFIX += 1
        else:
            self._outdir = None

    def name(self) -> str:
        """Get the name of the test.

        Returns:
            str: Name of the test.
        """
        return self._test.name() + (
            f" (Run {self._run_suffix})" if self._run_suffix is not None else ""
        )

    def is_hermetic(self) -> bool:
        """Determine if a test is hermetic.

        Returns:
            bool: True if the wrapped test is hermetic, False otherwise.
        """
        return self._test.info.is_hermetic()

    def command_line(self, test_cases: list[str] | None = None) -> list[str]:
        """Format the command line required to execute this test.

        Raises:
            TestCouldNotRun: If we do not know how to run this type of test.

        Returns:
            list[str]: The command line for the test.
        """

        min_severity_logs: list[str] = []

        if self._use_test_pilot() and self._test.info.execution is not None:
            # assert for mypy
            assert self._test.build.test.new_path is not None

            execution = self._test.info.execution

            if self._flags.min_severity_logs:
                min_severity_logs = self._flags.min_severity_logs
            elif execution.min_severity_logs is not None:
                min_severity_logs = [execution.min_severity_logs]

            extra_args = []

            if execution.realm:
                extra_args += ["--realm=" + execution.realm]
            if execution.max_severity_logs and self._flags.restrict_logs:
                extra_args += [
                    "--max-severity-logs=" + execution.max_severity_logs,
                ]
            if min_severity_logs:
                extra_args += [
                    "--min-severity-logs=" + ",".join(min_severity_logs)
                ]

            parallel_cases = self._flags.parallel_cases
            if (
                parallel_cases == 0
                and self._test.build.test.parallel is not None
            ):
                parallel_cases = self._test.build.test.parallel
            if parallel_cases != 0:
                extra_args += [
                    "--max-concurrent-test-case-runs=" + str(parallel_cases),
                ]

            for test_filter in self._flags.test_filter:
                extra_args += ["--test-case-filter=" + test_filter]
            if self._flags.also_run_disabled_tests:
                extra_args += ["--run-disabled-cases"]
            if self._flags.show_full_moniker_in_logs:
                extra_args += ["--show-full-moniker-in-logs"]
            if self._flags.break_on_failure:
                extra_args += ["--break-on-failure"]

            if self._flags.extra_args:
                extra_args += [
                    "--target-test-args=" + ",".join(self._flags.extra_args)
                ]

            timeout = self._calculate_timeout()
            if timeout is not None:
                extra_args += [
                    "--timeout=" + str(math.ceil(timeout)),
                ]

            return [
                os.path.join(
                    self._exec_env.out_dir, self._test.build.test.new_path
                )
            ] + extra_args

        elif self._test.info.execution is not None:
            exec_env = self._exec_env
            execution = self._test.info.execution

            component_url = self._get_component_url()
            assert component_url is not None

            if self._flags.min_severity_logs:
                min_severity_logs = self._flags.min_severity_logs
            elif execution.min_severity_logs is not None:
                min_severity_logs = [execution.min_severity_logs]

            extra_args = []
            if execution.realm:
                extra_args += ["--realm", execution.realm]
            if execution.max_severity_logs and self._flags.restrict_logs:
                extra_args += [
                    "--max-severity-logs",
                    execution.max_severity_logs,
                ]
            if min_severity_logs:
                for min_severity_log in min_severity_logs:
                    extra_args += ["--min-severity-logs", min_severity_log]

            parallel_cases = self._flags.parallel_cases
            if (
                parallel_cases == 0
                and self._test.build.test.parallel is not None
            ):
                parallel_cases = self._test.build.test.parallel
            if parallel_cases != 0:
                extra_args += [
                    "--parallel",
                    str(parallel_cases),
                ]

            if (
                self._test.build.test.create_no_exception_channel is not None
                and self._test.build.test.create_no_exception_channel
            ):
                extra_args += [
                    "--no-exception-channel",
                ]

            # If command line filters are given, they should override (cancel)
            # any filters that may be in test-list.json.
            test_execution_filters = self._test.info.execution.test_filters
            if self._flags.test_filter:
                for test_filter in self._flags.test_filter:
                    extra_args += ["--test-filter", test_filter]
            elif test_execution_filters:
                extra_args.append("--no-cases-equals-success")
                for test_filter in test_execution_filters:
                    extra_args += ["--test-filter", test_filter]
            if self._flags.also_run_disabled_tests:
                extra_args += ["--run-disabled"]
            if self._flags.show_full_moniker_in_logs:
                extra_args += ["--show-full-moniker-in-logs"]
            if self._flags.break_on_failure:
                extra_args += ["--break-on-failure"]
            if self._outdir is not None:
                extra_args += ["--output-directory", self._outdir]
            if self._flags.capture_syslog:
                extra_args += ["--capture-syslog"]

            timeout = self._calculate_timeout()
            if timeout is not None:
                extra_args += [
                    "--timeout",
                    str(math.ceil(timeout)),
                ]

            suffix_args = (
                ["--"] + self._flags.extra_args
                if self._flags.extra_args
                else []
            )

            return (
                exec_env.fx_cmd_line("ffx", "test", "run")
                + extra_args
                + [component_url]
                + suffix_args
            )
        elif self._test.build.test.path:
            command_line = [self._test.build.test.path]
            if self._test.is_e2e_test() and self._device_env is not None:
                if (
                    self._test.build.test.list_cases_argument
                    == "list_mobly_tests"
                ):
                    command_line += [
                        "--ssh-key",
                        self._device_env.private_key_path,
                    ]
            if test_cases:
                command_line += ["--test_cases"] + test_cases
            return command_line + self._flags.extra_args
        else:
            raise TestCouldNotRun(
                f"We do not know how to run this test: {str(self._test)}"
            )

    def enumerate_cases_command_line(self) -> list[str] | None:
        """Get the command line to enumerate all test cases in this test.

        If this type of test does not support test case enumeration,
        return None.

        Returns:
            list[str] | None: Command line to enumerate cases
                if possible, None otherwise.
        """

        # If the test definition includes a `list_cases_argument` (e.g. "list_mobly_tests"),
        # we construct a command to execute the test binary with that argument.
        # This is primarily used for host tests which support listing cases via a flag.
        if (
            self._test.build.test.list_cases_argument
            and self._test.build.test.path
        ):
            arg = f"--{self._test.build.test.list_cases_argument}"
            path = self._test.build.test.path
            if not os.path.isabs(path):
                path = os.path.join(self._exec_env.out_dir, path)
            return [path, arg]

        execution = self._test.info.execution
        exec_env = self._exec_env

        try:
            component_url = self._get_component_url()
        except TestCouldNotRun:
            return None
        if component_url is None or execution is None:
            return None

        extra_args = []
        if execution.realm:
            extra_args += ["--realm", execution.realm]

        return (
            exec_env.fx_cmd_line("ffx", "test", "list-cases")
            + extra_args
            + [component_url]
        )

    def enumerate_cases_output_template(self) -> str:
        """Get the template string for the output command.

        Returns:
            str: The template string
        """
        # TODO(fxbug.dev/481539525): This behavior should ideally be in a script run by fx test
        if self._test.build.test.list_cases_argument == "list_mobly_tests":
            return _MOBLY_TEST_OUTPUT_TEMPLATE
        if (
            self._test.build.test.list_cases_argument
            == "list_host_python_unittests"
        ):
            return _PYTHON_HOST_TEST_OUTPUT_TEMPLATE
        return _DEFAULT_TEST_OUTPUT_TEMPLATE

    async def enumerate_mobly_test(
        self,
        recorder: event.EventRecorder,
        parent: event.Id | None = None,
    ) -> command.CommandOutput | None:
        """Enumerate test cases for this (Mobly) test.

        Args:
            recorder (event.EventRecorder): Recorder for events.
            parent (event.Id | None): Parent event ID.

        Returns:
            command.CommandOutput | None: Command output if successful.
        """
        cmd = self.enumerate_cases_command_line()
        if not cmd:
            return None

        env = self.environment() or {}
        cwd = env.get("CWD")

        # mobly_driver (and potentially others) requires FUCHSIA_TEST_OUTDIR to be set.
        # We create a temporary directory for enumeration outputs.
        with tempfile.TemporaryDirectory() as temp_out_dir:
            # Adjust temp_out_dir if CWD is set, mirroring execution.py logic
            out_dir_val = temp_out_dir
            if cwd and not os.path.isabs(out_dir_val):
                out_dir_val = os.path.relpath(out_dir_val, cwd)

            env["FUCHSIA_TEST_OUTDIR"] = out_dir_val

            output = await run_command(
                *cmd,
                recorder=recorder,
                parent=parent,
                env=env,
                print_verbatim=False,
            )

            if output and output.stdout:
                # Parse Mobly output to extract test cases.
                # Format is:
                # ==========> ClassName <==========
                # test_name_1
                # test_name_2
                lines = output.stdout.splitlines()
                clean_lines = []

                # We discard all output lines until we match a class header.
                # Once a header is found, subsequent lines are treated as test case names
                # belonging to that class, until EOL.
                current_class = None
                for line in lines:
                    line = line.strip()
                    if not line:
                        continue

                    match = _MOBLY_CLASS_HEADER_REGEX.match(line)
                    if match:
                        current_class = match.group("class_name")
                        continue

                    # Capture tests under the current class header
                    if current_class:
                        clean_lines.append(line)

                if clean_lines:
                    output.stdout = "\n".join(clean_lines)

            return output

    async def enumerate_test_cases(
        self,
        recorder: event.EventRecorder,
        parent: event.Id | None = None,
    ) -> command.CommandOutput | None:
        """Enumerate test cases for this test.

        This method dispatches to specific enumeration logic based on test type.

        Args:
            recorder (event.EventRecorder): Recorder for events.
            parent (event.Id | None): Parent event ID.

        Returns:
            command.CommandOutput | None: Command output if successful.
        """
        if self._test.build.test.list_cases_argument == "list_mobly_tests":
            return await self.enumerate_mobly_test(recorder, parent)

        cmd = self.enumerate_cases_command_line()
        if not cmd:
            return None

        # Standard enumeration (e.g. device tests)
        return await run_command(
            *cmd,
            recorder=recorder,
            parent=parent,
            print_verbatim=False,
            env=self.environment(),
        )

    def environment(self) -> dict[str, str] | None:
        """Format environment variables needed to run the test.

        Returns:
            dict[str, str] | None: Environment for
                the test, or None if no environment is needed.
        """
        env = self._flags.computed_env()
        if (
            self._test.build.test.path
            or self._test.is_e2e_test()
            or self._use_test_pilot()
        ):
            env.update(
                {
                    "CWD": self._exec_env.out_dir,
                }
            )
        if self._use_test_pilot():
            if self._device_env is not None:
                # TODO(https://fxbug.dev/550548948): don't assume x64 host
                env.update(
                    {
                        "FUCHSIA_DEVICE_ADDR": self._device_env.address,
                        "FUCHSIA_SSH_KEY": self._device_env.private_key_path,
                        "FUCHSIA_HOST_TOOLS": "./host_x64",
                    }
                )
            if self._flags.extra_args:
                custom_args = " ".join(self._flags.extra_args)
                env.update(
                    {
                        "FUCHSIA_CUSTOM_TEST_ARGS": custom_args,
                    }
                )

        if self._test.is_e2e_test() and self._device_env is not None:
            env.update(
                {
                    "FUCHSIA_DEVICE_ADDR": self._device_env.address,
                    "FUCHSIA_SSH_PORT": self._device_env.port,
                    "FUCHSIA_SSH_KEY": self._device_env.private_key_path,
                    "FUCHSIA_NODENAME": self._device_env.name,
                }
            )

        if self._exec_env.usb_socket_path:
            env.update(
                {
                    "FUCHSIA_TEST_FFX_USB_SOCKET_PATH": self._exec_env.usb_socket_path,
                }
            )

        return None if not env else env

    def should_symbolize(self) -> bool:
        """Determine if we should symbolize the output of this test.

        Returns:
            bool: True if we should run the output through a symbolizer, False otherwise.
        """
        return self._test.info.execution is not None

    async def run(
        self,
        recorder: event.EventRecorder,
        flags: args.Flags,
        parent: event.Id,
        timeout: float | None = None,
        abort_signal: asyncio.Event | None = None,
    ) -> command.CommandOutput:
        """Asynchronously execute this test.

        Args:
            recorder (event.EventRecorder): Recorder for events.
            flags (args.Flags): Command flags to control output.
            parent (event.Id): Parent event to nest the execution under.
            timeout (float, optional): If set, timeout after this number of seconds.
            abort_event (asyncio.Event, optional): If set and signaled, abort this test.

        Raises:
            TestFailed: If the test reported failure.
            TestTimeout: If the test timed out.
            TestSkipped: If the test should not run.

        Returns:
            command.CommandOutput: The output of executing this command.
        """
        if self._test.is_boot_test():
            raise TestSkipped(
                "Boot tests are not supported by `fx test`. Use `fx run-boot-test` or `fx core-tests`."
            )
        if self._test.is_e2e_test() and not flags.e2e:
            raise TestSkipped(
                "Skipping optional end to end test. Pass --e2e to execute this test."
            )
        exec_env = self._exec_env

        symbolizer_args = None
        if self.should_symbolize():
            symbolizer_args = exec_env.fx_cmd_line("ffx", "debug", "symbolize")

        resolved_test_cases = None
        if (
            self._test.build.test.list_cases_argument == "list_mobly_tests"
            and flags.test_filter
        ):
            enumeration_output = await self.enumerate_test_cases(
                recorder, parent
            )
            if enumeration_output and enumeration_output.return_code == 0:
                all_cases = enumeration_output.stdout.splitlines()
                resolved_test_cases = []
                for case in all_cases:
                    for filter_str in flags.test_filter:
                        if filter_str in case:
                            resolved_test_cases.append(case)
                            break
                if not resolved_test_cases:
                    raise TestSkipped(
                        f"No test cases matched filters: {flags.test_filter}"
                    )
            else:
                raise TestCouldNotRun(
                    f"Failed to enumerate test cases for filtering. Output: {enumeration_output.stderr if enumeration_output else 'None'}"
                )

        command = self.command_line(test_cases=resolved_test_cases)
        env = self.environment() or {}

        outdir = self._outdir
        maybe_temp_dir: tempfile.TemporaryDirectory[str] | None = None
        if not outdir:
            maybe_temp_dir = tempfile.TemporaryDirectory()
            outdir = maybe_temp_dir.name
        if not os.path.exists(outdir):
            os.makedirs(outdir)

        # Update the outdir to match the current working directory for the test command (if any).
        # Only do this if the output directory is a relative path, otherwise keep it as is.
        if "CWD" in env and not os.path.isabs(outdir):
            outdir = os.path.relpath(outdir, env["CWD"])

        env.update(
            {
                "FUCHSIA_TEST_OUTDIR": outdir,
            }
        )

        # We add a grace period when timing out in order to give the test itself time to time
        # out first. If it fails to terminate before the grace period expires, we terminate the
        # process.
        timeout = self._calculate_timeout()
        if timeout is not None:
            timeout += self._flags.timeout_grace_period

        try:
            output = await run_command(
                *command,
                recorder=recorder,
                parent=parent,
                print_verbatim=flags.output,
                symbolizer_args=symbolizer_args,
                env=env,
                timeout=timeout,
                abort_signal=abort_signal,
            )
        finally:
            if maybe_temp_dir is not None:
                files: list[str] = []
                for prefix, _, names in os.walk(maybe_temp_dir.name):
                    files.extend(
                        [
                            os.path.relpath(
                                os.path.join(prefix, n), maybe_temp_dir.name
                            )
                            for n in names
                        ]
                    )
                if files:
                    name_list = statusinfo.ellipsize(", ".join(files), 100)
                    recorder.emit_instruction_message(
                        f"Deleting {len(files)} files at {maybe_temp_dir.name}: {name_list}"
                    )
                    recorder.emit_instruction_message(
                        "To keep these files, set --ffx-output-directory."
                    )
                maybe_temp_dir.cleanup()

        if not output:
            raise TestFailed("Failed to run the test command")
        elif output.return_code != 0 or output.was_timeout:
            if not flags.output:
                # If output wasn't streamed live, print the header for the failure.
                recorder.emit_info_message(
                    f"\n{statusinfo.error_highlight(self._test.name(), style=flags.style)}:\n"
                )

            if flags.gemini_analysis is not None:
                error_log_parts = []
                if output.stdout:
                    error_log_parts.append(output.stdout)
                if output.stderr:
                    error_log_parts.append(output.stderr)
                if not output.stdout and not output.stderr:
                    error_log_parts.append("<No command output>")

                error_log = "\n".join(error_log_parts)
                if error_log and not error_log.endswith("\n"):
                    error_log += "\n"

                if error_log:
                    api_key = self._exec_env.gemini_api_key

                    if not api_key:
                        recorder.emit_warning_message(
                            "Gemini analysis skipped: GEMINI_API_KEY environment variable not set."
                        )
                    else:
                        # TODO(https://fxbug.dev/456511584): change gemini_analysis call to use a built binary
                        gemini_tool_path = os.path.join(
                            self._exec_env.fuchsia_dir,
                            "tools/gemini_analysis/gemini_analysis.py",
                        )
                        gemini_output = await run_command(
                            gemini_tool_path,
                            "--api-key",
                            api_key,
                            "--gemini-model",
                            flags.gemini_model,
                            "--verbosity",
                            str(flags.gemini_analysis),
                            recorder=recorder,
                            parent=parent,
                            print_verbatim=True,
                            input_bytes=error_log.encode(),
                        )
                        if not gemini_output or gemini_output.return_code != 0:
                            recorder.emit_warning_message(
                                "Gemini analysis failed."
                            )
                            if gemini_output and gemini_output.stderr:
                                recorder.emit_verbatim_message(
                                    gemini_output.stderr
                                )
            elif not flags.output:
                # gemini is off, give output
                if output.stdout:
                    recorder.emit_verbatim_message(output.stdout)
                if output.stderr:
                    recorder.emit_verbatim_message(output.stderr)
                if not output.stderr and not output.stdout:
                    recorder.emit_verbatim_message("<No command output>")

            if output.was_timeout:
                raise TestTimeout(
                    f"Test exceeded runtime of {timeout} seconds",
                    command_output=output,
                )
            else:
                raise TestFailed("Test reported failure", command_output=output)
        return output

    def _get_component_url(self) -> str | None:
        """Get the final component URL to execute for this test.

        If this test is not a component test, return None.

        Raises:
            TestCouldNotRun: If we cannot determine the merkle hash for this test.

        Returns:
            str | None: The final URL to execute (optionally with
                hash), or None if this is not a component test.
        """
        if self._test.info.execution is None:
            return None

        execution = self._test.info.execution

        component_url = execution.component_url
        if self._flags.use_package_hash:
            try:
                package_repo = (
                    package_repository.PackageRepository.from_env_cached(
                        self._exec_env
                    )
                )
                component_url = package_repo.resolve_component_url(
                    component_url
                )
            except package_repository.PackageRepositoryError as e:
                raise TestCouldNotRun(
                    f"Could not load a Merkle hash for this test ({str(e)}).\nTry running fx build."
                    f"{package_repository.MERKLE_ERROR_HELP_SUFFIX}"
                    if package_repository.MERKLE_ERROR_HELP_SUFFIX not in str(e)
                    else str(e)
                )
        return component_url

    def _use_test_pilot(self) -> bool:
        """Should this test use test_pilot.


        Returns:
            True if experimental flag is enabled and the test supports new
            interface.
        """
        return (
            self._flags.use_test_pilot
            and self._test.build.test.new_path is not None
        )

    def _calculate_timeout(self) -> float | None:
        # Use the config/default timeout if not specified on the command line. If a zero timeout
        # is specified, do not time out.
        if self._flags.timeout is None:
            if self._test.build.test.timeout_secs is not None:
                return self._test.build.test.timeout_secs
            else:
                return DEFAULT_TIMEOUT
        elif self._flags.timeout > 0:
            return self._flags.timeout
        else:
            return None


class DeviceConfigError(Exception):
    """There was an error reading the device configuration"""


async def get_device_environment_from_exec_env(
    exec_env: environment.ExecutionEnvironment,
    recorder: event.EventRecorder | None = None,
) -> environment.DeviceEnvironment:
    # Wait for the target to become ready and reachable.
    # This is more efficient than polling ffx target list in a sleep loop.
    wait_output = await run_command(
        *exec_env.fx_cmd_line("ffx", "target", "wait", "-t", "10"),
        recorder=recorder,
        timeout=_DEVICE_PROBE_TIMEOUT_SECONDS,
    )
    if not wait_output or wait_output.return_code != 0:
        raise DeviceConfigError("Failed to wait for target to become reachable")

    target_output = await run_command(
        *exec_env.fx_cmd_line("ffx", "target", "default", "get"),
        recorder=recorder,
        timeout=_DEVICE_PROBE_TIMEOUT_SECONDS,
    )
    if not target_output or target_output.return_code != 0:
        raise DeviceConfigError(
            "Failed to get the target name. Please ensure you have set a default target using 'fx set-device'. See 'fx set-device --help' for more details on target resolution."
        )
    target_name = target_output.stdout.strip()

    ssh_output = await run_command(
        *exec_env.fx_cmd_line(
            "ffx",
            "target",
            "list",
            "--format",
            "addresses-with-scope",
            target_name,
        ),
        recorder=recorder,
        timeout=_DEVICE_PROBE_TIMEOUT_SECONDS,
    )

    if (
        not ssh_output
        or ssh_output.return_code != 0
        or not ssh_output.stdout.strip()
    ):
        raise DeviceConfigError("Failed to get the ssh address of the target")

    target_line = ssh_output.stdout.strip().splitlines()[-1].strip()
    last_colon_index = target_line.rfind(":")
    if last_colon_index == -1:
        raise DeviceConfigError(
            f"Could not parse target address: {ssh_output.stdout!r}.\n"
            f"Expected 'ip:port' format.\n"
            f"Return code: {ssh_output.return_code},\n"
            f"Stderr: {ssh_output.stderr!r}"
        )
    ip = target_line[0:last_colon_index].strip()
    port = target_line[last_colon_index + 1 :].strip()

    # get the configured private key. Ideally, the private key usage
    # should be an implementation detail internal to ffx commands.
    ssh_key_output = await run_command(
        *exec_env.fx_cmd_line(
            "ffx",
            "config",
            "get",
            "--process",
            "file",
            "ssh.priv",
        ),
        recorder=recorder,
        timeout=_DEVICE_PROBE_TIMEOUT_SECONDS,
    )
    if not ssh_key_output or ssh_key_output.return_code != 0:
        msg = "No return information"
        if ssh_key_output:
            msg = ssh_key_output.stderr
        raise DeviceConfigError(
            f"Failed to get private ssh key: {msg}. Please check your ffx configuration by running 'ffx config get ssh.priv' or 'ffx config env'."
        )
    ssh_path = ssh_key_output.stdout.strip()
    # remove any double quotes around the path
    ssh_path = ssh_path.replace('"', "")

    if not os.path.exists(ssh_path):
        raise DeviceConfigError(
            f"Path returned by 'ssh.priv' does not exist: {ssh_path}. Please check your ffx configuration by running 'ffx config get ssh.priv'. You can also check where config files are located by running 'ffx config env'."
        )

    return environment.DeviceEnvironment(
        address=ip, port=port, name=target_name, private_key_path=ssh_path
    )


async def run_command(
    name: str,
    *args: str,
    recorder: event.EventRecorder | None = None,
    parent: event.Id | None = None,
    print_verbatim: bool = False,
    symbolizer_args: list[str] | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    abort_signal: asyncio.Event | None = None,
    quiet_mode: bool = False,
    input_bytes: bytes | None = None,
    do_not_log_output: bool = False,
) -> command.CommandOutput | None:
    """Utility method to run a test command asynchronously.

    Args:
        name (str): Command to run.
        args (list[str]): Arguments to the command.
        recorder (event.EventRecorder | None):
            Recorder for events. Defaults to None.
        parent (event.Id | None): Parent event ID for reporting.
            Defaults to None.
        print_verbatim (bool, optional): If set, record verbatim
            output events for stdout and stderr. Defaults to False.
        symbolize (bool, optional): If true, pipe output through
            symbolizer. Defaults to False.
        env (dict[str, str], optional):
            Environment to pass to the command. Defaults to None.
        timeout (float, optional): The number of seconds to wait before timing out.
        abort_signal (asyncio.Event, optional): If set, when the
            event is signaled this command will attempt a graceful
            termination.
        quiet_mode (bool, optional): If set, this command will run
            quietly in the background and will not announce cancellation or
            clutter the running task list.
        do_not_log_output (bool, optional): If set, this command will not log its output.
            This is useful for noisy programs whose output may be too much for
            downstream processing.

    Returns:
        command.CommandOutput | None: The command output if it could
            be executed, None otherwise.
    """
    event_id: event.Id | None = None
    abort_task: asyncio.Task[None] | None = None
    if recorder is not None:
        event_id = recorder.emit_program_start(
            name, list(args), env, parent=parent, quiet_mode=quiet_mode
        )
    try:
        started = await command.AsyncCommand.create(
            name,
            *args,
            symbolizer_args=symbolizer_args,
            env=env,
            timeout=timeout,
            input_bytes=input_bytes,
        )

        async def handle_abort() -> None:
            if abort_signal is not None:
                await abort_signal.wait()
                if recorder is not None and not quiet_mode:
                    recorder.emit_info_message(f"Aborting {name}...")
                started.terminate()
                await asyncio.sleep(5)
                if recorder is not None:
                    recorder.emit_warning_message(
                        f"{name} did not terminate in time, killing it"
                    )
                started.kill()

        abort_task = asyncio.Task(handle_abort())

        def handle_event(current_event: command.CommandEvent) -> None:
            if recorder is not None:
                assert event_id is not None
                if (
                    isinstance(current_event, command.StdoutEvent)
                    and not do_not_log_output
                ):
                    recorder.emit_program_output(
                        event_id,
                        current_event.text.decode(errors="replace"),
                        stream=event.ProgramOutputStream.STDOUT,
                        print_verbatim=print_verbatim,
                    )
                if (
                    isinstance(current_event, command.StderrEvent)
                    and not do_not_log_output
                ):
                    recorder.emit_program_output(
                        event_id,
                        current_event.text.decode(errors="replace"),
                        stream=event.ProgramOutputStream.STDERR,
                        print_verbatim=print_verbatim,
                    )
                if isinstance(current_event, command.TerminationEvent):
                    recorder.emit_program_termination(
                        event_id, current_event.return_code
                    )

        ret = await started.run_to_completion(callback=handle_event)
        return ret
    except command.AsyncCommandError as e:
        if recorder is not None:
            assert event_id is not None
            recorder.emit_program_termination(event_id, -1, error=str(e))
        return None
    finally:
        if abort_task is not None:
            abort_task.cancel()
