# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for async knobs that control a device."""

import abc
from collections.abc import Awaitable, Callable

import fuchsia_inspect
from honeydew.affordances import location
from honeydew.affordances.connectivity import netstack
from honeydew.affordances.connectivity.bluetooth.avrcp import avrcp
from honeydew.affordances.connectivity.bluetooth.gap import gap
from honeydew.affordances.connectivity.bluetooth.le import le
from honeydew.affordances.connectivity.wlan import core as wlan_core
from honeydew.affordances.connectivity.wlan import wlan_policy, wlan_policy_ap
from honeydew.affordances.hello_world import hello_world
from honeydew.affordances.power.system_power_state_controller import (
    system_power_state_controller,
)
from honeydew.affordances.rtc import rtc
from honeydew.affordances.session import session
from honeydew.affordances.starnix import starnix
from honeydew.affordances.tracing import tracing
from honeydew.affordances.ui.screenshot import screenshot
from honeydew.affordances.ui.user_input import user_input
from honeydew.affordances.virtual_audio import audio
from honeydew.transports.adb import adb as adb_transport
from honeydew.transports.fastboot import fastboot as fastboot_transport
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fuchsia_controller_transport,
)
from honeydew.transports.serial import serial as serial_transport
from honeydew.transports.sl4f import sl4f as sl4f_transport
from honeydew.typing import custom_types
from honeydew.utils import properties


