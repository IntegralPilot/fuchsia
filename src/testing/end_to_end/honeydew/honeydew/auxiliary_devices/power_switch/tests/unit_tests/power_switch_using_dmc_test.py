# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for power_switch_using_dmc.py."""

import os
import unittest
from collections.abc import Callable
from typing import Any
from unittest import mock

from honeydew import errors
from honeydew.auxiliary_devices.power_switch import (
    power_switch,
    power_switch_using_dmc,
)
from honeydew.utils import host_shell
from parameterized import param, parameterized

_MOCK_OS_ENVIRON: dict[str, str] = {"DMC_PATH": "/tmp/foo/bar"}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = testcase_func.__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_with_{test_label}"


class PowerSwitchUsingDmcTests(unittest.TestCase):
    """Unit tests for power_switch_using_dmc.py."""

    def setUp(self) -> None:
        super().setUp()

        with mock.patch.dict(os.environ, _MOCK_OS_ENVIRON, clear=True):
            self.power_switch_using_dmc_obj: (
                power_switch_using_dmc.PowerSwitchUsingDmc
            ) = power_switch_using_dmc.PowerSwitchUsingDmc(device_name="fx-emu")

    def test_instantiate_power_switch_using_dmc_when_dmc_path_not_set(
        self,
    ) -> None:
        """Test case to make sure creating PowerSwitchUsingDmc when DMC_PATH not set
        will result in a failure."""
        with self.assertRaisesRegex(
            power_switch_using_dmc.PowerSwitchDmcError,
            "environmental variable is not set",
        ):
            power_switch_using_dmc.PowerSwitchUsingDmc(device_name="fx-emu")

    def test_power_switch_using_dmc_is_a_power_switch(self) -> None:
        """Test case to make sure PowerSwitchUsingDmc is PowerSwitch."""
        self.assertIsInstance(
            self.power_switch_using_dmc_obj, power_switch.PowerSwitch
        )

    @mock.patch.object(
        power_switch_using_dmc.PowerSwitchUsingDmc,
        "_run",
        autospec=True,
    )
    def test_power_off(
        self, mock_power_switch_using_dmc_run: mock.Mock
    ) -> None:
        """Test case for PowerSwitchUsingDmc.power_off()."""
        self.power_switch_using_dmc_obj.power_off()
        mock_power_switch_using_dmc_run.assert_called_once()

    @mock.patch.object(
        power_switch_using_dmc.PowerSwitchUsingDmc,
        "_run",
        autospec=True,
    )
    def test_power_on(self, mock_power_switch_using_dmc_run: mock.Mock) -> None:
        """Test case for PowerSwitchUsingDmc.power_on()."""
        self.power_switch_using_dmc_obj.power_on()
        mock_power_switch_using_dmc_run.assert_called_once()

    @mock.patch.object(
        host_shell,
        "run",
        autospec=True,
    )
    def test_run(self, mock_host_shell_run: mock.Mock) -> None:
        """Test case for PowerSwitchUsingDmc._run() success case."""
        self.power_switch_using_dmc_obj._run(  # pylint: disable=protected-access
            command=["ls"]
        )
        mock_host_shell_run.assert_called_once()

    @mock.patch.object(
        host_shell,
        "run",
        side_effect=errors.HostCmdError("error"),
        autospec=True,
    )
    def test_run_error(self, mock_host_shell_run: mock.Mock) -> None:
        """Test case for PowerSwitchUsingDmc._run() failure case."""
        with self.assertRaises(power_switch.PowerSwitchError):
            self.power_switch_using_dmc_obj._run(  # pylint: disable=protected-access
                command=["ls"]
            )
        mock_host_shell_run.assert_called_once()
