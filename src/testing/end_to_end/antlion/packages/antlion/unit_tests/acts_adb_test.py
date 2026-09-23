#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import mock
from antlion.controllers import adb
from antlion.controllers.adb_lib.error import AdbCommandError, AdbError


class MockJob(object):
    def __init__(self, exit_status=0, stderr="", stdout=""):
        self.exit_status = exit_status
        self.stderr = stderr
        self.stdout = stdout


class MockAdbProxy(adb.AdbProxy):
    def __init__(self):
        pass


class ADBTest(unittest.TestCase):
    """A class for testing antlion/controllers/adb.py"""

    def test__exec_cmd_failure_old_adb(self):
        mock_job = MockJob(exit_status=1, stderr="error: device not found")
        cmd = ["adb", "-s", '"SOME_SERIAL"', "shell", '"SOME_SHELL_CMD"']
        with mock.patch("antlion.libs.proc.job.run", return_value=mock_job):
            with self.assertRaises(AdbError):
                MockAdbProxy()._exec_cmd(cmd)

    def test__exec_cmd_failure_new_adb(self):
        mock_job = MockJob(
            exit_status=1, stderr="error: device 'DEADBEEF' not found"
        )
        cmd = ["adb", "-s", '"SOME_SERIAL"', "shell", '"SOME_SHELL_CMD"']
        with mock.patch("antlion.libs.proc.job.run", return_value=mock_job):
            with self.assertRaises(AdbError):
                MockAdbProxy()._exec_cmd(cmd)

    def test__exec_cmd_pass_basic(self):
        mock_job = MockJob(exit_status=0, stderr="DEADBEEF", stdout="FEEDACAB")
        cmd = ["adb", "-s", '"SOME_SERIAL"', "shell", '"SOME_SHELL_CMD"']
        with mock.patch("antlion.libs.proc.job.run", return_value=mock_job):
            result = MockAdbProxy()._exec_cmd(cmd)
        self.assertEqual(result, "FEEDACAB")

    def test__exec_cmd_ignore_status(self):
        mock_job = MockJob(exit_status=0, stderr="DEADBEEF", stdout="")
        cmd = ["adb", "-s", '"SOME_SERIAL"', "shell", '"SOME_SHELL_CMD"']
        with mock.patch("antlion.libs.proc.job.run", return_value=mock_job):
            result = MockAdbProxy()._exec_cmd(cmd, ignore_status=True)
        self.assertEqual(result, "DEADBEEF")

    def test__exec_cmd_pass_grep(self):
        mock_job = MockJob(exit_status=1, stderr="", stdout="foo")
        cmd = ["adb", "-s", '"SOME_SERIAL"', "shell", '"grep foo"']
        with mock.patch("antlion.libs.proc.job.run", return_value=mock_job):
            result = MockAdbProxy()._exec_cmd(cmd)
        self.assertEqual(result, "foo")

    def test__exec_cmd_failure_ret_nonzero(self):
        mock_job = MockJob(exit_status=1, stderr="error not related to adb")
        cmd = ["adb", "-s", '"SOME_SERIAL"', "shell", '"SOME_SHELL_CMD"']
        with mock.patch("antlion.libs.proc.job.run", return_value=mock_job):
            with self.assertRaises(AdbCommandError):
                MockAdbProxy()._exec_cmd(cmd)

    def test__exec_cmd_raises_on_bind_error(self):
        """Tests _exec_cmd raises an AdbError on port forwarding failure."""
        mock_job = MockJob(
            exit_status=1,
            stderr="error: cannot bind listener: " "Address already in use",
            stdout="",
        )
        cmd = ["adb", "-s", '"SOME_SERIAL"', "shell", '"SOME_SHELL_CMD"']
        with mock.patch("antlion.libs.proc.job.run", return_value=mock_job):
            with self.assertRaises(AdbError):
                MockAdbProxy()._exec_cmd(cmd)

    def test__get_version_number_gets_version_number(self):
        """Tests the positive case for AdbProxy.get_version_number()."""
        proxy = MockAdbProxy()
        expected_version_number = 39
        proxy.version = lambda: (
            f"Android Debug Bridge version 1.0.{expected_version_number}\nblah"
        )
        self.assertEqual(expected_version_number, proxy.get_version_number())

    def test__get_version_number_raises_upon_parse_failure(self):
        """Tests the failure case for AdbProxy.get_version_number()."""
        proxy = MockAdbProxy()
        proxy.version = lambda: "Bad format"
        with self.assertRaises(AdbError):
            proxy.get_version_number()


if __name__ == "__main__":
    unittest.main()
