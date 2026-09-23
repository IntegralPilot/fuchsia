# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Runs shell commands to enumerate tests or build a GN target.

Results: Returned from polling a command - tells its status and any outputs
destined for the UI.

TargetBuilder and TestEnumerator start running when the instance is created,
and finish running when poll() returns a status of Success or Failure.
poll() also returns any text output produced by the running command, sorted
into status updates, important messages, and errors.

TargetBuilder: Builds a GN target passed to the class initializer.

TestEnumerator: Enumerates all the tests in a given GN target.
    Upon Success, member `tests` holds the enumerated tests.
"""

import select
import subprocess
import sys
import time
from dataclasses import dataclass
from typing import IO, Any

import data_structure
import util
from json_get import JsonGet


@dataclass
class Results:
    """Returned from CommandRunner.poll().

    `status` will be one of Working, Success, Failing, or Failure.
    `outputs` contains new text for the user to see.
    """

    outputs: util.Outputs
    status: util.Status


# TODO(https://fxbug.dev/412727988): unit-test the classes in this file

# Subclasses of CommandRunner are essentially helper classes, but there's enough state
# shared between CommandRunner and its helpers that it's easier to make them the same
# object with a few calling contracts, rather than multiple objects with calling
# contracts _and_ pointers into each other.


class CommandRunner:
    """Sets up and runs a command.

    Must be subclassed. The subclass will prepare for _command() to be called,
    then call __init__() which will start the command running. Then poll() can
    be called by the holder of the instance.
    """

    def __init__(self) -> None:
        """Sets up and runs the command."""

        self.outputs = util.Outputs()
        self.finalized_results: Results | None = None
        self.status = util.Status.WORKING
        self.exit_code: int | None = None
        self.saw_error = False
        self.command = _Command(self._command())

    def poll(self) -> Results:
        """Returns the command's status and outputs."""

        if self.finalized_results:
            return self.finalized_results
        if self.command.is_done():
            return self._finalize()
        out, err = self.command.get_new()
        if out:
            self._handle_outputs(out)
        if err:
            self._handle_errors(err)
        results = Results(outputs=self.outputs, status=self.status)
        self.outputs = util.Outputs()
        return results

    def _command(self) -> str:
        """Returns the command string from the subclass.

        Must be overridden. Subclasses must ensure that any state required by
        their _command() method implementation is initialized prior to calling
        super().__init__().
        """

        return ""

    def _handle_outputs(self, lines: list[str]) -> None:
        """Override to handle stdout lines."""

        for line in lines:
            self.outputs.update(line)

    def _handle_errors(self, lines: list[str]) -> None:
        """Override to handle stderr lines."""

        if lines:
            self.saw_error = True
        for line in lines:
            self.outputs.error(line)

    def _almost_done(self) -> None:
        """Override to do something when the runner is about to finalize."""

    def _should_fail(self) -> bool:
        """Override to compute a different failure condition."""
        return self.saw_error or self.exit_code != 0

    def _finalize(self) -> Results:
        """Wrap up a command and set self.finalized_results.

        Gets and processes all remaining output from the command.

        Only call when the shell command has finished, or it may hang or
        return misleading results.
        """

        self.exit_code = self.command.exit_code()
        out, err = self.command.get_remaining()
        if out:
            self._handle_outputs(out)
        if err:
            self._handle_errors(err)
        if self._should_fail():
            self.status = util.Status.FAILING
        if self.status == util.Status.FAILING:
            self.status = util.Status.FAILURE
        else:
            self.status = util.Status.SUCCESS
        self._almost_done()
        self.finalized_results = Results(
            outputs=util.Outputs(), status=self.status
        )
        r = Results(outputs=self.outputs, status=self.status)
        # Anything written into this object will be ignored
        self.outputs = util.Outputs()
        return r


class TargetBuilder(CommandRunner):
    """Builds a GN target.

    Init with a GN target containing tests, like "//sdk/ctf/release"
    Call poll() (returns Results) until state is "success" or "failure".
    """

    def __init__(self, target: str) -> None:
        self.command_string = f".jiri_root/bin/fx build {target}"
        super().__init__()

    def _command(self) -> str:
        return self.command_string

    def _handle_outputs(self, lines: list[str]) -> None:
        self.outputs.update(lines[-1])


