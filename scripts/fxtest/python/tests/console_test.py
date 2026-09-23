# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import contextlib
from dataclasses import dataclass
from dataclasses import field
import io
import unittest
import unittest.mock as mock

from parameterized import parameterized

import args
import console
import event


@dataclass
class ConsoleTestArgs:
    name: str
    args: list[str] = field(default_factory=list)
    expected_present: list[str] = field(default_factory=list)
    expected_absent: list[str] = field(default_factory=list)
    sleep_length: float = 0

    def __str__(self) -> str:
        return self.name


class TestConsole(unittest.IsolatedAsyncioTestCase):
    @parameterized.expand(
        [
            ConsoleTestArgs(
                "test basic output with --status",
                ["--status"],
                expected_present=["Status"],
                expected_absent=["This is output from program"],
            ),
            ConsoleTestArgs(
                "test output is printed with --slow when timeout reached",
                ["--status", "--slow", "1"],
                expected_present=[
                    "Status",
                    "This is output from program",
                    "Runtime has exceeded 1.0 seconds",
                ],
                sleep_length=1.1,
            ),
            ConsoleTestArgs(
                "test output is not printed with --slow when timeout not reached",
                ["--status", "--slow", "1000"],
                expected_present=[
                    "Status",
                ],
                expected_absent=[
                    "This is output from program",
                    "Runtime has exceeded 1000.0 seconds",
                ],
                sleep_length=1,
            ),
            ConsoleTestArgs(
                "background task status is not printed",
                ["--status"],
                expected_present=[],
                expected_absent=[
                    "background_task",
                ],
            ),
        ]
    )
    @mock.patch("console.termout.is_valid", return_value=True)
    @mock.patch(
        "console.statusinfo.shutil.get_terminal_size",
        return_value=mock.MagicMock(columns=80),
    )
    async def test_console(
        self,
        test_args: ConsoleTestArgs,
        _is_valid_mock: mock.Mock,
        _terminal_size_mock: mock.Mock,
    ) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            recorder = event.EventRecorder()
            default_flags = args.parse_args(test_args.args)
            status_event = asyncio.Event()
            printer_task = asyncio.create_task(
                console.ConsoleOutput().console_printer(
                    recorder, default_flags, status_event
                )
            )
            status_event.set()

            recorder.emit_init()
            build_id = recorder.emit_build_start([])
            recorder.emit_end(id=build_id)
            recorder.emit_info_message("A message")
            test_group_id = recorder.emit_test_group(3)
            suite_ids = []
            suite_ids.append(
                recorder.emit_test_suite_started(
                    "foo", hermetic=True, parent=test_group_id
                )
            )
            suite_ids.append(
                recorder.emit_test_suite_started(
                    "bar", hermetic=True, parent=test_group_id
                )
            )
            suite_ids.append(
                recorder.emit_test_suite_started(
                    "baz", hermetic=False, parent=test_group_id
                )
            )
            background_task_ids = []
            background_task_ids.append(
                recorder.emit_program_start(
                    "background_task", [], quiet_mode=True
                )
            )

            before = output.getvalue()
            while output.getvalue() == before:
                # Wait until the output refreshes.
                await asyncio.sleep(0.1)

            for id in suite_ids:
                pid = recorder.emit_program_start("foo", [], {}, parent=id)
                recorder.emit_program_output(
                    pid,
                    f"This is output from program {id}",
                    event.ProgramOutputStream.STDOUT,
                )
                recorder.emit_program_termination(pid, return_code=0)
            for id in background_task_ids:
                recorder.emit_program_output(
                    id,
                    f"This is output from bg task {id}",
                    event.ProgramOutputStream.STDOUT,
                )
                recorder.emit_program_termination(id, return_code=0)

            before = output.getvalue()
            while output.getvalue() == before:
                # Wait until the output refreshes.
                await asyncio.sleep(0.1)

            if test_args.sleep_length:
                await asyncio.sleep(test_args.sleep_length)

            for id in suite_ids:
                recorder.emit_test_suite_ended(
                    id, event.TestSuiteStatus.PASSED, "Passed"
                )

            recorder.emit_end(id=test_group_id)
            recorder.emit_end()

            await printer_task

        for expected_string in test_args.expected_present:
            self.assertTrue(
                expected_string in output.getvalue(),
                f"Expected to find '{expected_string}' in output",
            )
        for not_expected in test_args.expected_absent:
            self.assertFalse(
                not_expected in output.getvalue(),
                f"Expected to not find '{not_expected}' in output",
            )

    @mock.patch("console.termout.is_valid", return_value=True)
    @mock.patch(
        "console.statusinfo.shutil.get_terminal_size",
        return_value=mock.MagicMock(columns=80),
    )
    async def test_build_output_status_line(
        self,
        _terminal_size_mock: mock.Mock,
        _is_valid_mock: mock.Mock,
    ) -> None:
        """Test that build output is shown as a status line and not printed verbatim."""
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            recorder = event.EventRecorder()
            default_flags = args.parse_args(["--status"])
            status_event = asyncio.Event()
            printer_task = asyncio.create_task(
                console.ConsoleOutput().console_printer(
                    recorder, default_flags, status_event
                )
            )
            status_event.set()

            recorder.emit_init()
            build_id = recorder.emit_build_start(["//src:foo"])
            pid = recorder.emit_program_start(
                "fx", ["build", "//src:foo"], parent=build_id, quiet_mode=True
            )

            # Emit some build output
            recorder.emit_program_output(
                pid,
                "[1/10] Compiling foo.cc\n[2/10] Compiling bar.cc\n",
                stream=event.ProgramOutputStream.STDOUT,
            )

            # Wait for output to refresh
            await asyncio.sleep(0.1)

            # Check that the last line of build output appeared in the status output
            self.assertIn("[2/10] Compiling bar.cc", output.getvalue())

            # Emit another update
            recorder.emit_program_output(
                pid,
                "[3/10] Linking foo\n",
                stream=event.ProgramOutputStream.STDOUT,
            )

            await asyncio.sleep(0.1)
            self.assertIn("[3/10] Linking foo", output.getvalue())

            recorder.emit_program_termination(pid, return_code=0)
            recorder.emit_end(id=build_id)
            recorder.emit_end()
            await printer_task
