# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for PowerSwitch interface."""

import logging

import fuchsia_base_test
from honeydew.auxiliary_devices.power_switch import (
    power_switch,
    power_switch_using_dmc,
    power_switch_using_pdu,
)
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

PDU_CONFIG_KEY: str = "pdu_config"


class PowerSwitchTest(fuchsia_base_test.FuchsiaBaseTest):
    """Mobly test for PowerSwitchDmc implementation of PowerSwitch interface."""

    _power_switch: power_switch.PowerSwitch
    _outlet_arg: int | None

    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()

        try:
            _LOGGER.info(
                "Attempting to instantiate PowerSwitchUsingDmc (Infra Mode)..."
            )
            self._power_switch = power_switch_using_dmc.PowerSwitchUsingDmc(
                device_name=self.dut.device_name
            )
            self._outlet_arg = None
            _LOGGER.info("Successfully configured for DMC.")
        except power_switch_using_dmc.PowerSwitchDmcError:
            _LOGGER.info(
                "DMC environment variable not found. Falling back to PDU (Host Mode)..."
            )

            pdu_config = self.user_params.get(PDU_CONFIG_KEY)

            if not pdu_config:
                asserts.abort_class(
                    f"'{PDU_CONFIG_KEY}' required for PDU mode."
                )
            assert pdu_config is not None
            try:
                self._power_switch = power_switch_using_pdu.PowerSwitchUsingPdu(
                    pdu_host=pdu_config["host"],
                    pdu_username=pdu_config["username"],
                    priv_key_path=pdu_config["priv_key_path"],
                )
                self._outlet_arg = pdu_config["outlet"]
                _LOGGER.info("Successfully configured for PDU.")
            except KeyError as e:
                asserts.abort_class(f"Missing required key in pdu_config: {e}")
            except power_switch_using_pdu.PowerSwitchPduError as e:
                asserts.abort_class(f"PDU setup failed: {e}")

    async def test_power_switch(self) -> None:
        """Test case for PowerSwitchDmc.power_off and PowerSwitchDmc.power_on"""
        _LOGGER.info(
            "Testing power switch using %s",
            self._power_switch.__class__.__name__,
        )

        # Check if device is online before powering off
        await self.dut.wait_for_online()

        # power off the device using the dynamically determined outlet argument
        _LOGGER.info("Starting power_off test cycle.")
        self.dut.ffx.notify_intentional_disconnect()
        self._power_switch.power_off(outlet=self._outlet_arg)

        self.dut.wait_for_offline()

        # power on the device using the dynamically determined outlet argument
        _LOGGER.info("Starting power_on test cycle.")
        self._power_switch.power_on(outlet=self._outlet_arg)

        await self.dut.wait_for_online()
        await self.dut.on_device_boot()

        _LOGGER.info("Power cycle test completed successfully.")


if __name__ == "__main__":
    test_runner.main()
