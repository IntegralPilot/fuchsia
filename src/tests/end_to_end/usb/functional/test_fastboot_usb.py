# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Fastboot USB bulk payload staging and PHY state handoff."""

import asyncio
import logging
import os
import re
import tempfile
from typing import Tuple

import fidl_fuchsia_hardware_power_statecontrol as fhp_statecontrol
import fuchsia_base_test
from honeydew import errors
from honeydew.auxiliary_devices.usb_power_hub import usb_power_hub
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.typing import custom_types as honeydew_types
from honeydew.utils import host_shell
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

_USB_PERIPHERAL_API_NAME = "fuchsia.hardware.usb.dci.UsbDciService"

_GETVAR_REGEX = re.compile(
    r"(\(bootloader\) )?(?P<name>[a-zA-Z0-9_\-:]+): ?(?P<val>.*)"
)


def _parse_getvar_line(line: str) -> Tuple[str, str] | None:
    """Parses a single `getvar` output line into a (name, value) tuple."""
    if "waiting for" in line or not line.strip():
        return None
    match = _GETVAR_REGEX.match(line.strip())
    if match:
        return (match.group("name"), match.group("val").strip())
    return None


class UsbFastbootTest(fuchsia_base_test.FuchsiaBaseTest):
    """Verifies Fastboot USB bulk payload staging and PHY state handoff."""

    async def setup_class(self) -> None:
        """setup_class is called once before running test cases."""
        await super().setup_class()
        self._usb_power_hub: usb_power_hub.UsbPowerHub | None = (
            self.dut.usb_power_hub
        )
        self._usb_port: int | None = self.dut.usb_power_hub_port
        if self._usb_power_hub is None:
            _LOGGER.info(
                "No USB power hub configured for %s. Proceeding without one.",
                self.dut.device_name,
            )
        else:
            self._usb_power_hub.power_on(port=self._usb_port)
            _LOGGER.info(
                "Successfully bound USB Power Hub fixture on port %s",
                self._usb_port,
            )

        # Pre-cache PersistentProperty values (board, product) while the device is online
        # in Fuchsia OS. Because they are lazily evaluated, if left un-evaluated and the test
        # later fails while in Fastboot mode or offline, Mobly's post-test metadata collection
        # (test_summary.yaml) would trigger an FFX query against the offline device.
        _ = self.dut.board
        _ = self.dut.product

    async def _reboot_to_fastboot_mode(
        self, fuchsia_reboot_timeout: int = 30
    ) -> None:
        """Reboots the DUT to bootloader/fastboot mode."""
        # Ensure USB power is turned ON before rebooting so bootloader sees VBUS
        if self._usb_power_hub:
            self._usb_power_hub.power_on(port=self._usb_port)

        try:
            if await self.dut.fastboot.is_in_fastboot_mode():
                _LOGGER.info("Device is already in Fastboot mode.")
                self.dut.fastboot._ready = True
                return
        except Exception as e:
            _LOGGER.debug("Check is_in_fastboot_mode error: %s", e)

        _LOGGER.info("Rebooting device to bootloader via FIDL...")

        power_admin_endpoint = honeydew_types.FidlEndpoint(
            "/bootstrap/shutdown_shim",
            "fuchsia.hardware.power.statecontrol.Admin",
        )

        try:
            await self.dut.fastboot._get_fastboot_node()
        except Exception as e:
            _LOGGER.debug(
                "Pre-populating fastboot node ID raised exception: %s", e
            )

        try:
            self.dut.ffx.notify_intentional_disconnect()
            fc_transport = self.dut.fuchsia_controller
            power_proxy = fhp_statecontrol.AdminClient(
                fc_transport.connect_device_proxy(power_admin_endpoint)
            )
            await power_proxy.shutdown(
                options=fhp_statecontrol.ShutdownOptions(
                    action=fhp_statecontrol.ShutdownAction.REBOOT_TO_BOOTLOADER,
                    reasons=[fhp_statecontrol.ShutdownReason.DEVELOPER_REQUEST],
                )
            )
        except Exception as e:
            _LOGGER.debug(
                "Reboot command raised exception (expected if device rebooted quickly): %s",
                e,
            )

        _LOGGER.info("Waiting for device to enter fastboot mode...")
        try:
            await asyncio.wait_for(
                self.dut.fastboot.wait_for_fastboot_mode(),
                timeout=fuchsia_reboot_timeout,
            )
        except asyncio.TimeoutError:
            _LOGGER.warning(
                "Device did not enter fastboot mode after FIDL shutdown. Attempting fallback via FFX reboot..."
            )
            try:
                self.dut.ffx.notify_intentional_disconnect()
                self.dut.ffx.run(
                    cmd=["target", "reboot", "--bootloader"],
                    log_status_on_failure=False,
                    timeout=15,
                )
            except Exception as e:
                _LOGGER.debug("FFX reboot to bootloader exception: %s", e)
            await asyncio.wait_for(
                self.dut.fastboot.wait_for_fastboot_mode(),
                timeout=fuchsia_reboot_timeout,
            )

        # Populate fastboot_node_id if needed and mark fastboot transport ready
        if self.dut.fastboot._fastboot_node_id is None:
            try:
                fb_devices_output = (
                    host_shell.run(
                        cmd=[self.dut.fastboot._fastboot_binary, "devices"]
                    )
                    or ""
                )
                discovered_serials: list[str] = []
                for line in fb_devices_output.strip().split("\n"):
                    tokens = line.split()
                    if len(tokens) >= 2 and tokens[1] == "fastboot":
                        discovered_serials.append(tokens[0])

                if len(discovered_serials) > 1:
                    _LOGGER.warning(
                        "Multiple devices detected in fastboot mode: %s; selecting %s",
                        discovered_serials,
                        discovered_serials[0],
                    )

                if discovered_serials:
                    self.dut.fastboot._fastboot_node_id = discovered_serials[0]
                    _LOGGER.info(
                        "Discovered fastboot node ID from fastboot devices: %s",
                        self.dut.fastboot._fastboot_node_id,
                    )
            except Exception as e:
                _LOGGER.debug(
                    "Could not resolve fastboot serial from fastboot devices: %s",
                    e,
                )

        self.dut.fastboot._ready = True

    async def _teardown_recover_fuchsia(
        self, fuchsia_reboot_timeout: int = 60
    ) -> None:
        """Ensures DUT is rebooted out of Fastboot and restored back to Fuchsia."""
        _LOGGER.info("Ensuring device is booted back to Fuchsia...")
        if self._usb_power_hub:
            self._usb_power_hub.power_on(port=self._usb_port)

        was_in_fastboot = False
        try:
            if await self.dut.fastboot.is_in_fastboot_mode():
                was_in_fastboot = True
                _LOGGER.info(
                    "Device is in Fastboot mode; issuing fastboot reboot to Fuchsia..."
                )
                await asyncio.wait_for(
                    self.dut.fastboot.boot_to_fuchsia_mode(),
                    timeout=fuchsia_reboot_timeout,
                )
        except Exception as e:
            _LOGGER.warning("fastboot reboot to Fuchsia command error (%s).", e)

        if was_in_fastboot:
            _LOGGER.info("Waiting for device to come back online in Fuchsia...")
            try:
                await asyncio.wait_for(
                    self.dut.wait_for_online(), timeout=fuchsia_reboot_timeout
                )
            except asyncio.TimeoutError as e:
                raise errors.FuchsiaDeviceError(
                    f"Timed out waiting for device to come online in Fuchsia after {fuchsia_reboot_timeout}s"
                ) from e
            await self.dut.on_device_boot()
            _LOGGER.info("Device is successfully back online in Fuchsia.")
        else:
            try:
                await asyncio.wait_for(self.dut.wait_for_online(), timeout=15)
            except Exception:
                self.dut.health_check()

    async def teardown_test(self) -> None:
        """Ensures device is returned safely to Fuchsia mode after each test."""
        try:
            await self._teardown_recover_fuchsia(fuchsia_reboot_timeout=60)
        except Exception as e:
            _LOGGER.warning("Error recovering device during teardown: %s", e)
        finally:
            await super().teardown_test()

    async def test_fastboot_stage_payload(self) -> None:
        """Verifies bounded USB bulk payload staging in Fastboot mode."""
        _LOGGER.info("Booting DUT to Fastboot mode...")
        await self._reboot_to_fastboot_mode(fuchsia_reboot_timeout=30)

        asserts.assert_true(
            await self.dut.fastboot.is_in_fastboot_mode(),
            msg=f"{self.dut.device_name} failed to enter fastboot mode.",
        )

        _LOGGER.info(
            "Querying Fastboot `max-download-size` to bound bulk payload staging..."
        )
        max_dl_lines = await self.dut.fastboot.run(
            ["getvar", "max-download-size"]
        )
        max_dl_parsed: list[Tuple[str, str]] = [
            res
            for line in max_dl_lines
            if (res := _parse_getvar_line(line)) is not None
        ]
        asserts.assert_true(
            len(max_dl_parsed) > 0,
            msg="Failed to parse max-download-size from fastboot getvar query",
        )
        _LOGGER.info("Parsed max-download-size: %s", max_dl_parsed[0])

        # Stage a safe 4MB payload, bounded by max-download-size, to thoroughly
        # exercise USB bulk OUT transfers without risking bootloader RAM exhaustion or timeouts (b/553616205).
        max_dl_bytes = 0
        if max_dl_parsed:
            max_dl_val_str = max_dl_parsed[0][1].strip()
            try:
                if max_dl_val_str.lower().startswith("0x"):
                    max_dl_bytes = int(max_dl_val_str, 16)
                else:
                    max_dl_bytes = int(max_dl_val_str)
            except ValueError:
                max_dl_bytes = 0

        stage_size_bytes = 4 * 1024 * 1024
        if max_dl_bytes > 0:
            stage_size_bytes = min(stage_size_bytes, max_dl_bytes // 2)

        _LOGGER.info(
            "Generating %d-byte payload for Fastboot staging...",
            stage_size_bytes,
        )
        with tempfile.NamedTemporaryFile(delete=False) as f:
            f.write(os.urandom(stage_size_bytes))
            temp_path = f.name

        try:
            _LOGGER.info(
                "Staging %d-byte payload %s to Fastboot memory buffer via USB...",
                stage_size_bytes,
                temp_path,
            )
            # Run 'stage' command using the underlying ffx target fastboot execution.
            await self.dut.fastboot.run(["stage", temp_path])
            _LOGGER.info(
                "Successfully staged payload over USB without flashing."
            )
        finally:
            os.remove(temp_path)

        _LOGGER.info("Rebooting DUT back to Fuchsia mode...")
        await self._teardown_recover_fuchsia(fuchsia_reboot_timeout=60)

    async def test_phy_state_handoff_and_reenumeration(self) -> None:
        """Verifies seamless USB PHY state handoff and driver re-enumeration after boot to Zircon."""
        _LOGGER.info(
            "Booting DUT to Fastboot mode to test PHY state handoff..."
        )
        await self._reboot_to_fastboot_mode(fuchsia_reboot_timeout=30)

        asserts.assert_true(
            await self.dut.fastboot.is_in_fastboot_mode(),
            msg=f"{self.dut.device_name} failed to enter fastboot mode.",
        )

        _LOGGER.info(
            "Rebooting DUT out of Fastboot to trigger PHY handoff to Zircon..."
        )
        await self._teardown_recover_fuchsia(fuchsia_reboot_timeout=60)

        _LOGGER.info("Verifying DUT is online in Fuchsia mode...")
        await self.dut.wait_for_online()

        # TODO(b/556242644): Refactor duplicated USB driver and inspect verification
        # into a shared usb_health_check helper once utilities are aligned between
        # Sorrel and Iris.
        _LOGGER.info("Executing diagnostics: checking USB peripheral driver...")
        driver_list = self.dut.ffx.run(["driver", "list-devices", "-v"])
        asserts.assert_in(
            _USB_PERIPHERAL_API_NAME,
            driver_list,
            msg=f"Expected {_USB_PERIPHERAL_API_NAME} driver to be active after PHY handoff",
        )

        _LOGGER.info(
            "Executing diagnostics: ffx inspect show for usb-policy..."
        )
        cli_output = ""
        for moniker in ["core/usb-policy", "bootstrap/boot-drivers"]:
            try:
                cli_output += self.dut.ffx.run(["inspect", "show", moniker])
            except ffx_errors.FfxCommandError as e:
                _LOGGER.debug(f"Inspect query on {moniker} failed: {e}")

        if "usb_state_history" not in cli_output:
            _LOGGER.info(
                "Not in core/usb-policy or boot-drivers, checking entire Inspect tree..."
            )
            try:
                cli_output += self.dut.ffx.run(["inspect", "show"])
            except ffx_errors.FfxCommandError as e:
                _LOGGER.debug(f"Full inspect show failed: {e}")

        asserts.assert_in(
            "usb_state_history",
            cli_output,
            msg="Expected usb_state_history in ffx inspect show after PHY handoff",
        )
        _LOGGER.info(
            "Successfully verified USB PHY state handoff and re-enumeration."
        )


if __name__ == "__main__":
    test_runner.main()
