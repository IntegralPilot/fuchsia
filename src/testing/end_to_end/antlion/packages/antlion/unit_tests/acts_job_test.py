#!/usr/bin/env python3

# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import unittest

import mock
from libs.proc import job
from libs.proc.runner import CalledProcessError

if os.name == "posix" and sys.version_info[0] < 3:
    import subprocess32 as subprocess
else:
    import subprocess


class FakePopen(object):
    """A fake version of the object returned from subprocess.Popen()."""

    def __init__(
        self, stdout=None, stderr=None, returncode=0, will_timeout=False
    ):
        self.returncode = returncode
        self._stdout = bytes(stdout, "utf-8") if stdout is not None else bytes()
        self._stderr = bytes(stderr, "utf-8") if stderr is not None else bytes()
        self._will_timeout = will_timeout

    def communicate(self, timeout=None):
        if self._will_timeout:
            raise subprocess.TimeoutExpired(
                -1, "Timed out according to test logic"
            )
        return self._stdout, self._stderr

    def kill(self):
        pass

    def wait(self):
        pass


class JobTestCases(unittest.TestCase):
    @mock.patch(
        "libs.proc.job.subprocess.Popen",
        return_value=FakePopen(stdout="TEST\n"),
    )
    def test_run_success(self, popen):
        """Test running a simple shell command."""
        result = job.run("echo TEST")
        self.assertTrue(result.stdout.startswith("TEST"))

    @mock.patch(
        "libs.proc.job.subprocess.Popen",
        return_value=FakePopen(stderr="TEST\n"),
    )
    def test_run_stderr(self, popen):
        """Test that we can read process stderr."""
        result = job.run("echo TEST 1>&2")
        self.assertEqual(len(result.stdout), 0)
        self.assertTrue(result.stderr.startswith("TEST"))
        self.assertFalse(result.stdout)

    @mock.patch(
        "libs.proc.job.subprocess.Popen",
        return_value=FakePopen(returncode=1),
    )
    def test_run_error(self, popen):
        """Test that we raise on non-zero exit statuses."""
        self.assertRaises(CalledProcessError, job.run, "exit 1")

    @mock.patch(
        "libs.proc.job.subprocess.Popen",
        return_value=FakePopen(returncode=1),
    )
    def test_run_with_ignored_error(self, popen):
        """Test that we can ignore exit status on request."""
        result = job.run("exit 1", ignore_status=True)
        self.assertEqual(result.exit_status, 1)

    @mock.patch(
        "libs.proc.job.subprocess.Popen",
        return_value=FakePopen(will_timeout=True),
    )
    def test_run_timeout(self, popen):
        """Test that we correctly implement command timeouts."""
        self.assertRaises(
            CalledProcessError, job.run, "sleep 5", timeout_sec=0.1
        )

    @mock.patch(
        "libs.proc.job.subprocess.Popen",
        return_value=FakePopen(stdout="TEST\n"),
    )
    def test_run_no_shell(self, popen):
        """Test that we handle running without a wrapping shell."""
        result = job.run(["echo", "TEST"])
        self.assertTrue(result.stdout.startswith("TEST"))

    @mock.patch(
        "libs.proc.job.subprocess.Popen",
        return_value=FakePopen(stdout="TEST\n"),
    )
    def test_job_env(self, popen):
        """Test that we can set environment variables correctly."""
        test_env = {"MYTESTVAR": "20"}
        result = job.run("printenv", env=test_env.copy())
        popen.assert_called_once()
        _, kwargs = popen.call_args
        self.assertTrue("env" in kwargs)
        self.assertEqual(kwargs["env"], test_env)


if __name__ == "__main__":
    unittest.main()
