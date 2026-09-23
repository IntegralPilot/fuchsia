# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for power_switch_using_pdu.py."""

import unittest
from typing import Any
from unittest import mock

from honeydew import errors
from honeydew.auxiliary_devices.power_switch import (
    power_switch,
    power_switch_using_pdu,
)
from honeydew.utils import host_shell
from parameterized import param, parameterized

_MOCK_PDU_HOST: str = "pdu-host-123"
_MOCK_PDU_USERNAME: str = "pdu_user"
_MOCK_PRIV_KEY_PATH: str = "/mock/path/to/id_rsa"


class PowerSwitchUsingPduTests(unittest.TestCase):
    """Unit tests for power_switch_using_pdu.py."""

    @mock.patch("os.path.exists", return_value=True, autospec=True)
    def setUp(self, _: mock.Mock) -> None:
        """Set up for PDU tests by creating a PDU object."""
        super().setUp()
        self.pdu_obj: power_switch_using_pdu.PowerSwitchUsingPdu = (
            power_switch_using_pdu.PowerSwitchUsingPdu(
                pdu_host=_MOCK_PDU_HOST,
                pdu_username=_MOCK_PDU_USERNAME,
                priv_key_path=_MOCK_PRIV_KEY_PATH,
            )
        )
        self.outlet_num: int = 7

    @mock.patch("os.path.exists", return_value=False, autospec=True)
    def test_instantiate_power_switch_using_pdu_when_key_not_found(
        self, mock_exists: mock.Mock
    ) -> None:
        """Test case to make sure creating PDU object fails if SSH key is missing."""
        with self.assertRaisesRegex(
            power_switch_using_pdu.PowerSwitchPduError,
            "SSH private key not found at path",
        ):
            power_switch_using_pdu.PowerSwitchUsingPdu(
                pdu_host=_MOCK_PDU_HOST,
                pdu_username=_MOCK_PDU_USERNAME,
                priv_key_path=_MOCK_PRIV_KEY_PATH,
            )
        mock_exists.assert_called_once_with(_MOCK_PRIV_KEY_PATH)

    def test_power_switch_using_pdu_is_a_power_switch(self) -> None:
        """Test case to make sure PowerSwitchUsingPdu is PowerSwitch."""
        self.assertIsInstance(self.pdu_obj, power_switch.PowerSwitch)

    @parameterized.expand(
        [
            param(
                {
                    "label": "off",
                    "method": "power_off",
                    "expected_state": "false",
                }
            ),
            param(
                {
                    "label": "on",
                    "method": "power_on",
                    "expected_state": "true",
                }
            ),
        ]
    )
    @mock.patch.object(
        power_switch_using_pdu.PowerSwitchUsingPdu, "run", autospec=True
    )
    def test_power_on_off_success(
        self,
        test_data: dict[str, Any],
        mock_run: mock.Mock,
    ) -> None:
        """Test case for PowerSwitchUsingPdu.power_on() and power_off() success."""
        method = getattr(self.pdu_obj, test_data["method"])
        outlet = self.outlet_num
        expected_state = test_data["expected_state"]
        method(outlet=self.outlet_num)
        mock_run.assert_called_once()
        actual_cmd_list = mock_run.call_args[1]["command"]
        expected_cmd_start = f"ssh -i {_MOCK_PRIV_KEY_PATH} {_MOCK_PDU_USERNAME}@{_MOCK_PDU_HOST}"
        expected_remote_cmd = (
            f"uom set relay/outlets/{outlet}/state {expected_state}"
        )

        command_string = " ".join(actual_cmd_list)

        self.assertIn(expected_cmd_start, command_string)
        self.assertIn(expected_remote_cmd, command_string)

    @parameterized.expand(
        [
            param({"label": "off_no_outlet", "method": "power_off"}),
            param({"label": "on_no_outlet", "method": "power_on"}),
        ]
    )
    def test_power_on_off_no_outlet_failure(
        self, test_data: dict[str, Any]
    ) -> None:
        """Test case to ensure failure when outlet is None."""
        method = getattr(self.pdu_obj, test_data["method"])
        with self.assertRaisesRegex(
            power_switch_using_pdu.PowerSwitchPduError,
            "Outlet number must be specified",
        ):
            method(outlet=None)

    @mock.patch.object(
        host_shell,
        "run",
        side_effect=errors.HostCmdError("SSH connection failed"),
        autospec=True,
    )
    def test_run_error(self, mock_host_shell_run: mock.Mock) -> None:
        """Test case for PowerSwitchUsingPdu._run() failure case, wrapping HostCmdError."""
        with self.assertRaises(power_switch_using_pdu.PowerSwitchPduError):
            self.pdu_obj.run(
                command=["ssh", "fail-command"]
            )  # pylint: disable=protected-access
        mock_host_shell_run.assert_called_once()
