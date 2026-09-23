#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Abstract base class and discovery interface for testusb backends.

Defines `USBTestBackend` which specifies the common interface implemented by
concrete backends (such as ioctl and mock backends), including lifecycle
management, configuration switching, transfer operations, and device discovery.
"""

from __future__ import annotations

import contextlib
import logging
import os
import stat
from abc import ABC, abstractmethod
from typing import Any

from .models import TestParams, TestResult, TestStatus

_LOGGER = logging.getLogger(__name__)

USB_DT_DEVICE = 0x01
USB_DT_DEVICE_SIZE = 18

_USB_DIR = "/dev/bus/usb"
_SYSFS_USB_DEVICES_DIR = "/sys/bus/usb/devices"

# Recognized USB Test Device VENDOR:PRODUCT IDs
KNOWN_TEST_DEVICES: frozenset[tuple[int, int]] = frozenset(
    {
        (0x18D1, 0xA022),  # Fuchsia USB Test Gadget (Source/Sink)
        (0x18D1, 0xA023),  # Fuchsia USB Test Gadget (Loopback)
    }
)


class USBTestBackend(ABC):
    """Abstract base class for USB test backends."""

    def __init__(self, device_path: str | None = None) -> None:
        """Initialize the USB test backend.

        Args:
            device_path: Optional path to the target USB device node.
        """
        self.device_path = device_path
        self.is_open = False
        self.speed_name = "unknown"
        self.current_config = 1

    @abstractmethod
    def open(self) -> None:
        """Open the USB device."""

    @abstractmethod
    def close(self) -> None:
        """Close the USB device."""

    @abstractmethod
    def get_speed(self) -> str:
        """Get the USB device speed as a string."""

    @abstractmethod
    def set_configuration(self, config_val: int) -> bool:
        """Set the active USB configuration.

        Args:
            config_val: Target configuration number (e.g. 1 for Source/Sink,
                2 for Loopback).

        Returns:
            True if configuration was successfully set, False otherwise.
        """

    def get_configuration(self) -> int:
        """Get the active USB configuration number."""
        return self.current_config

    def get_num_configurations(self) -> int:
        """Get total number of USB configurations supported by device."""
        sysfs_device = getattr(self, "sysfs_device", None)
        if sysfs_device is not None:
            val = getattr(sysfs_device, "bNumConfigurations", None)
            if val is not None:
                try:
                    return int(val)
                except (ValueError, TypeError) as err:
                    _LOGGER.warning(
                        "Failed to parse bNumConfigurations %r as integer: %s",
                        val,
                        err,
                    )
        return 1

    def run_bulk_loopback(
        self,
        params: TestParams,
        length_vary: bool = False,
        is_short: bool = False,
        is_zlp: bool = False,
        test_id: int = 0,
        test_name: str = "Loopback: BULK_ROUNDTRIP",
        ep_out: int | None = None,
        ep_in: int | None = None,
    ) -> TestResult:
        """Run bidirectional bulk loopback test.

        Args:
            params: Test parameters controlling transfer length and iterations.
            length_vary: If True, vary transfer lengths across iterations.
            is_short: If True, test short packet handling.
            is_zlp: If True, test zero-length packet handling.
            test_id: Numerical identifier of the test case.
            test_name: Human-readable name of the test case.
            ep_out: Target bulk OUT endpoint address.
            ep_in: Target bulk IN endpoint address.

        Returns:
            TestResult summarizing outcome and transfer metrics.
        """
        return TestResult(
            test_id=test_id,
            test_name=test_name,
            status=TestStatus.SKIP,
            duration_secs=0.0,
            error_message="Bulk loopback test not implemented for backend",
        )

    @abstractmethod
    def claimed_interface(
        self, ifnum: int
    ) -> contextlib.AbstractContextManager[None]:
        """Context manager to claim and manage the lifecycle of a USB interface.

        Args:
            ifnum: Interface number to claim.

        Returns:
            Context manager yielding None once the interface has been claimed.
        """

    @abstractmethod
    def run_test(self, test_id: int, params: TestParams) -> TestResult:
        """Run a specific test case.

        Args:
            test_id: Numerical test identifier matching Linux kernel usbtest.
            params: Test execution parameters.

        Returns:
            TestResult summarizing outcome and transfer metrics.
        """

    def __enter__(self) -> USBTestBackend:
        """Open the backend on context manager entry."""
        self.open()
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: Any,
    ) -> None:
        """Close the backend on context manager exit."""
        self.close()

    @classmethod
    def discover_devices(cls) -> list[str]:
        """Discover available USB test devices on the system.

        Returns:
            Sorted list of device node paths matching known test device
            VID/PIDs.
        """
        found = set()

        # Prefer non-blocking sysfs scan
        if os.path.exists(_SYSFS_USB_DEVICES_DIR):
            try:
                for entry in os.listdir(_SYSFS_USB_DEVICES_DIR):
                    p = os.path.join(_SYSFS_USB_DEVICES_DIR, entry)
                    v_path = os.path.join(p, "idVendor")
                    p_path = os.path.join(p, "idProduct")
                    b_path = os.path.join(p, "busnum")
                    d_path = os.path.join(p, "devnum")
                    if (
                        os.path.exists(v_path)
                        and os.path.exists(p_path)
                        and os.path.exists(b_path)
                        and os.path.exists(d_path)
                    ):
                        try:
                            with (
                                open(v_path, encoding="utf-8") as fv,
                                open(p_path, encoding="utf-8") as fp,
                                open(b_path, encoding="utf-8") as fb,
                                open(d_path, encoding="utf-8") as fd,
                            ):
                                vid = int(fv.read().strip(), 16)
                                pid = int(fp.read().strip(), 16)
                                bus = int(fb.read().strip())
                                dev = int(fd.read().strip())
                                if (vid, pid) in KNOWN_TEST_DEVICES:
                                    dev_node = (
                                        f"/dev/bus/usb/{bus:03d}/{dev:03d}"
                                    )
                                    if os.path.exists(dev_node) and os.access(
                                        dev_node, os.R_OK | os.W_OK
                                    ):
                                        found.add(dev_node)
                        except (OSError, ValueError) as err:
                            _LOGGER.debug(
                                "Skipping sysfs entry %s: %s", entry, err
                            )
                            continue
            except (OSError, ValueError) as err:
                _LOGGER.debug(
                    "Error listing sysfs directory %s: %s",
                    _SYSFS_USB_DEVICES_DIR,
                    err,
                )
            if found:
                return sorted(found)

        if not os.path.exists(_USB_DIR):
            return sorted(found)

        for root, _, files in os.walk(_USB_DIR):
            for file in files:
                path = os.path.join(root, file)
                if not os.access(path, os.R_OK | os.W_OK):
                    continue
                try:
                    st = os.stat(path)
                    if not stat.S_ISCHR(st.st_mode):
                        continue
                except OSError as err:
                    _LOGGER.debug("Failed stat on %s: %s", path, err)
                    continue

                if cls._is_test_device(path):
                    found.add(path)
        return sorted(found)

    @classmethod
    def _is_test_device(cls, dev_path: str) -> bool:
        """Check if a device node matches a known USB test gadget VID/PID.

        Args:
            dev_path: Path to the character device node.

        Returns:
            True if the device descriptor matches a known test gadget VID/PID.
        """
        try:
            with open(dev_path, "rb") as f:
                data = f.read(USB_DT_DEVICE_SIZE)
                if len(data) < USB_DT_DEVICE_SIZE:
                    return False
                if data[0] != USB_DT_DEVICE_SIZE or data[1] != USB_DT_DEVICE:
                    return False
                vid = int.from_bytes(data[8:10], "little")
                pid = int.from_bytes(data[10:12], "little")
                return (vid, pid) in KNOWN_TEST_DEVICES
        except OSError as err:
            _LOGGER.debug(
                "Failed reading USB descriptor from %s: %s", dev_path, err
            )
            return False
