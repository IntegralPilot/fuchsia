# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.utils.common.py."""

import time
import unittest
from collections.abc import Callable
from typing import Any
from unittest import mock

from honeydew import errors
from honeydew.utils import common
from parameterized import param, parameterized

CONFIG_DICT: dict[str, Any] = {
    "affordances": {
        "bluetooth": {
            "implementation": "fuchsia-controller",
        },
    },
    "transports": {
        "ffx": {
            "ssh_keepalive_timeout": 123,
        },
    },
    "supported": {
        "features": [
            "trace",
            "inspect",
            "snapshot",
        ]
    },
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom test name function method."""
    test_func_name: str = testcase_func.__name__
    test_label: str = parameterized.to_safe_name(param_arg.kwargs["label"])

    return f"{test_func_name}_with_{test_label}"


class CommonUtilsTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.utils.common.py."""

    async def test_wait_for_state_success(self) -> None:
        """Test case for common.wait_for_state() success case."""
        await common.wait_for_state(
            state_fn=lambda: True, expected_state=True, timeout=5
        )

    @mock.patch("asyncio.sleep", autospec=True)
    @mock.patch("time.time", side_effect=[0, 1, 2, 3, 4, 5], autospec=True)
    async def test_wait_for_state_fail(
        self, mock_time: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Test case for common.wait_for_state() failure case where state_fn
        never returns the expected state."""
        with self.assertRaises(errors.HoneydewTimeoutError):
            await common.wait_for_state(
                state_fn=lambda: True, expected_state=False, timeout=5
            )

        mock_time.assert_called()
        mock_sleep.assert_called()

    @mock.patch("asyncio.sleep", autospec=True)
    @mock.patch("time.time", side_effect=[0, 1, 2, 3, 4, 5], autospec=True)
    async def test_wait_for_state_fail_2(
        self, mock_time: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Test case for common.wait_for_state() failure case where state_fn
        keeps raising exception."""

        def _state_fn() -> bool:
            raise RuntimeError("Error")

        with self.assertRaises(errors.HoneydewTimeoutError):
            await common.wait_for_state(
                state_fn=_state_fn, expected_state=False, timeout=5
            )

        mock_time.assert_called()
        mock_sleep.assert_called()

    async def test_retry_success_with_no_ret_val(self) -> None:
        """Test case for common.retry() success case where fn() does not return
        anything."""

        def _fn() -> None:
            return

        await common.retry(fn=_fn, timeout=60, wait_time=5)

    async def test_retry_success_with_ret_val(self) -> None:
        """Test case for common.retry() success case where fn() returns an
        object."""

        def _fn() -> str:
            return "some_string"

        self.assertEqual(
            await common.retry(fn=_fn, timeout=60, wait_time=5), "some_string"
        )

    @mock.patch("asyncio.sleep", autospec=True)
    @mock.patch(
        "time.time",
        side_effect=[0, 5, 10, 15, 20, 25, 30, 35],
        autospec=True,
    )
    async def test_retry_fail(
        self, mock_time: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Test case for common.retry() failure case where fn never succeeds."""

        def _fn() -> None:
            raise RuntimeError("Error")

        with self.assertRaises(errors.HoneydewTimeoutError):
            await common.retry(
                fn=_fn,
                timeout=30,
                wait_time=5,
            )

        mock_time.assert_called()
        mock_sleep.assert_called()

    @parameterized.expand(
        [
            param(
                label="returning_dict",
                dictionary=CONFIG_DICT,
                key_path=(
                    "affordances",
                    "bluetooth",
                ),
                should_exist=True,
                expected_value={
                    "implementation": "fuchsia-controller",
                },
            ),
            param(
                label="returning_str",
                dictionary=CONFIG_DICT,
                key_path=(
                    "affordances",
                    "bluetooth",
                    "implementation",
                ),
                should_exist=True,
                expected_value="fuchsia-controller",
            ),
            param(
                label="returning_list",
                dictionary=CONFIG_DICT,
                key_path=(
                    "supported",
                    "features",
                ),
                should_exist=True,
                expected_value=[
                    "trace",
                    "inspect",
                    "snapshot",
                ],
            ),
            param(
                label="invalid_path_without_should_exist",
                dictionary=CONFIG_DICT,
                key_path=(
                    "affordances",
                    "invalid",
                    "implementation",
                ),
                should_exist=False,
                expected_value=None,
            ),
        ],
        name_func=_custom_test_name_func,
    )
    def test_read_from_dict(
        self,
        label: str,  # pylint: disable=unused-argument,
        dictionary: dict[str, Any],
        key_path: tuple[str, ...],
        should_exist: bool,
        expected_value: Any,
    ) -> None:
        """Test case for common.read_from_dict() static method."""
        self.assertEqual(
            common.read_from_dict(
                dictionary,
                key_path,
                should_exist,
            ),
            expected_value,
        )

    def test_read_from_dict_exception(self) -> None:
        """Test case for common.read_from_dict() static method raising exception."""
        with self.assertRaisesRegex(
            errors.ConfigError,
            r"'affordances', 'invalid'.*does not exist in the config dict passed during init",
        ):
            common.read_from_dict(
                d=CONFIG_DICT,
                key_path=(
                    "affordances",
                    "invalid",
                    "implementation",
                ),
                should_exist=True,
            )

    def test_time_limit(self) -> None:
        """Test case for common.time_limit() where the underlying method finishes successfully"""
        with common.time_limit(timeout=1):
            print("hello world!")

    def test_time_limit_error_1(self) -> None:
        """Test case for common.time_limit() where the underlying method raises an exception"""
        with self.assertRaises(RuntimeError):
            with common.time_limit(timeout=1):
                raise RuntimeError("run time error")

    # This test will take at-least 1 sec to execute, not ideal for an unit test.
    def test_time_limit_error_2(self) -> None:
        """Test case for common.time_limit() where the underlying method times out"""
        with self.assertRaises(errors.HoneydewTimeoutError):
            with common.time_limit(timeout=1):
                time.sleep(2)