class TestEnumerator(CommandRunner):
    """

    Init with a GN target containing tests, like "//sdk/ctf/release"
    Call poll() (returns Results) until state is "success" or "failure"
    """

    def __init__(self, target: str) -> None:
        self.command_string = (
            f".jiri_root/bin/fx test {target} --list --no-build --logpath -"
        )
        super().__init__()
        self.tests: data_structure.Tests = data_structure.Tests()
        self.outputs.print(
            "Please stand by - enumerating tests (may take 30 seconds)"
        )
        # `fx test` launches subcommands. Some of them produce output aimed at STDERR even though
        # there's no error. We'll ignore that output. Each subcommand has a different "id".
        self.ignore_errors: set[int] = set()
        self.run_count = 0
        self.total_runs = 0

    def _command(self) -> str:
        return self.command_string

    def _almost_done(self) -> None:
        if not self._should_fail():
            self.outputs.update("Done enumerating tests")

    def _handle_outputs(self, lines: list[str]) -> None:
        """Scan the JSON output for useful info."""
        for line in lines:
            if "tests could not be enumerated" in line:
                self._handle_errors(
                    [
                        "Enumeration didn't work. You may need to start emu and/or serve."
                    ]
                )
            info = JsonGet(line)
            self._look_for_test_selections(info)
            self._look_for_program_starts(info)
            self._look_for_executing(info)
            self._look_for_enumeration(info)
            self._look_for_building(info)
            self._look_for_error_output(info)

    def _handle_errors(self, lines: list[str]) -> None:
        """Output useful information based on error line contents."""
        if not lines:
            return
        self.saw_error = True
        for e in lines:
            self.outputs.error(e)
            if (
                "ERROR: Unknown GN label" in e
                or "all_package_manifests.list" in e
            ):
                self.outputs.error("You may need to do an `fx build`.")

    def _look_for_test_selections(self, info: JsonGet) -> None:
        info.match(
            {"payload": {"test_selections": {"selected": Any}}},
            self._selected_tests_callback,
        )

    def _selected_tests_callback(self, data_with_selected: Any) -> None:
        self.total_runs = len(
            data_with_selected.payload.test_selections.selected.keys()
        )

    def _look_for_program_starts(self, info: JsonGet) -> None:
        # {"id": 3, "timestamp": 1280753.01070115, "parent": 2, "starting": true,
        #   "payload": {"program_execution": {"command": "fx",
        #        "flags": ["--dir", "/usr/local/google/home/cphoenix/fuchsia/out/emu", "dldist", "-v",
        #                  "--needle", "//sdk/ctf/tests/pkg/fdio:fdio-spawn-tests-package", "--input",
        #                  "/tmp/tmpp_1dukb1/temp-input.txt", "--match-contains"], "environment": {}}}}
        info.match(
            {
                "id": Any,
                "starting": True,
                "payload": {
                    "program_execution": {"command": Any, "flags": Any}
                },
            },
            self._program_start_callback,
        )

    def _program_start_callback(self, start: Any) -> None:
        if "dldist" in start.payload.program_execution.flags:
            self.ignore_errors.add(start.id)

    def _look_for_executing(self, info: JsonGet) -> None:
        # { "id": 110, "timestamp": 406305.148786878, "parent": 16, "starting": true,
        #   "payload":
        #     {"program_execution":
        #        {"command": "fx",
        #        "flags": ["--dir", "/usr/local/google/home/cphoenix/fuchsia/out/emu",
        #               "ffx", "test", "list-cases",
        #               "fuchsia-pkg://fuchsia.com/fuchsia-element-tests_ctf24?hash=0f997b3e79d591aa729b3fdc448869259a36c9083a82cbb0a5a0e3aa783f16bc#meta/test-root.cm"], "environment": {}}}}
        info.match(
            {
                "starting": True,
                "payload": {"program_execution": {"flags": Any}},
            },
            self._executing_callback,
        )

    def _executing_callback(self, data_with_flags: Any) -> None:
        if not self.total_runs:
            return
        program_name = data_with_flags.payload.program_execution.flags[-1]
        self.run_count += 1
        self.outputs.update(
            f"{int(self.run_count*100/self.total_runs)}%: {program_name.split('?')[0]}"
        )

    def _look_for_error_output(self, info: JsonGet) -> None:
        # {"id": 11, "timestamp": 1268958.160280603, "payload": {"program_output":
        #    {"data": "ERROR: It looks like the package repository server is not running.\n",
        #       "stream": "STDERR", "print_verbatim": false}}}
        info.match(
            {
                "id": Any,
                "payload": {
                    "program_output": {"stream": "STDERR", "data": Any}
                },
            },
            self._error_output_callback,
        )

    def _error_output_callback(self, info: Any) -> None:
        if info.id not in self.ignore_errors:
            self._handle_errors([info.payload.program_output.data])

    def _look_for_building(self, info: JsonGet) -> None:
        # {"id": 8, "timestamp": 1267997.052252219, "starting": true,
        #  "payload": {"build_targets": ["--default", "//sdk/ctf/tests/pkg/fdio:fdio-spawn-tests-package",
        #       "//sdk/ctf/tests/pkg/fdio:fdio-spawn-tests-package"]}}
        info.match(
            {"starting": True, "payload": {"build_targets": Any}},
            self._building_callback,
        )

    def _building_callback(self, info: Any) -> None:
        self.outputs.print(
            f"Building (could take a while): {info.payload.build_targets[1:]}"
        )

    def _look_for_enumeration(self, info: JsonGet) -> None:
        # {"id": 382, "timestamp": 406316.24595807, "payload": {
        #   "enumerate_test_cases": {
        #     "test_name": "fuchsia-pkg://fuchsia.com/media-button-test-suite_ctf26#meta/media-button-conformance-test.cm",
        #     "test_case_names": ["main"]}}}
        info.match(
            {
                "payload": {
                    "enumerate_test_cases": {
                        "test_name": Any,
                        "test_case_names": Any,
                    }
                }
            },
            self._enumeration_callback,
        )

    def _enumeration_callback(self, data_with_enumerate: Any) -> None:
        suite_name = data_with_enumerate.payload.enumerate_test_cases.test_name
        case_names = (
            data_with_enumerate.payload.enumerate_test_cases.test_case_names
        )
        suite = data_structure.TestSuite(name=suite_name)
        for cn in case_names:
            suite.cases.append(data_structure.TestCase(cn))
        self.tests.tests.append(suite)