class DeviceKnobs(abc.ABC):
    """Abstract base class for async knobs that control a device.

    This class contains abstract methods that are supported by every device
    running Fuchsia irrespective of the device type.
    """

    # List all the persistent properties
    @properties.PersistentProperty
    @abc.abstractmethod
    def board(self) -> str:
        """Returns the board value of the device.

        Returns:
            board value of the device.
        """

    @properties.PersistentProperty
    @abc.abstractmethod
    def device_name(self) -> str:
        """Returns the name of the device.

        Returns:
            Name of the device.
        """

    @properties.persistent_method
    @abc.abstractmethod
    async def manufacturer(self) -> str:
        """Returns the manufacturer of the device.

        Returns:
            Manufacturer of the device.
        """

    @properties.persistent_method
    @abc.abstractmethod
    async def model(self) -> str:
        """Returns the model of the device.

        Returns:
            Model of the device.
        """

    @properties.PersistentProperty
    @abc.abstractmethod
    def product(self) -> str:
        """Returns the product value of the device.

        Returns:
            product value of the device.
        """

    @properties.persistent_method
    @abc.abstractmethod
    async def product_name(self) -> str:
        """Returns the product name of the device.

        Returns:
            Product name of the device.
        """

    @properties.persistent_method
    @abc.abstractmethod
    async def serial_number(self) -> str:
        """Returns the serial number of the device.

        Returns:
            Serial number of the device.
        """

    # List all the dynamic properties
    @abc.abstractmethod
    async def firmware_version(self) -> str:
        """Returns the firmware version of the device.

        Returns:
            Firmware version of the device.
        """

    @abc.abstractmethod
    async def last_reboot_reason(self) -> str:
        """Returns the last reboot reason of the device.

        Returns:
            Last reboot reason of the device.
        """

    # List all the transports
    @properties.Transport
    @abc.abstractmethod
    def adb(self) -> adb_transport.Adb:
        """Returns the ADB transport object.

        Returns:
            ADB transport interface implementation.

        Raises:
            errors.AdbError: Failed to instantiate.
        """

    @properties.Transport
    @abc.abstractmethod
    def fastboot(self) -> fastboot_transport.Fastboot:
        """Returns the Fastboot transport object.

        Returns:
            Fastboot object.

        Raises:
            errors.FuchsiaDeviceError: Failed to instantiate.
        """

    @properties.Transport
    @abc.abstractmethod
    def ffx(self) -> ffx_transport.FFX:
        """Returns the FFX transport object.

        Returns:
            FFX object.

        Raises:
            FfxCommandError: Failed to instantiate.
        """

    @properties.Transport
    @abc.abstractmethod
    def fuchsia_controller(
        self,
    ) -> fuchsia_controller_transport.FuchsiaController:
        """Returns the Fuchsia-Controller transport object.

        Returns:
            Fuchsia-Controller transport object.

        Raises:
            errors.FuchsiaControllerError: Failed to instantiate.
        """

    @properties.Transport
    @abc.abstractmethod
    def serial(self) -> serial_transport.Serial:
        """Returns the Serial transport object.

        Returns:
            Serial transport object.
        """

    @properties.Transport
    @abc.abstractmethod
    def sl4f(self) -> sl4f_transport.SL4F:
        """Returns the SL4F transport object.

        Returns:
            SL4F object.

        Raises:
            errors.Sl4fError: Failed to instantiate.
        """

    # List all the affordances
    @properties.Affordance
    @abc.abstractmethod
    def bluetooth_avrcp(self) -> avrcp.Avrcp:
        """Returns a Bluetooth Avrcp affordance object.

        Returns:
            Bluetooth Avrcp object
        """

    @properties.Affordance
    @abc.abstractmethod
    def bluetooth_gap(self) -> gap.Gap:
        """Returns a Bluetooth Gap affordance object.

        Returns:
            Bluetooth Gap object
        """

    @properties.Affordance
    @abc.abstractmethod
    def bluetooth_le(self) -> le.LE:
        """Returns a Bluetooth LE affordance object.

        Returns:
            Bluetooth LE object
        """

    @properties.Affordance
    @abc.abstractmethod
    def hello_world(self) -> hello_world.HelloWorld:
        """Returns a HelloWorld affordance object.

        Returns:
            hello_world.HelloWorld object
        """

    @properties.Affordance
    @abc.abstractmethod
    def rtc(self) -> rtc.Rtc:
        """Returns an RTC affordance object.

        Returns:
            rtc.Rtc object
        """

    @properties.Affordance
    @abc.abstractmethod
    def screenshot(self) -> screenshot.Screenshot:
        """Returns a screenshot affordance object.

        Returns:
            screenshot.Screenshot object
        """

    @properties.Affordance
    @abc.abstractmethod
    def virtual_audio(self) -> audio.VirtualAudio:
        """Returns an audio affordance object.

        Returns:
            audio.Audio object
        """

    @properties.Affordance
    @abc.abstractmethod
    def session(self) -> session.Session:
        """Returns a session affordance object.

        Returns:
            session.Session object
        """

    @properties.Affordance
    @abc.abstractmethod
    def starnix(self) -> starnix.Starnix:
        """Returns a starnix affordance object.

        Returns:
            starnix.Starnix object
        """

    @properties.Affordance
    @abc.abstractmethod
    def system_power_state_controller(
        self,
    ) -> system_power_state_controller.SystemPowerStateController:
        """Returns a SystemPowerStateController affordance object.

        Returns:
            system_power_state_controller.SystemPowerStateController object

        Raises:
            errors.NotSupportedError: If Fuchsia device does not support Starnix
        """

    @properties.Affordance
    @abc.abstractmethod
    def tracing(self) -> tracing.Tracing:
        """Returns a tracing affordance object.

        Returns:
            tracing.Tracing object
        """

    @properties.Affordance
    @abc.abstractmethod
    def user_input(self) -> user_input.UserInput:
        """Returns a user_input affordance object.

        Returns:
            user_input.UserInput object
        """

    @properties.Affordance
    @abc.abstractmethod
    def wlan_policy(self) -> wlan_policy.WlanPolicy:
        """Returns a WlanPolicy affordance object.

        Returns:
            wlan_policy.WlanPolicy object
        """

    @properties.Affordance
    @abc.abstractmethod
    def wlan_policy_ap(self) -> wlan_policy_ap.WlanPolicyAp:
        """Returns a WlanPolicyAp affordance object.

        Returns:
            wlan_policy_ap.WlanPolicyAp object
        """

    @properties.Affordance
    @abc.abstractmethod
    def wlan_core(self) -> wlan_core.WlanCore:
        """Returns a Wlan affordance object.

        Returns:
            wlan.Wlan object
        """

    @properties.Affordance
    @abc.abstractmethod
    def netstack(self) -> netstack.Netstack:
        """Returns a netstack affordance object.

        Returns:
            netstack.Netstack object
        """

    @properties.Affordance
    @abc.abstractmethod
    def location(self) -> location.Location:
        """Returns a Location affordance object.

        Returns:
            location.Location object
        """

    # List all the public methods
    @abc.abstractmethod
    async def close(self) -> None:
        """Clean up method."""

    @abc.abstractmethod
    def health_check(self) -> None:
        """Ensure device is healthy.

        Raises:
            errors.HealthCheckError
        """

    @abc.abstractmethod
    def get_inspect_data(
        self,
        selectors: list[str] | None = None,
        monikers: list[str] | None = None,
    ) -> fuchsia_inspect.InspectDataCollection:
        """Return the inspect data associated with the given selectors and
        monikers.

        Args:
            selectors: selectors to be queried.
            monikers: component monikers.

        Note: If both `selectors` and `monikers` lists are empty, inspect data
        for the whole system will be returned.

        Returns:
            Inspect data collection

        Raises:
            InspectError: Failed to return inspect data.
        """

    @abc.abstractmethod
    async def log_message_to_device(
        self, message: str, level: custom_types.LEVEL
    ) -> None:
        """Log message to fuchsia device at specified level.

        Args:
            message: Message that need to logged.
            level: Log message level.
        """

    @abc.abstractmethod
    async def power_cycle(self) -> None:
        """Power cycle (power off, wait for delay, power on) the device.

        Requires a power switch to be configured for the device.

        Raises:
            NotSupportedError: If power switch is not configured.
        """

    @abc.abstractmethod
    def register_for_on_device_close(
        self, fn: Callable[[], None] | Callable[[], Awaitable[None]]
    ) -> None:
        """Register a function that will be called during device clean up in `close()`.

        Args:
            fn: Function that need to be called during FuchsiaDevice cleanup.
        """

    @abc.abstractmethod
    def register_for_on_device_ip_change(
        self,
        fn: Callable[[custom_types.IpPort], None]
        | Callable[[custom_types.IpPort], Awaitable[None]],
    ) -> None:
        """Register a function that will be called when an IP address is changed.

        Args:
            fn: Function that need to be called when an IP address is changed.
        """

    @abc.abstractmethod
    async def resolve_device_ip(self) -> None:
        """Resolves the IP address of Fuchsia device."""

    @abc.abstractmethod
    async def snapshot(
        self,
        directory: str,
        snapshot_file: str | None = None,
    ) -> str:
        """Captures the snapshot of the device.

        Args:
            directory: Absolute path on the host where snapshot file need
                to be saved.

            snapshot_file: Name of the file to be used to save snapshot file.
                If not provided, API will create a name using
                "Snapshot_{device_name}_{'%Y-%m-%d-%I-%M-%S-%p'}" format.

        Returns:
            Absolute path of the snapshot file.
        """

    @abc.abstractmethod
    def is_starnix_device(self) -> bool:
        """Check if the device under test is a starnix device.

        Some operation maybe heavy on starnix device, allow caller to find if running on starnix
        device.

        Raises:
            FuchsiaDeviceError: failed to check the device.
        """
