# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Linux Virtual UsbPower auxiliary device implementation."""

import glob
import logging
import os
import platform
import re

from honeydew import errors
from honeydew.auxiliary_devices.usb_power_hub import usb_power_hub
from honeydew.utils import host_shell

_LOGGER: logging.Logger = logging.getLogger(__name__)


class LinuxVirtualUsbPowerHub(usb_power_hub.UsbPowerHub):
    """LinuxVirtualUsbPowerHub auxiliary device implementation.

    This class enables virtual USB plug/unplug (authorization control)
    on a Linux host. It is intended for local at-desk developer testing
    and virtual/emulated testbeds where physical power hubs are not available.

    Note on permissions:
        Writing to the authorized file requires root privileges by default.
        To run this without sudo (e.g. in automated tests or local development
        without prompting for password), you can set up a udev rule on the host:

        ```udev
        SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="18d1", ATTR{idProduct}=="a02b", RUN+="/bin/chmod a+w /sys/bus/usb/devices/$kernel/authorized"
        ```
        Replace the vendor/product IDs if your device uses different ones.

    Args:
        target_serial: The serial number of the Fuchsia device. Used for
            discovery to match against connected USB devices.
        use_sudo: Whether to use `sudo` to write to the authorized file.
            Defaults to False.
    """

    def __init__(
        self,
        target_serial: str | None = None,
        use_sudo: bool = False,
    ) -> None:
        super().__init__()
        if platform.system() != "Linux":
            raise usb_power_hub.UsbPowerHubError(
                "LinuxVirtualUsbPowerHub is only supported on Linux hosts."
            )

        self._use_sudo = use_sudo
        self._target_serial = target_serial
        self._usb_bus_id: str = self._find_usb_bus_id()

        if not re.match(r"^[a-zA-Z0-9.-]+$", self._usb_bus_id):
            raise ValueError(f"Invalid usb_bus_id format: {self._usb_bus_id}")

    def power_off(self, port: int | None = None) -> None:
        """Deauthorizes (virtually unplugs) the USB device.

        Args:
            port: None. Not used by this implementation.
        """
        _LOGGER.info("Virtually unplugging USB device %s...", self._usb_bus_id)
        cmd: list[str] = []
        if self._use_sudo:
            cmd.append("sudo")
        cmd.extend(
            [
                "sh",
                "-c",
                f"echo 0 > /sys/bus/usb/devices/{self._usb_bus_id}/authorized",
            ]
        )
        try:
            host_shell.run(cmd=cmd)
        except errors.HostCmdError as err:
            raise usb_power_hub.UsbPowerHubError(err) from err
        _LOGGER.info(
            "Successfully virtually unplugged USB device %s.", self._usb_bus_id
        )

    def power_on(self, port: int | None = None) -> None:
        """Authorizes (virtually plugs in) the USB device.

        Args:
            port: None. Not used by this implementation.
        """
        _LOGGER.info("Virtually plugging in USB device %s...", self._usb_bus_id)
        cmd: list[str] = []
        if self._use_sudo:
            cmd.append("sudo")
        cmd.extend(
            [
                "sh",
                "-c",
                f"echo 1 > /sys/bus/usb/devices/{self._usb_bus_id}/authorized",
            ]
        )
        try:
            host_shell.run(cmd=cmd)
        except errors.HostCmdError as err:
            raise usb_power_hub.UsbPowerHubError(err) from err
        _LOGGER.info(
            "Successfully virtually plugged in USB device %s.", self._usb_bus_id
        )

    def _find_usb_bus_id(self) -> str:
        """Finds the USB bus ID for the target device."""
        vendor_id = "18d1"
        product_ids = {p.lower() for p in ["a02b", "a025", "d00d"]}

        serial = self._target_serial

        matching_devices = []
        for dev_path in glob.glob("/sys/bus/usb/devices/*"):
            vendor_file = os.path.join(dev_path, "idVendor")
            product_file = os.path.join(dev_path, "idProduct")
            serial_file = os.path.join(dev_path, "serial")

            if not (
                os.path.exists(vendor_file) and os.path.exists(product_file)
            ):
                continue

            try:
                with open(vendor_file, "r", encoding="utf-8") as f_v, open(
                    product_file, "r", encoding="utf-8"
                ) as f_p:
                    v_id = f_v.read().strip().lower()
                    p_id = f_p.read().strip().lower()

                if v_id != vendor_id.lower() or p_id not in product_ids:
                    continue

                if serial:
                    if not os.path.exists(serial_file):
                        continue
                    with open(serial_file, "r", encoding="utf-8") as f_s:
                        dev_serial = f_s.read().strip()
                    if dev_serial != serial:
                        continue

                matching_devices.append(os.path.basename(dev_path))
            except OSError:
                pass

        if not matching_devices:
            raise ValueError(
                f"No USB device with vendor {vendor_id} and products {product_ids} (serial: {serial}) found."
            )
        if len(matching_devices) > 1:
            raise ValueError(
                f"Multiple USB devices found: {matching_devices}. "
                "Please specify target_serial explicitly."
            )
        return matching_devices[0]
