#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import functools
import os
import signal
import subprocess
import unittest
from unittest import mock

import signal_utils


class SignalUtilsTest(unittest.TestCase):
    @mock.patch.object(os, "kill")
    @mock.patch.object(os, "killpg")
    @mock.patch.object(os, "getpgid")
    @mock.patch.object(signal, "signal")
    def test_wait_and_forward_signals_leader(
        self,
        mock_signal: mock.Mock,
        mock_getpgid: mock.Mock,
        mock_killpg: mock.Mock,
        mock_kill: mock.Mock,
    ) -> None:
        """Verify that signals are forwarded to the PGID if the child is a group leader."""
        mock_process = mock.Mock()
        mock_process.pid = 1234
        mock_process.returncode = 0
        mock_getpgid.return_value = 1234  # Same as PID, so it's a leader

        # Setup handlers to trigger when wait() is called.
        def side_effect() -> int:
            # Trigger the SIGINT handler.
            for call in mock_signal.call_args_list:
                if call.args[0] == signal.SIGINT:
                    handler = call.args[1]
                    handler(signal.SIGINT, None)
            return 0

        mock_process.wait.side_effect = side_effect

        with self.assertRaises(signal_utils.BuildInterruptedError) as cm:
            signal_utils._wait_and_forward_signals(mock_process)

        self.assertEqual(cm.exception.return_code, 0)
        self.assertEqual(cm.exception.signum, signal.SIGINT)

        mock_killpg.assert_called_once_with(1234, signal.SIGINT)
        mock_kill.assert_not_called()

    @mock.patch.object(os, "kill")
    @mock.patch.object(os, "killpg")
    @mock.patch.object(os, "getpgid")
    @mock.patch.object(signal, "signal")
    def test_wait_and_forward_signals_follower(
        self,
        mock_signal: mock.Mock,
        mock_getpgid: mock.Mock,
        mock_killpg: mock.Mock,
        mock_kill: mock.Mock,
    ) -> None:
        """Verify that signals are forwarded to the PID only if the child is not a group leader."""
        mock_process = mock.Mock()
        mock_process.pid = 1234
        mock_process.returncode = 0
        mock_getpgid.return_value = (
            1111  # Different from PID, so it's a follower
        )

        def side_effect() -> int:
            for call in mock_signal.call_args_list:
                if call.args[0] == signal.SIGINT:
                    handler = call.args[1]
                    handler(signal.SIGINT, None)
            return 0

        mock_process.wait.side_effect = side_effect

        with self.assertRaises(signal_utils.BuildInterruptedError) as cm:
            signal_utils._wait_and_forward_signals(mock_process)

        self.assertEqual(cm.exception.return_code, 0)
        mock_kill.assert_called_once_with(1234, signal.SIGINT)
        mock_killpg.assert_not_called()

    @mock.patch.object(os, "kill")
    @mock.patch.object(os, "killpg")
    @mock.patch.object(os, "getpgid")
    @mock.patch.object(signal, "signal")
    def test_wait_and_forward_signals_failure_preserved(
        self,
        mock_signal: mock.Mock,
        mock_getpgid: mock.Mock,
        mock_killpg: mock.Mock,
        mock_kill: mock.Mock,
    ) -> None:
        """Verify that child failure code is preserved even when interrupted."""
        mock_process = mock.Mock()
        mock_process.pid = 1234
        mock_process.returncode = 1  # Child failed with 1
        mock_getpgid.return_value = 1234

        def side_effect() -> int:
            for call in mock_signal.call_args_list:
                if call.args[0] == signal.SIGTERM:
                    handler = call.args[1]
                    handler(signal.SIGTERM, None)
            return 1

        mock_process.wait.side_effect = side_effect

        with self.assertRaises(signal_utils.BuildInterruptedError) as cm:
            signal_utils._wait_and_forward_signals(mock_process)

        self.assertEqual(cm.exception.return_code, 1)
        self.assertEqual(cm.exception.signum, signal.SIGTERM)

    @mock.patch.object(os, "kill")
    @mock.patch.object(os, "killpg")
    @mock.patch.object(os, "getpgid")
    @mock.patch.object(signal, "signal")
    def test_wait_and_forward_signals_negative_rc_conversion(
        self,
        mock_signal: mock.Mock,
        mock_getpgid: mock.Mock,
        mock_killpg: mock.Mock,
        mock_kill: mock.Mock,
    ) -> None:
        """Verify that negative signal status is converted to shell-style (128+N)."""
        mock_process = mock.Mock()
        mock_process.pid = 1234
        # subprocess returns -15 for SIGTERM
        mock_process.returncode = -15
        mock_getpgid.return_value = 1234

        def side_effect() -> int:
            for call in mock_signal.call_args_list:
                if call.args[0] == signal.SIGTERM:
                    handler = call.args[1]
                    handler(signal.SIGTERM, None)
            return -15

        mock_process.wait.side_effect = side_effect

        with self.assertRaises(signal_utils.BuildInterruptedError) as cm:
            signal_utils._wait_and_forward_signals(mock_process)

        # Since child died of SIGTERM (-15), status should be 143.
        self.assertEqual(cm.exception.return_code, 143)
        # The last received signal was SIGTERM.
        self.assertEqual(cm.exception.signum, signal.SIGTERM)

    @mock.patch.object(os, "kill")
    @mock.patch.object(os, "killpg")
    @mock.patch.object(os, "getpgid")
    @mock.patch.object(signal, "signal")
    def test_wait_and_forward_signals_multiple_succession(
        self,
        mock_signal: mock.Mock,
        mock_getpgid: mock.Mock,
        mock_killpg: mock.Mock,
        mock_kill: mock.Mock,
    ) -> None:
        """Verify that multiple different signals are all forwarded correctly."""
        mock_process = mock.Mock()
        mock_process.pid = 1234
        mock_process.returncode = 0
        mock_getpgid.return_value = 1234

        def side_effect() -> int:
            # Simulate receiving INT then TERM
            handlers = {
                call.args[0]: call.args[1]
                for call in mock_signal.call_args_list
            }
            handlers[signal.SIGINT](signal.SIGINT, None)
            handlers[signal.SIGTERM](signal.SIGTERM, None)
            return 0

        mock_process.wait.side_effect = side_effect

        with self.assertRaises(signal_utils.BuildInterruptedError) as cm:
            signal_utils._wait_and_forward_signals(mock_process)

        self.assertEqual(cm.exception.signum, signal.SIGTERM)
        self.assertEqual(mock_killpg.call_count, 2)

    @mock.patch.object(os, "setpgrp")
    @mock.patch.object(signal, "signal")
    def test_preexec_setup(
        self, mock_signal: mock.Mock, mock_setpgrp: mock.Mock
    ) -> None:
        """Verify that _preexec_setup resets signals and optionally sets pgrp."""
        # Test with separate_pgrp=True
        signal_utils._preexec_setup(separate_pgrp=True)
        mock_setpgrp.assert_called_once()

        # Verify that expected signals were reset to SIG_DFL
        expected_signals = [signal.SIGINT, signal.SIGHUP, signal.SIGTERM]
        if hasattr(signal, "SIGQUIT"):
            expected_signals.append(signal.SIGQUIT)

        actual_signals_reset = [
            call.args[0] for call in mock_signal.call_args_list
        ]
        for sig in expected_signals:
            self.assertIn(sig, actual_signals_reset)
            # Find the call for this signal and verify it used SIG_DFL
            for call in mock_signal.call_args_list:
                if call.args[0] == sig:
                    self.assertEqual(call.args[1], signal.SIG_DFL)

        mock_signal.reset_mock()
        mock_setpgrp.reset_mock()

        # Test with separate_pgrp=False
        signal_utils._preexec_setup(separate_pgrp=False)
        mock_setpgrp.assert_not_called()
        self.assertGreater(mock_signal.call_count, 0)

    @mock.patch("signal_utils.subprocess.Popen")
    @mock.patch("signal_utils._wait_and_forward_signals")
    def test_signal_managed_process(
        self, mock_wait: mock.Mock, mock_popen: mock.Mock
    ) -> None:
        """Verify SignalManagedProcess correctly orchestrates execution."""
        mock_process = mock.Mock()
        mock_popen.return_value = mock_process
        mock_wait.return_value = 42

        managed = signal_utils.SignalManagedProcess(
            ["test", "cmd"], separate_pgrp=True, verbose=True, env={"A": "B"}
        )
        rc = managed.run()

        self.assertEqual(rc, 42)
        mock_popen.assert_called_once()
        args, kwargs = mock_popen.call_args
        self.assertEqual(args[0], ["test", "cmd"])
        self.assertEqual(kwargs["env"], {"A": "B"})
        self.assertIsInstance(kwargs["preexec_fn"], functools.partial)
        self.assertEqual(kwargs["preexec_fn"].func, signal_utils._preexec_setup)
        self.assertEqual(kwargs["preexec_fn"].args, (True,))

        mock_wait.assert_called_once_with(mock_process, verbose=True)

    @mock.patch("signal_utils.SignalManagedProcess")
    def test_run_command(self, mock_managed_class: mock.Mock) -> None:
        """Verify run_command convenience function."""
        mock_instance = mock_managed_class.return_value
        mock_instance.run.return_value = 123

        rc = signal_utils.run_command(
            ["cmd"], separate_pgrp=False, verbose=True
        )

        self.assertEqual(rc, 123)
        mock_managed_class.assert_called_once_with(
            ["cmd"], separate_pgrp=False, verbose=True
        )
        mock_instance.run.assert_called_once()

    def test_wait_and_forward_signals_timeout(self) -> None:
        """Verify timeout_callback is invoked when wait times out."""
        mock_process = mock.Mock()
        call_count = 0

        def fake_wait(timeout: float | None = None) -> int:
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                raise subprocess.TimeoutExpired(cmd="test", timeout=0.01)
            return 0

        mock_process.wait.side_effect = fake_wait
        callback_called: list[float] = []

        rc = signal_utils._wait_and_forward_signals(
            mock_process,
            timeout=0.01,
            timeout_callback=lambda p, elapsed: callback_called.append(elapsed),
        )
        self.assertEqual(rc, 0)
        self.assertEqual(len(callback_called), 1)

    @mock.patch("signal_utils._wait_and_forward_signals")
    @mock.patch.object(subprocess, "Popen")
    def test_signal_managed_process_with_timeout_and_post_spawn(
        self, mock_popen: mock.Mock, mock_wait: mock.Mock
    ) -> None:
        """Verify SignalManagedProcess passes timeout, timeout_callback, and invokes post_spawn_callback."""
        mock_process = mock.Mock()
        mock_popen.return_value = mock_process
        mock_wait.return_value = 0

        post_spawn_called = []
        timeout_cb = lambda p, e: None

        managed = signal_utils.SignalManagedProcess(
            ["cmd"],
            timeout=60.0,
            timeout_callback=timeout_cb,
            post_spawn_callback=lambda p: post_spawn_called.append(p),
        )
        rc = managed.run()

        self.assertEqual(rc, 0)
        self.assertEqual(post_spawn_called, [mock_process])
        mock_wait.assert_called_once_with(
            mock_process,
            verbose=False,
            timeout=60.0,
            timeout_callback=timeout_cb,
            initial_timeout=None,
        )

    def test_wait_and_forward_signals_initial_timeout(self) -> None:
        """Verify initial_timeout is used for the first tick."""
        mock_process = mock.Mock()
        call_count = 0

        def fake_wait(timeout: float | None = None) -> int:
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                # First wait should use initial_timeout (0.01)
                self.assertAlmostEqual(timeout or 0.0, 0.01, places=2)
                raise subprocess.TimeoutExpired(cmd="test", timeout=0.01)
            return 0

        mock_process.wait.side_effect = fake_wait
        callback_called: list[float] = []

        rc = signal_utils._wait_and_forward_signals(
            mock_process,
            timeout=60.0,
            initial_timeout=0.01,
            timeout_callback=lambda p, elapsed: callback_called.append(elapsed),
        )
        self.assertEqual(rc, 0)
        self.assertEqual(len(callback_called), 1)

    def test_wait_and_forward_signals_initial_timeout_larger_than_timeout(
        self,
    ) -> None:
        """Verify initial_timeout is not clamped when larger than timeout."""
        mock_process = mock.Mock()
        call_count = 0

        def fake_wait(timeout: float | None = None) -> int:
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                # First wait should use initial_timeout (0.05) rather than timeout (0.01)
                self.assertAlmostEqual(timeout or 0.0, 0.05, places=2)
                raise subprocess.TimeoutExpired(cmd="test", timeout=0.05)
            return 0

        mock_process.wait.side_effect = fake_wait
        callback_called: list[float] = []

        rc = signal_utils._wait_and_forward_signals(
            mock_process,
            timeout=0.01,
            initial_timeout=0.05,
            timeout_callback=lambda p, elapsed: callback_called.append(elapsed),
        )
        self.assertEqual(rc, 0)
        self.assertEqual(len(callback_called), 1)

    def test_wait_and_forward_signals_only_initial_timeout(self) -> None:
        """Verify that when only initial_timeout is provided, subsequent waits do not time out."""
        mock_process = mock.Mock()
        call_count = 0

        def fake_wait(timeout: float | None = None) -> int:
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                self.assertAlmostEqual(timeout or 0.0, 0.01, places=2)
                raise subprocess.TimeoutExpired(cmd="test", timeout=0.01)
            if call_count == 2:
                self.assertEqual(timeout, float("inf"))
            return 0

        mock_process.wait.side_effect = fake_wait
        callback_called: list[float] = []

        rc = signal_utils._wait_and_forward_signals(
            mock_process,
            initial_timeout=0.01,
            timeout_callback=lambda p, elapsed: callback_called.append(elapsed),
        )
        self.assertEqual(rc, 0)
        self.assertEqual(len(callback_called), 1)

    @mock.patch("signal_utils._wait_and_forward_signals")
    @mock.patch.object(subprocess, "Popen")
    def test_signal_managed_process_with_initial_timeout(
        self, mock_popen: mock.Mock, mock_wait: mock.Mock
    ) -> None:
        """Verify SignalManagedProcess passes initial_timeout to _wait_and_forward_signals."""
        mock_process = mock.Mock()
        mock_popen.return_value = mock_process
        mock_wait.return_value = 0

        timeout_cb = lambda p, e: None

        managed = signal_utils.SignalManagedProcess(
            ["cmd"],
            timeout=60.0,
            initial_timeout=2.0,
            timeout_callback=timeout_cb,
        )
        rc = managed.run()

        self.assertEqual(rc, 0)
        mock_wait.assert_called_once_with(
            mock_process,
            verbose=False,
            timeout=60.0,
            timeout_callback=timeout_cb,
            initial_timeout=2.0,
        )


if __name__ == "__main__":
    unittest.main()
