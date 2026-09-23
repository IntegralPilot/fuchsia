# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import multiprocessing
import os
import signal
import stat
import tempfile
import unittest

import async_utils.signals
from async_utils import command


class TestCommand(unittest.IsolatedAsyncioTestCase):
    def assertStdout(self, event: command.CommandEvent, line: bytes) -> None:
        """Helper to assert on contents of a StdoutEvent.

        Args:
            event (command.CommandEvent): Event to cast and compare.
            line (bytes): Expected line value.
        """
        self.assertTrue(isinstance(event, command.StdoutEvent))
        assert isinstance(event, command.StdoutEvent)
        e: command.StdoutEvent = event
        self.assertEqual(e.text, line)

    def assertStderr(self, event: command.CommandEvent, line: bytes) -> None:
        """Helper to assert on contents of a StderrEvent.

        Args:
            event (command.CommandEvent): Event to cast and compare.
            line (bytes): Expected line value.
        """
        self.assertTrue(isinstance(event, command.StderrEvent))
        assert isinstance(event, command.StderrEvent)
        e: command.StderrEvent = event
        self.assertEqual(e.text, line)

    def assertTermination(
        self, event: command.CommandEvent, return_code: int
    ) -> None:
        """Helper to assert on contents of a TerminationEvent.

        Args:
            event (command.CommandEvent): Event to cast and compare.
            return_code (int): Expected return code.
        """
        self.assertTrue(isinstance(event, command.TerminationEvent))
        assert isinstance(event, command.TerminationEvent)
        e: command.TerminationEvent = event
        self.assertEqual(e.return_code, return_code)

    async def test_basic_command(self) -> None:
        """Test running a basic command and getting the output.

        We create a file in a temporary directory and simply assert that `ls`
        prints that file as output.
        """
        with tempfile.TemporaryDirectory() as td:
            with open(os.path.join(td, "temp-file.txt"), "w") as f:
                f.write("hello world")

            cmd = await command.AsyncCommand.create("ls", ".", env={"CWD": td})
            events = []
            complete = await cmd.run_to_completion(
                lambda event: events.append(event)
            )
            self.assertEqual(len(events), 2, f"Events was actually {events}")

            self.assertStdout(events[0], b"temp-file.txt\n")
            self.assertTermination(events[1], 0)

            self.assertEqual(complete.stdout, "temp-file.txt\n")
            self.assertEqual(complete.return_code, 0)

    async def test_command_with_input(self) -> None:
        """Test passing input to a command."""
        cmd = await command.AsyncCommand.create(
            "cat", input_bytes=b"hello\nworld"
        )
        events = []
        complete = await cmd.run_to_completion(
            lambda event: events.append(event)
        )
        self.assertEqual(len(events), 3, f"Events was actually {events}")

        self.assertStdout(events[0], b"hello\n")
        self.assertStdout(events[1], b"world")
        self.assertTermination(events[2], 0)

        self.assertEqual(complete.stdout, "hello\nworld")
        self.assertEqual(complete.return_code, 0)

    async def test_wrapper_with_input(self) -> None:
        """Test passing input to a command that uses a symbolizer"""
        cmd = await command.AsyncCommand.create(
            "cat",
            symbolizer_args=["sed", "s/hello/goodbye/g"],
            input_bytes=b"hello\nworld",
        )
        events = []
        complete = await cmd.run_to_completion(
            lambda event: events.append(event)
        )
        self.assertEqual(len(events), 3, f"Events was actually {events}")

        self.assertStdout(events[0], b"goodbye\n")
        self.assertStdout(events[1], b"world")
        self.assertTermination(events[2], 0)

        self.assertEqual(complete.stdout, "goodbye\nworld")
        self.assertEqual(complete.return_code, 0)

    async def test_long_line_output(self) -> None:
        """Test processing a very large output from a program"""
        cmd = await command.AsyncCommand.create(
            "cat",
            input_bytes=b"a" * 1024 * 1024,
        )
        events = []
        complete = await cmd.run_to_completion(
            lambda event: events.append(event)
        )
        self.assertEqual(len(events), 2, f"Events was actually {events}")

        self.assertStdout(events[0], b"a" * 1024 * 1024)
        self.assertTermination(events[1], 0)

        self.assertEqual(complete.stdout, "a" * 1024 * 1024)
        self.assertEqual(complete.return_code, 0)

    async def test_basic_command_with_long_timeout(self) -> None:
        """Test running a basic command and getting the output.

        We create a file in a temporary directory and simply assert that `ls`
        prints that file as output.
        """
        with tempfile.TemporaryDirectory() as td:
            with open(os.path.join(td, "temp-file.txt"), "w") as f:
                f.write("hello world")

            cmd = await command.AsyncCommand.create(
                "ls", ".", env={"CWD": td}, timeout=3600
            )
            events = []
            complete = await cmd.run_to_completion(
                lambda event: events.append(event)
            )
            self.assertEqual(len(events), 2, f"Events was actually {events}")

            self.assertStdout(events[0], b"temp-file.txt\n")
            self.assertTermination(events[1], 0)

            self.assertEqual(complete.stdout, "temp-file.txt\n")
            self.assertEqual(complete.return_code, 0)
            self.assertFalse(complete.was_timeout)

    async def test_with_stderr(self) -> None:
        """Test running a command with stderr output.

        We create a temporary directory and try to `ls` a file we know does not
        exist. `ls` should print to stderr and report an error return code.
        """
        with tempfile.TemporaryDirectory() as td:
            cmd = await command.AsyncCommand.create(
                "ls", os.path.join(td, "does-not-exist")
            )
            complete = await cmd.run_to_completion()
            self.assertEqual(complete.stdout, "")
            self.assertNotEqual(complete.stderr, "")
            self.assertNotEqual(complete.return_code, 0)

    async def test_symbolized_command(self) -> None:
        """Test piping output through another program.

        We run `ls` as in the above tests, but this time we pipe the output
        through `sed` to change the word "temp" to "temporary" and assert on
        the new output.
        """
        with tempfile.TemporaryDirectory() as td:
            with open(os.path.join(td, "temp-file.txt"), "w") as f:
                f.write("hello world")

            cmd = await command.AsyncCommand.create(
                "ls",
                ".",
                env={"CWD": td},
                symbolizer_args=["sed", "s/temp/temporary/g"],
            )
            events = []
            await cmd.run_to_completion(lambda event: events.append(event))
            self.assertEqual(len(events), 2, f"Events was actually {events}")

            self.assertStdout(events[0], b"temporary-file.txt\n")
            self.assertTermination(events[1], 0)

    async def test_terminate_and_kill(self) -> None:
        """Test that we can terminate and kill programs.

        We spawn `sleep` to run for over a day, then terminate it. We expect
        the return code to be set by the OS to represent that the program
        was killed.
        """
        cmd = await command.AsyncCommand.create("sleep", "100000")
        task = asyncio.create_task(cmd.run_to_completion())
        cmd.terminate()
        out: command.CommandOutput = await task
        self.assertEqual(out.return_code, -15)

        cmd = await command.AsyncCommand.create("sleep", "100000")
        task = asyncio.create_task(cmd.run_to_completion())
        cmd.kill()
        out = await task
        self.assertEqual(out.return_code, -9)

        cmd = await command.AsyncCommand.create(
            "sleep", "100000", symbolizer_args=["sleep", "100000"]
        )
        task = asyncio.create_task(cmd.run_to_completion())
        cmd.terminate()
        out = await task
        self.assertEqual(out.return_code, -15)
        self.assertEqual(out.wrapper_return_code, -15)

        cmd = await command.AsyncCommand.create(
            "sleep", "100000", symbolizer_args=["sleep", "100000"]
        )
        task = asyncio.create_task(cmd.run_to_completion())
        cmd.kill()
        out = await task
        self.assertEqual(out.return_code, -9)
        self.assertEqual(out.wrapper_return_code, -9)

    async def test_kill_process_groups(self) -> None:
        """Test that terminating a program kills the entire process group."""

        BASH_SHORT = "#!/usr/bin/env bash\nsleep .1\necho 'OK'"
        BASH_LONG = "#!/usr/bin/env bash\nsleep 100000\necho 'OK'"
        with tempfile.TemporaryDirectory() as td:
            # Create scripts and make them executable.
            paths = [
                os.path.join(td, "short.sh"),
                os.path.join(td, "long.sh"),
            ]
            short_path, long_path = paths
            with open(short_path, "w") as f:
                f.write(BASH_SHORT)
            with open(long_path, "w") as f:
                f.write(BASH_LONG)
            for name in paths:
                st = os.stat(name)
                os.chmod(
                    name,
                    st.st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH,
                )

            # Make sure we can run the commands to start with, and they do not hang.
            cmd = await command.AsyncCommand.create(
                short_path,
                env={"CWD": td},
                symbolizer_args=[short_path],
            )
            events = []
            await cmd.run_to_completion(lambda event: events.append(event))
            self.assertEqual(len(events), 2, f"Events was actually {events}")

            self.assertStdout(events[0], b"OK\n")
            # This causes spurious failures on Mac for some reason, where
            # the return value is sometimes -13.
            # self.assertTermination(events[1], 0)

            # Run the long-running shell script, and ensure terminating it does not hang.
            cmd = await command.AsyncCommand.create(
                long_path,
                env={"CWD": td},
                symbolizer_args=[long_path],
            )
            await asyncio.sleep(0.001)
            cmd.terminate()
            events = []
            await cmd.run_to_completion(lambda event: events.append(event))
            self.assertEqual(len(events), 1, f"Events was actually {events}")
            self.assertTermination(events[0], -15)

            # Run again, this time using SIGKILL.
            cmd = await command.AsyncCommand.create(
                long_path,
                env={"CWD": td},
                symbolizer_args=[long_path],
            )
            await asyncio.sleep(0.001)
            cmd.kill()
            events = []
            await cmd.run_to_completion(lambda event: events.append(event))
            self.assertEqual(len(events), 1, f"Events was actually {events}")
            self.assertTermination(events[0], -9)

    async def test_timeout(self) -> None:
        """Test that commands timeout"""
        cmd = await command.AsyncCommand.create("sleep", "100000", timeout=0.1)
        task = asyncio.create_task(cmd.run_to_completion())
        out: command.CommandOutput = await task
        self.assertEqual(out.return_code, -15)
        self.assertTrue(out.was_timeout)

    async def test_invalid_program(self) -> None:
        """Test running a program that doesn't exist, and expect an error."""
        with self.assertRaises(command.AsyncCommandError):
            await command.AsyncCommand.create("..........")

    async def test_stdin_pipe_none_when_no_input(self) -> None:
        """Test that stdin is not piped when input_bytes is not provided."""
        cmd = await command.AsyncCommand.create("echo", "test")
        self.assertIsNone(cmd._process.stdin)
        await cmd.run_to_completion()

    async def test_env_dict_not_mutated(self) -> None:
        """Test that caller-provided env dict is not mutated in place."""
        with tempfile.TemporaryDirectory() as td:
            original_env = {"CWD": td, "CUSTOM_VAR": "VALUE"}
            passed_env = original_env.copy()
            cmd = await command.AsyncCommand.create("ls", ".", env=passed_env)
            await cmd.run_to_completion()
            self.assertEqual(passed_env, original_env)

    async def test_signal_on_already_exited_process(self) -> None:
        """Test that sending signals to an already-exited process does not raise."""
        cmd = await command.AsyncCommand.create("true")
        await cmd.run_to_completion()
        # Should not raise ProcessLookupError
        cmd.terminate()
        cmd.kill()
        cmd.send_signal(signal.SIGTERM)


