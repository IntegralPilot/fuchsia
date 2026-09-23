#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Base Class for Defining Common WiFi Test Functionality
"""

import logging
from typing import TypedDict, TypeVar

import fuchsia_async_extension
import openwrt_access_point
from antlion import context, controllers
from antlion.controllers.access_point import AccessPoint
from antlion.controllers.ap_lib.hostapd_security import SecurityMode
from antlion.controllers.attenuator import Attenuator
from antlion.controllers.fuchsia_device import FuchsiaDevice
from antlion.controllers.iperf_client import IPerfClientBase
from antlion.controllers.iperf_server import IPerfServer, IPerfServerOverSsh
from antlion.controllers.pdu import PduDevice
from antlion.test_utils.abstract_devices.wlan_device import FuchsiaWlanDevice
from antlion.types import Controller
from honeydew.typing import custom_types
from mobly import signals
from mobly.base_test import BaseTestClass
from mobly.config_parser import TestRunConfig
from mobly.records import TestResultRecord
from openwrt_access_point import OpenWrtAP

MAX_AP_COUNT = 2

_LOGGER: logging.Logger = logging.getLogger(__name__)


class Network(TypedDict):
    SSID: str
    security: SecurityMode
    password: str | None
    hiddenSSID: bool
    wepKeys: list[str] | None
    ieee80211w: str | None


class NetworkUpdate(TypedDict, total=False):
    SSID: str
    security: SecurityMode
    password: str | None
    hiddenSSID: bool
    wepKeys: list[str] | None
    ieee80211w: str | None


NetworkList = dict[str, Network]

_T = TypeVar("_T")


class WifiBaseTest(BaseTestClass):
    def __init__(self, configs: TestRunConfig) -> None:
        super().__init__(configs)
        self.openwrt_ap: OpenWrtAP | None = None
        self.access_point: AccessPoint | None = None

        T = TypeVar("T")

        def register_controller(module: Controller[T]) -> list[T]:
            registered_controllers: list[T] | None = self.register_controller(
                module, required=False
            )
            if registered_controllers is None:
                return []
            return registered_controllers

        self.access_points: list[AccessPoint] = register_controller(
            controllers.access_point
        )
        self.openwrt_aps: list[OpenWrtAP] = register_controller(
            openwrt_access_point
        )
        self.attenuators: list[Attenuator] = register_controller(
            controllers.attenuator
        )
        self.fuchsia_devices: list[FuchsiaDevice] = register_controller(
            controllers.fuchsia_device
        )
        self.iperf_clients: list[IPerfClientBase] = register_controller(
            controllers.iperf_client
        )
        iperf_servers: list[
            IPerfServer | IPerfServerOverSsh
        ] = register_controller(controllers.iperf_server)
        self.iperf_servers = [
            iperf_server
            for iperf_server in iperf_servers
            if isinstance(iperf_server, IPerfServerOverSsh)
        ]
        self.pdu_devices: list[PduDevice] = register_controller(controllers.pdu)

        for attenuator in self.attenuators:
            attenuator.set_atten(0)

    def setup_test(self) -> None:
        self.write_to_device_logs(
            f"Started executing '{self.current_test_info.name}'",
            custom_types.LEVEL.INFO,
        )

    def teardown_test(self) -> None:
        self.write_to_device_logs(
            f"Finished executing '{self.current_test_info.name}'",
            custom_types.LEVEL.INFO,
        )

    def teardown_class(self) -> None:
        super().teardown_class()
        if hasattr(self, "fuchsia_devices"):
            for device in self.fuchsia_devices:
                device.take_bug_report()
        self.download_logs()

        for access_point in self.access_points:
            access_point.stop_all_aps()

    def on_fail(self, record: TestResultRecord) -> None:
        """A function that is executed upon a test failure.

        Args:
        record: A copy of the test record for this test, containing all information of
            the test execution including exception objects.
        """
        # Download support device logs
        self.download_logs()

        # Gets a wlan_device log and calls the generic device fail on DUT.
        for fd in self.fuchsia_devices:
            self.on_device_fail(fd, record)

    def on_device_fail(
        self, device: FuchsiaDevice, _: TestResultRecord
    ) -> None:
        """Gets a generic device DUT bug report.

        This method takes a bug report if the device has the
        'take_bug_report_on_fail' config value, and if the flag is true. This
        method also power cycles if 'hard_reboot_on_fail' is True.

        Args:
            device: Generic device to gather logs from.
            record: More information about the test.
        """
        if (
            not hasattr(device, "take_bug_report_on_fail")
            or device.take_bug_report_on_fail
        ):
            device.take_bug_report()

        if (
            hasattr(device, "hard_reboot_on_fail")
            and device.hard_reboot_on_fail
        ):
            device.reboot(reboot_type="hard", testbed_pdus=self.pdu_devices)

    def get_dut(self) -> FuchsiaWlanDevice:
        """Get the DUT, default to Fuchsia."""
        return self.get_dut_type(FuchsiaDevice)[1]

    def get_dut_type(
        self, device_type: type[_T]
    ) -> tuple[_T, FuchsiaWlanDevice]:
        if device_type is FuchsiaDevice:
            if len(self.fuchsia_devices) == 0:
                raise signals.TestAbortClass(
                    "Requires at least one Fuchsia device"
                )
            fd = self.fuchsia_devices[0]
            assert isinstance(fd, device_type)
            return fd, FuchsiaWlanDevice(fd)

        raise signals.TestAbortClass(
            f"Invalid device_type specified: {device_type.__name__}. "
            "Expected FuchsiaDevice."
        )

    def write_to_device_logs(
        self, message: str, level: custom_types.LEVEL
    ) -> None:
        """Writes a message to device logs."""
        for fd in self.fuchsia_devices:
            try:
                fuchsia_async_extension.get_loop().run_until_complete(
                    fd.honeydew_fd.log_message_to_device(
                        message=message, level=level
                    )
                )
            except Exception as err:  # pylint: disable=broad-except
                _LOGGER.exception(
                    "Unable to log message '%s' on '%s'. Failed with error: %s",
                    message,
                    fd.name,
                    err,
                )

    def download_logs(self) -> None:
        """Downloads the DHCP and hostapd logs from the access_point.

        Using the current TestClassContext and TestCaseContext this method pulls
        the DHCP and hostapd logs and outputs them to the correct path.
        """
        current_path = context.get_current_context().get_full_output_path()
        if hasattr(self, "access_points"):
            for access_point in self.access_points:
                access_point.download_ap_logs(current_path)
        if hasattr(self, "openwrt_aps"):
            for openwrt_ap in self.openwrt_aps:
                openwrt_ap.download_logs(current_path)
        if hasattr(self, "iperf_servers"):
            for iperf_server in self.iperf_servers:
                iperf_server.download_logs(current_path)
