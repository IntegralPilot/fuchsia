# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fuchsia_base_test
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

_USB_PERIPHERAL_API_NAME = "fuchsia.hardware.usb.dci.UsbDciService"
_USB_HOST_API_NAME = "fuchsia.hardware.usb.hci.UsbHciService"

_USB_PERIPHERAL_NAME = "USB Peripheral"
_USB_HOST_NAME = "USB Host"

_LSUSB_DEVICE_TOKEN = "Device"


class UsbEnumerationTest(fuchsia_base_test.FuchsiaBaseTest):
    """Verifies USB host and peripheral mode functionality on a DUT."""

    def test_lsusb_device_list(self) -> None:
        """Verifies 'lsusb' detects a device when in host mode."""

        _LOGGER.info("Executing 'lsusb' command on target...")
        lsusb_output = self.dut.ffx.run_ssh_cmd("lsusb")
        _LOGGER.info(f"lsusb output:\n{lsusb_output}")

        is_host = self._is_operating_mode_supported(_USB_HOST_API_NAME)

        if is_host:
            _LOGGER.info(
                f"Host mode is supported. Verifying '{_LSUSB_DEVICE_TOKEN}' "
                "is present in lsusb output..."
            )
            asserts.assert_in(
                _LSUSB_DEVICE_TOKEN,
                lsusb_output,
                (
                    "Expected to find a USB device not a host, but lsusb"
                    f" output was: {lsusb_output}"
                ),
            )
            _LOGGER.info("Successfully verified host mode enumeration.")
        else:
            _LOGGER.info(
                f"Host mode is NOT supported. Verifying '{_LSUSB_DEVICE_TOKEN}' "
                "is absent in lsusb output..."
            )
            asserts.assert_not_in(
                _LSUSB_DEVICE_TOKEN,
                lsusb_output,
                (
                    "Unexpectedly found a USB host device, but lsusb output"
                    f" was: {lsusb_output}"
                ),
            )
            _LOGGER.info(
                "Successfully verified device is not enumerating without host mode."
            )

    def test_usb_peripheral_detected(self) -> None:
        """Verifies the peripheral driver is loaded when in peripheral mode."""
        self._verify_driver_loaded(
            _USB_PERIPHERAL_API_NAME, _USB_PERIPHERAL_NAME
        )

    def test_usb_host_detected(self) -> None:
        """Verifies the host driver is loaded when in host mode."""
        self._verify_driver_loaded(_USB_HOST_API_NAME, _USB_HOST_NAME)

    def test_usb_cli_diagnostics(self) -> None:
        """Verifies 'usb-cli diagnostics' runs successfully and returns diagnostics."""
        _LOGGER.info("Executing 'usb-cli diagnostics' command on target...")
        output = self.dut.ffx.run_ssh_cmd("usb-cli diagnostics")
        _LOGGER.info(f"usb-cli diagnostics output:\n{output}")

        asserts.assert_in(
            "=== Inspect:",
            output,
            "Expected to find Inspect header in usb-cli output",
        )
        asserts.assert_in(
            "usb_state_history",
            output,
            "Expected to find usb_state_history in usb-cli output",
        )

    def _verify_driver_loaded(self, required_api: str, name: str) -> None:
        """Helper to verify that a driver providing the API is loaded if the
        mode is supported.
        """
        _LOGGER.info(f"Verifying {name} driver load status...")
        is_supported = self._is_operating_mode_supported(required_api)

        # We dump the driver list to check if a driver exposing the API is
        # loaded
        driver_list_devices_output = self.dut.ffx.run(
            ["driver", "list-devices", "-v"]
        )

        has_driver = required_api in driver_list_devices_output
        _LOGGER.info(
            f"API {required_api} presence in driver list: {has_driver}"
        )

        if is_supported:
            _LOGGER.info(f"{name} is supported. Asserting driver is loaded...")
            asserts.assert_true(
                has_driver,
                (
                    f"Expected to find {name} driver exposing {required_api}, "
                    f"but it was missing from driver list-devices output."
                ),
            )
            _LOGGER.info(
                f"Successfully verified {name} driver is actively loaded."
            )
        else:
            _LOGGER.info(
                f"{name} is NOT supported. Asserting driver is NOT loaded..."
            )
            asserts.assert_false(
                has_driver,
                (
                    f"Unexpectedly found {name} driver exposing {required_api}, "
                    f"but this operating mode is not supported by DUT."
                ),
            )
            _LOGGER.info(f"Successfully verified {name} driver is absent.")

    def _is_operating_mode_supported(self, operating_mode: str) -> bool:
        """Determines if the requested operating mode is supported by the DUT"""
        _LOGGER.info(
            f"Checking if operating mode API '{operating_mode}' is supported by "
            "any loaded driver..."
        )
        driver_list_devices_output = self.dut.ffx.run(
            ["driver", "list-devices", "-v"]
        )

        if operating_mode in driver_list_devices_output:
            _LOGGER.info(f"Found driver exposing '{operating_mode}'")
            return True

        _LOGGER.info(
            f"Service '{operating_mode}' was not found in any USB-related driver."
        )
        return False


if __name__ == "__main__":
    test_runner.main()