class TestSignals(unittest.TestCase):
    def test_async_signal_handler(self) -> None:
        """Test that registered signal handlers work appropriately."""

        multiprocessing.set_start_method("fork", force=True)

        output_directory = tempfile.TemporaryDirectory()
        self.addCleanup(output_directory.cleanup)
        output_file_name = os.path.join(output_directory.name, "output.txt")

        def main(output_file_name: str) -> None:
            async def internal_main() -> None:
                os.kill(os.getpid(), signal.SIGTERM)
                await asyncio.sleep(120)

            asyncio.set_event_loop(asyncio.new_event_loop())
            loop = asyncio.get_event_loop()

            fut = asyncio.ensure_future(internal_main())

            def write_output() -> None:
                with open(output_file_name, "a") as f:
                    f.write("Handler printed message\n")
                fut.cancel()

            async_utils.signals.register_on_terminate_signal(write_output)
            try:
                loop.run_until_complete(fut)
            except asyncio.CancelledError:
                with open(output_file_name, "a") as f:
                    f.write("Cancelled\n")

        proc = multiprocessing.Process(target=main, args=(output_file_name,))
        proc.start()
        proc.join()

        lines: list[str]
        with open(output_file_name, "r") as f:
            lines = [line.strip() for line in f.readlines()]

        self.assertListEqual(lines, ["Handler printed message", "Cancelled"])


if __name__ == "__main__":
    unittest.main()