class _Command:
    """Executes a shell command, and handles output while it's running.

    Can be used to busy-wait-poll; if no new input, it does a brief delay.
    """

    def __init__(self, command: str) -> None:
        words = command.split()
        try:
            self.c = subprocess.Popen(
                words,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                shell=False,
            )
        except FileNotFoundError:
            print(
                "This script can only be run in the root of your source tree."
            )
            sys.exit()

    def _read(self, stream: IO[bytes]) -> list[str]:
        lines = []
        while True:
            read_list, _, _ = select.select(
                [stream], [], [], 0.1
            )  # This makes the readline() non-blocking
            if not read_list:
                break
            line = read_list[0].readline()
            if line == b"":
                break
            else:
                lines.append(str(line, encoding="utf-8"))
        return lines

    def get_new(self, delay: float = 0.005) -> tuple[list[str], list[str]]:
        assert self.c.stdout, "keep MyPy happy - this should never be None"
        new_stdout = self._read(self.c.stdout)
        assert self.c.stderr, "keep MyPy happy - this should never be None"
        new_stderr = self._read(self.c.stderr)
        if not new_stdout and not new_stderr:
            time.sleep(delay)
        return new_stdout, new_stderr

    # If command is still running, this will read until it naturally quits, so it may never return.
    def get_remaining(self) -> tuple[list[str], list[str]]:
        stdout_lines: list[str] = []
        stderr_lines: list[str] = []
        while self.c.poll() is None:
            o, e = self.get_new()
            stdout_lines.extend(o)
            stderr_lines.extend(e)
        while True:
            o, e = self.get_new()
            if not o and not e:
                break
            stdout_lines.extend(o)
            stderr_lines.extend(e)
        return stdout_lines, stderr_lines

    def is_done(self) -> bool:
        return self.c.poll() is not None

    def exit_code(self) -> int | None:
        return self.c.poll()
