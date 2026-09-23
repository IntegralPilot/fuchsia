# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import io
import os
import signal
import subprocess
import typing
import unittest
import unittest.mock as mock

import debugger
from test_list_file import Test
import tests_json_file


class TestDebuggerTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        # Setup a mock for the subprocess.
        self.subprocess_mock_ = mock.MagicMock(return_value=mock.MagicMock())
        patch = mock.patch("debugger.subprocess.Popen", self.subprocess_mock_)
        patch.start()
        self.addCleanup(patch.stop)

        # Also need to mock out open since we're not actually creating a fifo.
        open_patch = mock.patch("builtins.open", mock.mock_open())
        open_mock = open_patch.start()
        open_mock.return_value = io.StringIO()
        self.addCleanup(open_patch.stop)

        self.fifo_path_ = ""

        def set_fifo_path(*args: typing.Any, **kwargs: typing.Any) -> None:
            self.fifo_path_ = args[0]

        # Replace mkfifo with nothing but our side effect. We just care about the path that was
        # generated
        mkfifo_patch = mock.patch(
            "debugger.os.mkfifo", side_effect=set_fifo_path
        )
        mkfifo_patch.start()
        self.addCleanup(mkfifo_patch.stop)

        # Mock out os.tcsetpgrp since we won't have a tty in tests.
        tcsetpgrp_patch = mock.patch("debugger.os.tcsetpgrp")
        self.tcsetpgrp_mock_ = tcsetpgrp_patch.start()
        self.addCleanup(tcsetpgrp_patch.stop)

        return super().setUp()

    async def test_break_on_failure(self) -> None:
        """Tests zxdb command generation with no breakpoints and break-on-failure is set."""

        # Don't need to do any waiting to check the arguments are correct.
        async def callback() -> None:
            pass

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        debugger.spawn([test], callback, None, True, False, None, [])

        # Should have immediately called the mock, since debugger.spawn is synchronous.
        self.assertTrue(self.subprocess_mock_.called)

        expected_args = [
            "fx",
            "ffx",
            "debug",
            "connect",
            "--new-agent",
            "--",
            "--execute",
            f"attach --weak --recursive {package_name}",
            "--console-mode",
            "embedded",
            "--embedded-mode-context",
            "test failure",
            "--stream-file",
            f"{self.fifo_path_}",
            "--signal-when-ready",
            str(os.getpid()),
        ]

        self.subprocess_mock_.assert_called_with(
            args=expected_args,
            preexec_fn=os.setpgrp,
            stderr=subprocess.STDOUT,
        )

    async def test_break_on_failure_multiple_packages(self) -> None:
        """Tests zxdb command generation when there are multiple packages selected for test."""

        # Don't need to do any waiting to check the arguments are correct.
        async def callback() -> None:
            pass

        tests = []
        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        package_name2 = "fuchsia-pkg://fuchsia.com/bar_test#meta/bar_test.cm"
        tests.append(
            Test(
                build=tests_json_file.TestEntry(
                    test=tests_json_file.TestSection(package_name, "", ""),
                ),
            )
        )
        tests.append(
            Test(
                build=tests_json_file.TestEntry(
                    test=tests_json_file.TestSection(package_name2, "", ""),
                ),
            )
        )

        debugger.spawn(tests, callback, None, True, False, None, [])

        # Should have immediately called the mock, since debugger.spawn is synchronous.
        self.assertTrue(self.subprocess_mock_.called)

        expected_args = [
            "fx",
            "ffx",
            "debug",
            "connect",
            "--new-agent",
            "--",
            "--execute",
            f"attach --weak --recursive {package_name}",
            "--execute",
            f"attach --weak --recursive {package_name2}",
            "--console-mode",
            "embedded",
            "--embedded-mode-context",
            "test failure",
            "--stream-file",
            f"{self.fifo_path_}",
            "--signal-when-ready",
            str(os.getpid()),
        ]

        self.subprocess_mock_.assert_called_with(
            args=expected_args,
            preexec_fn=os.setpgrp,
            stderr=subprocess.STDOUT,
        )

    async def test_explicit_breakpoints_no_break_on_failure(self) -> None:
        """Tests zxdb command generation when the user specifies breakpoints but not
        break-on-failure."""

        # Don't need to do any waiting to check the arguments are correct.
        async def callback() -> None:
            pass

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        debugger.spawn(
            [test], callback, None, False, False, None, ["myfile.rs:1234"]
        )

        # Should have immediately called the mock, since debugger.spawn is synchronous.
        self.assertTrue(self.subprocess_mock_.called)

        # No embedded mode context will be given if break_on_failure is not specified.
        expected_args = [
            "fx",
            "ffx",
            "debug",
            "connect",
            "--new-agent",
            "--",
            "--execute",
            # Always strong attach when an explicit breakpoint is given. All attaches should be
            # recursive.
            f"attach --recursive {package_name}",
            "--console-mode",
            "embedded",
            "--stream-file",
            f"{self.fifo_path_}",
            "--signal-when-ready",
            str(os.getpid()),
            # Breakpoints come last.
            "--execute",
            "break myfile.rs:1234",
        ]

        self.subprocess_mock_.assert_called_with(
            args=expected_args,
            preexec_fn=os.setpgrp,
            stderr=subprocess.STDOUT,
        )

    async def test_explicit_breakpoints_with_break_on_failure(self) -> None:
        """Tests zxdb command generation when the user specifies breakpoints and
        break-on-failure."""

        # Don't need to do any waiting to check the arguments are correct.
        async def callback() -> None:
            pass

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        debugger.spawn(
            [test], callback, None, True, False, None, ["myfile.rs:1234"]
        )

        # Should have immediately called the mock, since debugger.spawn is synchronous.
        self.assertTrue(self.subprocess_mock_.called)

        # Note now the embedded mode context is present because break_on_failure is true, despite
        # the presence of user requested breakpoints.
        expected_args = [
            "fx",
            "ffx",
            "debug",
            "connect",
            "--new-agent",
            "--",
            "--execute",
            # Always strong attach when an explicit breakpoint is given. All attaches should be
            # recursive.
            f"attach --recursive {package_name}",
            "--console-mode",
            "embedded",
            "--embedded-mode-context",
            "test failure",
            "--stream-file",
            f"{self.fifo_path_}",
            "--signal-when-ready",
            str(os.getpid()),
            # Breakpoints come last.
            "--execute",
            "break myfile.rs:1234",
        ]

        self.subprocess_mock_.assert_called_with(
            args=expected_args,
            preexec_fn=os.setpgrp,
            stderr=subprocess.STDOUT,
        )

    async def test_callback_when_ready(self) -> None:
        """Tests that the callback given to spawn is called when zxdb signals that it is ready."""
        condvar = asyncio.Condition()

        async def condvar_notify() -> None:
            async with condvar:
                condvar.notify_all()

        # This mock will get called as a callback.
        mock_callback = mock.MagicMock(side_effect=condvar_notify)

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        debugger.spawn([test], mock_callback, None, True, False, None, [])

        # Simulate the debugger sending sigusr1, there is no subprocess so our pid is the pid
        # listening. This kicks off the task that will notify the condition variable so that we may
        # proceed below.
        await asyncio.sleep(0.2)
        os.kill(os.getpid(), signal.SIGUSR1)

        async with condvar:
            await condvar.wait()

            mock_callback.assert_called_once()

    @mock.patch("debugger.portpicker.pick_unused_port")
    async def test_enable_debug_adapter_default(
        self, mock_pick_unused_port: mock.MagicMock
    ) -> None:
        """Tests that --enable-debug-adapter argument default does not specify a port on zxdb's
        command line"""
        mock_pick_unused_port.return_value = 5678

        # Don't need to do any waiting to check the arguments are correct.
        async def callback() -> None:
            pass

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        debugger.spawn(
            [test],
            callback,
            break_on_failure=True,
            enable_debug_adapter=True,
        )

        # Should have immediately called the mock, since debugger.spawn is synchronous.
        self.assertTrue(self.subprocess_mock_.called)

        expected_args = [
            "fx",
            "ffx",
            "debug",
            "connect",
            "--new-agent",
            "--",
            "--execute",
            f"attach --weak --recursive {package_name}",
            "--console-mode",
            "embedded",
            "--embedded-mode-context",
            "test failure",
            "--stream-file",
            f"{self.fifo_path_}",
            "--signal-when-ready",
            str(os.getpid()),
            "--enable-debug-adapter",
            "--debug-adapter-port",
            "5678",
        ]

        self.subprocess_mock_.assert_called_with(
            args=expected_args,
            preexec_fn=os.setpgrp,
            stderr=subprocess.STDOUT,
        )

    async def test_enable_debug_adapter_with_port(self) -> None:
        """Tests that --enable-debug-adapter argument passes port number to zxdb's command line"""

        # Don't need to do any waiting to check the arguments are correct.
        async def callback() -> None:
            pass

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        debugger.spawn(
            [test],
            callback,
            break_on_failure=True,
            enable_debug_adapter=True,
            debug_adapter_port=1234,
        )

        # Should have immediately called the mock, since debugger.spawn is synchronous.
        self.assertTrue(self.subprocess_mock_.called)

        # No embedded mode context will be given if break_on_failure is not specified.
        expected_args = [
            "fx",
            "ffx",
            "debug",
            "connect",
            "--new-agent",
            "--",
            "--execute",
            f"attach --weak --recursive {package_name}",
            "--console-mode",
            "embedded",
            "--embedded-mode-context",
            "test failure",
            "--stream-file",
            f"{self.fifo_path_}",
            "--signal-when-ready",
            str(os.getpid()),
            "--enable-debug-adapter",
            "--debug-adapter-port",
            "1234",
        ]

        self.subprocess_mock_.assert_called_with(
            args=expected_args,
            preexec_fn=os.setpgrp,
            stderr=subprocess.STDOUT,
        )

    @mock.patch("debugger._start_zxdb_daemon")
    async def test_enable_debug_adapter_sequences_daemon_startup(
        self, mock_start_daemon: mock.AsyncMock
    ) -> None:
        """Tests that when enable_debug_adapter is True, _start_zxdb_daemon is awaited before the ready callback."""
        condvar = asyncio.Condition()
        callback_called = False
        daemon_started = False
        daemon_completed = False

        mock_proc = mock.MagicMock()
        mock_manager = mock.MagicMock()
        mock_manager._proc = mock_proc

        async def mock_start(
            *args: typing.Any, **kwargs: typing.Any
        ) -> mock.MagicMock:
            nonlocal daemon_started, daemon_completed
            daemon_started = True
            await asyncio.sleep(0.1)  # Simulate async work
            daemon_completed = True
            return mock_manager

        mock_start_daemon.side_effect = mock_start

        async def mock_on_debugger_ready() -> None:
            nonlocal callback_called
            # Verify that the daemon startup has fully completed before ready callback is called!
            self.assertTrue(daemon_started)
            self.assertTrue(daemon_completed)
            callback_called = True
            async with condvar:
                condvar.notify_all()

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        debugger.spawn(
            [test],
            mock_on_debugger_ready,
            recorder=None,
            break_on_failure=True,
            enable_debug_adapter=True,
            debug_adapter_port=1234,
        )

        # Ensure signal handler is cleaned up
        self.addCleanup(
            lambda: asyncio.get_event_loop().remove_signal_handler(
                signal.SIGUSR1
            )
        )

        # Simulate SIGUSR1
        await asyncio.sleep(0.2)
        os.kill(os.getpid(), signal.SIGUSR1)

        async with condvar:
            await condvar.wait()
            self.assertTrue(callback_called)

    @mock.patch("debugger.DaemonManager")
    async def test_start_zxdb_daemon(
        self,
        mock_daemon_manager_class: mock.Mock,
    ) -> None:
        """Tests that _start_zxdb_daemon correctly starts the daemon."""
        mock_manager_instance = mock_daemon_manager_class.return_value
        mock_process = mock.MagicMock()
        mock_manager_instance.start = mock.AsyncMock(return_value=mock_process)
        mock_manager_instance._proc = mock_process

        manager = await debugger._start_zxdb_daemon(None, 1234)

        self.assertEqual(manager, mock_manager_instance)
        self.assertEqual(manager._proc, mock_process)
        mock_daemon_manager_class.assert_called_once_with(
            port=1234, connect_to_existing=True
        )
        mock_manager_instance.start.assert_called_once()

    @mock.patch("debugger.DaemonManager")
    async def test_start_zxdb_daemon_eof_fails(
        self,
        mock_daemon_manager_class: mock.Mock,
    ) -> None:
        """Tests that _start_zxdb_daemon fails if the daemon manager fails to start."""
        mock_manager_instance = mock_daemon_manager_class.return_value
        mock_manager_instance.start = mock.AsyncMock(
            side_effect=Exception("connection failed")
        )

        with self.assertRaises(RuntimeError) as context:
            await debugger._start_zxdb_daemon(None, 1234)

        self.assertIn("Failed to start zxdb-daemon", str(context.exception))
        mock_daemon_manager_class.assert_called_once_with(
            port=1234, connect_to_existing=True
        )
        mock_manager_instance.start.assert_called_once()

    async def test_spawn_non_tty_stdin_does_not_crash(self) -> None:
        """Test that debugger.spawn succeeds and does not raise OSError when stdin is not a tty or tcsetpgrp fails."""
        self.tcsetpgrp_mock_.side_effect = OSError(
            25, "Inappropriate ioctl for device"
        )

        async def callback() -> None:
            pass

        package_name = "fuchsia-pkg://fuchsia.com/foo_test#meta/foo_test.cm"
        test = Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(package_name, "", ""),
            ),
        )

        # Should not raise OSError
        debugger.spawn([test], callback, None, True, False, None, [])
