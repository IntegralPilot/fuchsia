# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.utils.host_shell.py."""

import subprocess
import unittest
from collections.abc import Callable
from unittest import mock

from honeydew import errors
from honeydew.utils import host_shell
from parameterized import param, parameterized


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_obj: param
) -> str:
    """Custom test name function method."""
    test_func_name: str = testcase_func.__name__
    test_label: str = parameterized.to_safe_name(param_obj.kwargs["label"])
    return f"{test_func_name}_when_{test_label}"


class HostShellTests(unittest.TestCase):
    """Unit tests for honeydew.utils.host_shell.py."""

    @mock.patch.object(
        subprocess,
        "run",
        return_value=subprocess.CompletedProcess(
            args=("ls",),
            returncode=0,
            stdout="",
            stderr=None,
        ),
        autospec=True,
    )
    def test_run(
        self,
        mock_subprocess_run: mock.Mock,
    ) -> None:
        """Test case for host_shell.run()"""
        host_shell.run(
            cmd=[
                "ls",
            ]
        )

        mock_subprocess_run.assert_called_with(
            [
                "ls",
            ],
            stdout=subprocess.PIPE,  # to capture stdout to PIPE
            stderr=subprocess.PIPE,  # to capture stderr to PIPE
            check=True,  # to raise exception if cmd fails
            text=True,  # to decode the output to str
            timeout=None,  # default is None which means no timeout
        )

    @parameterized.expand(
        [
            param(
                label="capture_output_is_True_and_capture_error_in_output_is_False",
                capture_output=True,
                capture_error_in_output=False,
                expected_stdout=subprocess.PIPE,
                expected_stderr=subprocess.PIPE,
            ),
            param(
                label="capture_output_is_True_and_capture_error_in_output_is_True",
                capture_output=True,
                capture_error_in_output=True,
                expected_stdout=subprocess.PIPE,
                expected_stderr=subprocess.STDOUT,
            ),
            param(
                label="capture_output_is_False_and_capture_error_in_output_is_False",
                capture_output=False,
                capture_error_in_output=False,
                expected_stdout=None,
                expected_stderr=None,
            ),
            param(
                label="capture_output_is_False_and_capture_error_in_output_is_True",
                capture_output=False,
                capture_error_in_output=True,
                expected_stdout=subprocess.PIPE,
                expected_stderr=subprocess.STDOUT,
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        subprocess,
        "run",
        return_value=subprocess.CompletedProcess(
            args=("ls",),
            returncode=0,
            stdout="",
            stderr=None,
        ),
        autospec=True,
    )
    def test_run_capturing_studout_and_stderr(
        self,
        mock_subprocess_run: mock.Mock,
        label: str,  # pylint: disable=unused-argument
        capture_output: bool,
        capture_error_in_output: bool,
        expected_stdout: int | None,
        expected_stderr: int | None,
    ) -> None:
        """Test case for host_shell.run()"""
        host_shell.run(
            cmd=[
                "ls",
            ],
            capture_output=capture_output,
            capture_error_in_output=capture_error_in_output,
        )

        mock_subprocess_run.assert_called_with(
            [
                "ls",
            ],
            stdout=expected_stdout,  # whether or not to capture and where to capture stdout
            stderr=expected_stderr,  # whether or not to capture and where to capture stderr
            check=True,  # to raise exception if cmd fails
            text=True,  # to decode the output to str
            timeout=None,  # default is None which means no timeout
        )

    @mock.patch.object(
        subprocess,
        "run",
        side_effect=subprocess.CalledProcessError(
            returncode=5,
            cmd="ls",
            output="output",
        ),
        autospec=True,
    )
    def test_run_raises_host_cmd_error(
        self,
        mock_subprocess_run: mock.Mock,
    ) -> None:
        """Test case for host_shell.run() raising HostCmdError"""
        with self.assertRaises(errors.HostCmdError):
            host_shell.run(
                cmd=[
                    "ls",
                ]
            )

        mock_subprocess_run.assert_called_once()

    @mock.patch.object(
        subprocess,
        "run",
        side_effect=subprocess.TimeoutExpired(cmd="ls", timeout=5),
        autospec=True,
    )
    def test_run_raises_honeydew_timeout_error(
        self,
        mock_subprocess_run: mock.Mock,
    ) -> None:
        """Test case for host_shell.run() raising HoneydewTimeoutError"""
        with self.assertRaises(errors.HoneydewTimeoutError):
            host_shell.run(
                cmd=[
                    "ls",
                ]
            )

        mock_subprocess_run.assert_called_once()

    @mock.patch.object(
        subprocess,
        "Popen",
        autospec=True,
    )
    def test_popen(self, mock_subprocess_popen_call: mock.Mock) -> None:
        """Test case for host_shell.popen()"""
        host_shell.popen(
            cmd=["ls"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        mock_subprocess_popen_call.assert_called_once_with(
            ["ls"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
