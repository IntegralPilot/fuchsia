# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""FuchsiaDevice abstract base class implementation."""


import asyncio
import dataclasses
import inspect
import ipaddress
import json
import logging
import os
import subprocess
from collections.abc import Awaitable, Callable, Mapping
from datetime import datetime
from functools import cached_property
from typing import Any

import fidl_fuchsia_buildinfo as f_buildinfo
import fidl_fuchsia_developer_remotecontrol as fd_remotecontrol
import fidl_fuchsia_diagnostics_types as f_diagnostics_types
import fidl_fuchsia_feedback as f_feedback
import fidl_fuchsia_hardware_power_statecontrol as fhp_statecontrol
import fidl_fuchsia_hwinfo as f_hwinfo
import fidl_fuchsia_io as f_io
import fuchsia_controller_py as fcp
import fuchsia_inspect
import honeydew.affordances.connectivity.wlan.core as wlan_core
from honeydew import affordances_capable, errors
from honeydew.affordances import location
from honeydew.affordances.connectivity import netstack
from honeydew.affordances.connectivity.bluetooth.avrcp import (
    avrcp,
    avrcp_using_sl4f,
)
from honeydew.affordances.connectivity.bluetooth.gap import gap, gap_using_fc
from honeydew.affordances.connectivity.bluetooth.le import le, le_using_fc
from honeydew.affordances.connectivity.bluetooth.utils import (
    types as bluetooth_types,
)
from honeydew.affordances.connectivity.wlan import wlan_policy, wlan_policy_ap
from honeydew.affordances.device_knobs import device_knobs
from honeydew.affordances.drivers import battery_manager
from honeydew.affordances.drivers.fake_battery import (
    fake_battery,
    fake_battery_using_ffx,
)
from honeydew.affordances.drivers.hardware import power as hardware_power
from honeydew.affordances.hello_world import hello_world, hello_world_using_ffx
from honeydew.affordances.media import media, media_using_fc
from honeydew.affordances.power.system_power_state_controller import (
    system_power_state_controller as system_power_state_controller_interface,
)
from honeydew.affordances.power.system_power_state_controller import (
    system_power_state_controller_using_starnix,
)
from honeydew.affordances.rtc import rtc, rtc_using_fc
from honeydew.affordances.session import session, session_using_ffx
from honeydew.affordances.starnix import errors as starnix_errors
from honeydew.affordances.starnix import starnix, starnix_using_ffx
from honeydew.affordances.tracing import (
    tracing,
    tracing_using_fc,
    tracing_using_ffx,
)
from honeydew.affordances.tracing import types as tracing_types
from honeydew.affordances.ui.screenshot import screenshot, screenshot_using_ffx
from honeydew.affordances.ui.user_input import user_input, user_input_using_fc
from honeydew.affordances.virtual_audio import (
    audio,
    audio_using_fuchsia_controller,
)
from honeydew.auxiliary_devices.power_switch import (
    power_switch as power_switch_interface,
)
from honeydew.auxiliary_devices.usb_power_hub import (
    usb_power_hub as usb_power_hub_interface,
)
from honeydew.transports.adb import adb as adb_transport
from honeydew.transports.fastboot import fastboot
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx
from honeydew.transports.ffx.config import FfxConfigData
from honeydew.transports.fuchsia_controller import errors as fc_errors
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.transports.serial import serial as serial_transport_interface
from honeydew.transports.serial import serial_using_unix_socket
from honeydew.transports.sl4f import sl4f as sl4f_transport
from honeydew.typing import custom_types
from honeydew.utils import common, properties

_FC_PROXIES: dict[str, custom_types.FidlEndpoint] = {
    "BuildInfo": custom_types.FidlEndpoint(
        "/core/build-info", "fuchsia.buildinfo.Provider"
    ),
    "DeviceInfo": custom_types.FidlEndpoint(
        "/core/hwinfo", "fuchsia.hwinfo.Device"
    ),
    "Feedback": custom_types.FidlEndpoint(
        "/core/feedback", "fuchsia.feedback.DataProvider"
    ),
    "LastRebootInfo": custom_types.FidlEndpoint(
        "/core/feedback", "fuchsia.feedback.LastRebootInfoProvider"
    ),
    "ProductInfo": custom_types.FidlEndpoint(
        "/core/hwinfo", "fuchsia.hwinfo.Product"
    ),
    "PowerAdmin": custom_types.FidlEndpoint(
        "/bootstrap/shutdown_shim", "fuchsia.hardware.power.statecontrol.Admin"
    ),
    "RemoteControl": custom_types.FidlEndpoint(
        "/core/remote-control", "fuchsia.developer.remotecontrol.RemoteControl"
    ),
}

_FFX_CMDS: dict[str, list[str]] = {
    "RESOLVE_IP": [
        "target",
        "list",
        "--no-probe",  # do not connect to targets
    ],
}

_LOG_SEVERITIES: dict[custom_types.LEVEL, f_diagnostics_types.Severity] = {
    custom_types.LEVEL.INFO: f_diagnostics_types.Severity.INFO,
    custom_types.LEVEL.WARNING: f_diagnostics_types.Severity.WARN,
    custom_types.LEVEL.ERROR: f_diagnostics_types.Severity.ERROR,
}

_LOGGER: logging.Logger = logging.getLogger(__name__)

_REBOOT_OFFLINE_TIMEOUT_SEC: int = 60


class FuchsiaDevice(
    device_knobs.DeviceKnobs,
    affordances_capable.RebootCapableDevice,
    affordances_capable.FuchsiaDeviceLogger,
    affordances_capable.FuchsiaDeviceClose,
    affordances_capable.InspectCapableDevice,
    affordances_capable.FuchsiaDeviceIpChange,
):
    """Class that provides access to an assortment of capabilities available
    on a Fuchsia device.

    Args:
        device_info: Fuchsia device information.
        ffx_config_data: Config that need to be used while running FFX commands.
        config: Honeydew device configuration, if any.
            Format:
                {
                    "transports": {
                        <transport_name>: {
                            <key>: <value>,
                            ...
                        },
                        ...
                    },
                    "affordances": {
                        <affordance_name>: {
                            <key>: <value>,
                            ...
                        },
                        ...
                    },
                }
            Example:
                {
                    "transports": {
                        "fuchsia_controller": {
                            "timeout": 30,
                        }
                    },
                    "affordances": {
                        "bluetooth": {
                            "implementation": "fuchsia-controller",
                        },
                    },
                }

    Raises:
        FFXCommandError: if FFX connection check fails.
        FuchsiaControllerError: if FC connection check fails.
    """

    def __init__(
        self,
        *,
        device_info: custom_types.DeviceInfo,
        ffx_config_data: FfxConfigData,
        # intentionally made this a Dict instead of dataclass to minimize the changes in remaining Lacewing stack every time we need to add a new configuration item
        config: dict[str, Any] | None = None,
        environ: Mapping[str, str] | None = None,
    ) -> None:
        _LOGGER.debug("Initializing FuchsiaDevice")
        if environ is None:
            environ = os.environ

        self._device_info: custom_types.DeviceInfo = device_info

        # Track if the device was created with a statically provided IP (infra).
        self._is_static_ip: bool = device_info.ip_port is not None and (
            environ.get("BOTANIST_CONFIG") is not None
            or environ.get("SWARMING_TASK_ID") is not None
        )

        self._ffx_config_data: FfxConfigData = ffx_config_data

        self._on_device_boot_fns: list[
            Callable[[], None] | Callable[[], Awaitable[None]]
        ] = []
        self._on_device_close_fns: list[
            Callable[[], None] | Callable[[], Awaitable[None]]
        ] = []
        self._on_device_ip_change_fns: list[
            Callable[[custom_types.IpPort], None]
            | Callable[[custom_types.IpPort], Awaitable[None]]
        ] = []
        self._on_device_suspend_fns: list[
            Callable[[], None] | Callable[[], Awaitable[None]]
        ] = []
        self._on_device_resume_fns: list[
            Callable[[], None] | Callable[[], Awaitable[None]]
        ] = []
        self._pre_suspend_boot_id: str | None = None

        self._config: dict[str, Any] | None = config
        self._created_context = False

        self._power_switch: power_switch_interface.PowerSwitch | None = None
        self._power_switch_outlet: int | None = None

        self._usb_power_hub: usb_power_hub_interface.UsbPowerHub | None = None
        self._usb_power_hub_port: int | None = None

        self.health_check()

        _LOGGER.debug("Initialized FuchsiaDevice")

    # List all the persistent properties
    @properties.PersistentProperty
    def board(self) -> str:
        """Returns the board value of the device.

        Returns:
            board value of the device.

        Raises:
            FfxCommandError: On failure.
        """
        return self.ffx.get_target_board()

    @properties.PersistentProperty
    def device_name(self) -> str:
        """Returns the name of the device.

        Returns:
            Name of the device.
        """
        return self._device_info.name

    @property
    def is_static_ip(self) -> bool:
        """Returns True if the device was created with a statically provided IP."""
        return self._is_static_ip

    async def manufacturer(self) -> str:
        """Returns the manufacturer of the device, cached after the first retrieval.

        Returns:
            Manufacturer of device.

        Raises:
            FuchsiaDeviceError: On failure.
        """
        return (await self._product_info())["manufacturer"]

    async def model(self) -> str:
        """Returns the model of the device, cached after the first retrieval.

        Returns:
            Model of device.

        Raises:
            FuchsiaDeviceError: On failure.
        """
        return (await self._product_info())["model"]

    @properties.PersistentProperty
    def product(self) -> str:
        """Returns the product value of the device.

        Returns:
            product value of the device.

        Raises:
            FfxCommandError: On failure.
        """
        return self.ffx.get_target_product()

    async def product_name(self) -> str:
        """Returns the product name of the device, cached after the first retrieval.

        Returns:
            Product name of the device.

        Raises:
            FuchsiaDeviceError: On failure.
        """
        return (await self._product_info())["name"]

    async def serial_number(self) -> str:
        """Returns the serial number of the device, cached after the first retrieval.

        Returns:
            Serial number of the device.
        """
        return (await self._device_info_from_fidl())["serial_number"]

    # List all the dynamic properties
    async def firmware_version(self) -> str:
        """Returns the firmware version of the device.

        Returns:
            Firmware version of the device.
        """
        return (await self._build_info())["version"]

    async def last_reboot_reason(self) -> str:
        """Returns the last reboot reason of the device.

        Returns:
            Last reboot reason of the device. Empty string if it doesn't exist.
        """
        reason = (await self._last_reboot_info())["reason"]
        if reason is None:
            return ""
        return f_feedback.RebootReason(reason).name

    # List all transports
    @properties.Transport
    def ffx(self) -> ffx.FFX:
        """Returns the FFX transport object.

        Returns:
            FFX transport interface implementation.

        Raises:
            FfxCommandError: Failed to instantiate.
        """
        use_monitor_state = False
        shared_data = None
        if self._config is not None:
            # Read monitor state
            config_use_monitor_state = common.read_from_dict(
                self._config,
                key_path=("transports", "ffx", "use_monitor_state"),
                should_exist=False,
            )
            if config_use_monitor_state is not None:
                use_monitor_state = config_use_monitor_state
            # Read shared_data path
            config_shared_data = common.read_from_dict(
                self._config,
                key_path=("transports", "ffx", "shared_data"),
                should_exist=False,
            )
            if config_shared_data is not None:
                shared_data = config_shared_data
        query: str = (
            str(self._device_info.ip_port)
            if self._device_info.ip_port
            else self.device_name
        )
        ffx_obj: ffx.FFX = ffx.FFX(
            query=query,
            name=self.device_name,
            config_data=self._ffx_config_data,
            use_monitor_state=use_monitor_state,
            shared_data=shared_data,
            device_ip_change=self,
        )
        return ffx_obj

    @properties.Transport
    def fuchsia_controller(
        self,
    ) -> fc_transport.FuchsiaController:
        """Returns the Fuchsia-Controller transport object.

        Returns:
            Fuchsia-Controller transport interface implementation.

        Raises:
            FuchsiaControllerError: Failed to instantiate.
        """
        fuchsia_controller_obj: fc_transport.FuchsiaController = (
            fc_transport.FuchsiaController(
                target_name=self.device_name,
                ffx_config_data=self._ffx_config_data,
                target_ip_port=self._device_info.ip_port,
                device_ip_change=self,
            )
        )
        return fuchsia_controller_obj

    @properties.Transport
    def adb(self) -> adb_transport.Adb:
        """Returns the ADB transport object.

        Returns:
            ADB transport interface implementation.

        Raises:
            errors.NotEnabledError: If ADB transport is not enabled.
        """
        run_isolated_server = False
        vendor_keys_path = None

        # Note - An existing ADB implementation in //vendor/google is used by
        # some Lacewing tests. Running two ADB server implementations against
        # the same device causes conflicts. Tests that manage ADB externally
        # can set `enabled` to False to disable Honeydew's ADB transport.
        # TODO(b/559563915): Delete this `enabled` config once legacy ADB
        # server implementations are removed and all Lacewing tests use
        # Honeydew's ADB transport.
        enabled = True
        if self._config:
            adb_config = self._config.get("transports", {}).get("adb", {})
            run_isolated_server = adb_config.get("run_isolated_server", False)
            vendor_keys_path = adb_config.get("vendor_keys_path")
            enabled = adb_config.get("enabled", True)

        if not enabled:
            raise errors.NotEnabledError(
                f"ADB transport is not enabled for '{self.device_name}'"
            )

        serial_number = self._device_info.serial_number or self.serial_number
        adb_obj: adb_transport.Adb = adb_transport.Adb(
            device_name=self.device_name,
            serial_number=serial_number,
            run_isolated_server=run_isolated_server,
            vendor_keys_path=vendor_keys_path,
        )
        self.register_for_on_device_close(adb_obj.close)
        return adb_obj

    @properties.Transport
    def fastboot(self) -> fastboot.Fastboot:
        """Returns the Fastboot transport object.

        Returns:
            Fastboot transport interface implementation.

        Raises:
            FuchsiaDeviceError: Failed to instantiate.
        """
        fastboot_obj: fastboot.Fastboot = fastboot.Fastboot(
            device_name=self.device_name,
            reboot_affordance=self,
            ffx_transport=self.ffx,
            fastboot_node_id=self._device_info.fastboot_node_id
            or self._device_info.serial_number,
        )
        return fastboot_obj

    @properties.Transport
    def serial(self) -> serial_transport_interface.Serial:
        """Returns the Serial transport object.

        Returns:
            Serial transport object.
        """
        if self._device_info.serial_socket is None:
            raise errors.FuchsiaDeviceError(
                "'serial_socket' arg need to be provided during the init to use Serial affordance"
            )

        serial_obj: serial_transport_interface.Serial = (
            serial_using_unix_socket.SerialUsingUnixSocket(
                device_name=self.device_name,
                socket_path=self._device_info.serial_socket,
            )
        )
        return serial_obj

    @properties.Transport
    def sl4f(self) -> sl4f_transport.SL4F:
        """Returns the SL4F transport object.

        Returns:
            SL4F transport interface implementation.

        Raises:
            Sl4fError: Failed to instantiate.
        """
        device_ip: ipaddress.IPv4Address | ipaddress.IPv6Address | None = None
        if self._device_info.ip_port:
            device_ip = self._device_info.ip_port.ip

        sl4f_obj: sl4f_transport.SL4F = sl4f_transport.SL4F(
            device_name=self.device_name,
            device_ip=device_ip,
            ffx_transport=self.ffx,
            device_ip_change=self,
        )
        return sl4f_obj

    # List all the affordances
    @properties.Affordance
    def session(self) -> session.Session:
        """Returns a session affordance object.

        Returns:
            session.Session object
        """
        return session_using_ffx.SessionUsingFfx(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_device_close=self,
        )

    @properties.Affordance
    def screenshot(self) -> screenshot.Screenshot:
        """Returns a screenshot affordance object.

        Returns:
            screenshot.Screenshot object
        """
        return screenshot_using_ffx.ScreenshotUsingFfx(self.ffx)

    @properties.Affordance
    def virtual_audio(self) -> audio.VirtualAudio:
        """Returns a virtual audio affordance object.

        Connecting to the protocols this connects to on startup will inject
        the virtual audio device which does the following things:

        - Input audio will only come from the virtual device. Actual microphones are disabled.
        - Output audio will only go to the virtual device. Actual speakers are disabled.

        TODO(https://fxbug.dev/417759272): There is currently no way to disable this
        behavior other than rebooting the device.

        Returns:
            audio.VirtualAudio object
        """
        return (
            audio_using_fuchsia_controller.VirtualAudioUsingFuchsiaController(
                device_name=self.device_name,
                fuchsia_controller=self.fuchsia_controller,
                ffx_transport=self.ffx,
            )
        )

    @properties.Affordance
    def starnix(self) -> starnix.Starnix:
        """Returns a starnix affordance object.

        Returns:
            starnix.Starnix object
        """
        return starnix_using_ffx.StarnixUsingFfx(
            device_name=self.device_name,
            ffx=self.ffx,
        )

    @properties.Affordance
    def system_power_state_controller(
        self,
    ) -> system_power_state_controller_interface.SystemPowerStateController:
        """Returns a SystemPowerStateController affordance object.

        Returns:
            system_power_state_controller_interface.SystemPowerStateController object

        Raises:
            errors.NotSupportedError: If Fuchsia device does not support Starnix
        """
        return system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix(
            device_name=self.device_name,
            ffx=self.ffx,
            inspect=self,
            device_logger=self,
            starnix_affordance=self.starnix,
        )

    @properties.Affordance
    def rtc(self) -> rtc.Rtc:
        """Returns an RTC affordance object.

        Returns:
            rtc.Rtc object
        """
        return rtc_using_fc.RtcUsingFc(
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def tracing(self) -> tracing.Tracing:
        """Returns a tracing affordance object.

        Returns:
            tracing.Tracing object
        """
        impl = self._get_tracing_affordance_implementation()
        if impl == tracing_types.Implementation.FFX:
            return tracing_using_ffx.TracingUsingFfx(
                device_name=self.device_name,
                ffx_inst=self.ffx,
                reboot_affordance=self,
            )
        return tracing_using_fc.TracingUsingFc(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def user_input(self) -> user_input.UserInput:
        """Returns an user input affordance object.

        Returns:
            user_input.UserInput object
        """
        return user_input_using_fc.UserInputUsingFc(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            ffx_transport=self.ffx,
        )

    @properties.Affordance
    def bluetooth_avrcp(self) -> avrcp.Avrcp:
        """Returns a Bluetooth Avrcp affordance object.

        Returns:
            Bluetooth Avrcp object
        """
        if (
            self._get_bluetooth_affordances_implementation()
            == bluetooth_types.Implementation.SL4F
        ):
            return avrcp_using_sl4f.AvrcpUsingSl4f(
                device_name=self.device_name,
                sl4f=self.sl4f,
                reboot_affordance=self,
            )
        raise NotImplementedError

    @properties.Affordance
    def bluetooth_le(self) -> le.LE:
        """Returns a Bluetooth LE affordance object.

        Returns:
            Bluetooth LE object
        """
        return le_using_fc.LEUsingFc(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def bluetooth_gap(self) -> gap.Gap:
        """Returns a Bluetooth Gap affordance object.

        Returns:
            Bluetooth Gap object
        """
        return gap_using_fc.GapUsingFc(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def wlan_policy(self) -> wlan_policy.WlanPolicy:
        """Returns a wlan_policy affordance object.

        Returns:
            wlan_policy.WlanPolicy object
        """
        return wlan_policy.WlanPolicy(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
            fuchsia_device_close=self,
            location=self.location,
        )

    @properties.Affordance
    def wlan_policy_ap(self) -> wlan_policy_ap.WlanPolicyAp:
        """Returns a wlan_policy_ap affordance object.

        Returns:
            wlan_policy_ap.WlanPolicyAp object
        """
        return wlan_policy_ap.WlanPolicyAp(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
            fuchsia_device_close=self,
        )

    @properties.Affordance
    def wlan_core(self) -> wlan_core.WlanCore:
        """Returns a wlan affordance object.

        Returns:
            wlan.WlanCore object
        """
        return wlan_core.WlanCore(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
            fuchsia_device_close=self,
        )

    @properties.Affordance
    def battery(self) -> battery_manager.Battery:
        """Returns a battery affordance object.

        Returns:
            battery_manager.Battery object
        """
        return battery_manager.Battery(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
            fuchsia_device_close=self,
        )

    @properties.Affordance
    def battery_manager(self) -> battery_manager.Battery:
        """Returns a battery_manager affordance object.

        Returns:
            battery_manager.Battery object
        """
        return self.battery

    @properties.Affordance
    def fake_battery(self) -> fake_battery.FakeBattery:
        """Returns a fake_battery affordance object.

        Returns:
            fake_battery.FakeBattery object
        """
        return fake_battery_using_ffx.FakeBatteryUsingFfx(
            device_name=self.device_name,
            ffx=self.ffx,
        )

    @properties.Affordance
    def power_source(self) -> hardware_power.PowerSource:
        """Returns a power_source affordance object.

        Returns:
            hardware_power.PowerSource object
        """
        return hardware_power.PowerSource(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
            fuchsia_device_close=self,
        )

    @properties.Affordance
    def netstack(self) -> netstack.Netstack:
        """Returns a netstack affordance object.

        Returns:
            netstack.Netstack object
        """
        return netstack.Netstack(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def location(self) -> location.Location:
        """Returns a location affordance object.

        Returns:
            location.Location object
        """
        return location.Location(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def media(self) -> media.Media:
        """Returns a media affordance object.

        Returns:
            media.Media object
        """
        return media_using_fc.MediaUsingFc(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            ffx_transport=self.ffx,
        )

    @properties.Affordance
    def hello_world(self) -> hello_world.HelloWorld:
        """Returns a HelloWorld affordance object.

        Returns:
            hello_world.HelloWorld object
        """
        return hello_world_using_ffx.HelloWorldUsingFfx(
            device_name=self.device_name,
            ffx=self.ffx,
        )

    # List all the public methods
    async def close(self) -> None:
        """Clean up method."""
        for fn in self._on_device_close_fns:
            _LOGGER.info("Calling %s", fn.__qualname__)
            res = fn()
            if inspect.isawaitable(res):
                await res

    def health_check(self) -> None:
        """Ensure device is healthy.

        Raises:
            errors.HealthCheckError
        """
        try:
            # TODO(b/421476805): Reduce the timeout back to 60 seconds once
            # we're able to supply a configuration value for this. The timeout
            # of 180 is picked to give a sufficiently large amount of time for
            # devices in infrastructure that currently experience stalls during
            # boot but eventually settle.
            with common.time_limit(
                timeout=600,
                exception_message=f"Timeout occurred during the health check of '{self._device_info.name}'",
            ):
                _LOGGER.info(
                    "Starting the health check on %s...",
                    self.device_name,
                )

                # Note - FFX need to be invoked first before FC as FC depends on the daemon that
                # will be created by FFX
                self.ffx.check_connection()

                self.fuchsia_controller.check_connection()

                if self._is_sl4f_needed:
                    self.sl4f.check_connection()

                _LOGGER.info(
                    "Completed the health check successfully on %s...",
                    self.device_name,
                )
        except (
            errors.HoneydewTimeoutError,
            errors.TransportConnectionError,
        ) as err:
            # LINT.IfChange
            raise errors.HealthCheckError(
                f"health check failed on '{self._device_info.name}'"
            ) from err
            # LINT.ThenChange(//tools/testing/tefmocheck/string_in_log_check.go)

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
        selectors_and_monikers: list[str] = []
        if selectors:
            selectors_and_monikers += selectors
        if monikers:
            for moniker in monikers:
                selectors_and_monikers.append(moniker.replace(":", r"\:"))

        cmd: list[str] = [
            "--machine",
            "json",
            "inspect",
            "show",
        ] + selectors_and_monikers

        try:
            message: str = (
                f"Collecting the inspect data from {self.device_name}"
            )
            if selectors:
                message = f"{message}, with selectors={selectors}"
            if monikers:
                message = f"{message}, with monikers={monikers}"
            _LOGGER.info(message)
            inspect_data_json_str: str = self.ffx.run(
                cmd=cmd,
                log_output=False,
            )
            _LOGGER.info(
                "Collected the inspect data from %s.", self.device_name
            )

            inspect_data_json_obj: list[dict[str, Any]] = json.loads(
                inspect_data_json_str
            )
            return fuchsia_inspect.InspectDataCollection.from_list(
                inspect_data_json_obj
            )
        except (
            ffx_errors.FfxCommandError,
            errors.DeviceNotConnectedError,
            fuchsia_inspect.InspectDataError,
        ) as err:
            raise errors.InspectError(
                f"Failed to collect the inspect data from {self.device_name}"
            ) from err

    async def log_message_to_device(
        self, message: str, level: custom_types.LEVEL
    ) -> None:
        """Log message to fuchsia device at specified level.

        Args:
            message: Message that need to logged.
            level: Log message level.

        Raises:
            FuchsiaControllerError: On communications failure.
            Sl4fError: On communications failure.
        """
        timestamp: str = datetime.now().strftime("%Y-%m-%d-%I-%M-%S-%p")
        message = f"[Host Time: {timestamp}] - {message}"
        await self._send_log_command(
            tag="lacewing", message=message, level=level
        )

    async def on_device_boot(self) -> None:
        """Take actions after the device is rebooted.

        Raises:
            FuchsiaControllerError: On communications failure.
            Sl4fError: On communications failure.
        """
        # Restart the SL4F server on device boot up.
        if self._is_sl4f_needed:
            await common.retry(fn=self.sl4f.start_server, wait_time=5)

        # Create a new Fuchsia controller context for new device connection.
        self.fuchsia_controller.create_context()

        # Ensure device is healthy
        self.health_check()

        for fn in self._on_device_boot_fns:
            _LOGGER.info("Calling %s", fn.__qualname__)
            res = fn()
            if inspect.isawaitable(res):
                await res

    def set_power_switch(
        self,
        power_switch: power_switch_interface.PowerSwitch,
        outlet: int | None = None,
    ) -> None:
        """Set power switch for device.

        Args:
            power_switch: Implementation of PowerSwitch interface.
            outlet (int | None): If required by power switch hardware, outlet on
                power switch hardware where this fuchsia device is connected.
        """
        self._power_switch = power_switch
        self._power_switch_outlet = outlet

    async def power_cycle(self) -> None:
        """Power cycle (power off, wait for delay, power on) the device.

        Requires power switch to be set using `set_power_switch`.

        Raises:
            NotSupportedError: If power switch is not set.
            FuchsiaControllerError: On communications failure.
            Sl4fError: On communications failure.
        """
        if self._power_switch is None:
            raise errors.NotSupportedError(
                f"Cannot power cycle '{self.device_name}': Power switch is not configured. "
                "Either call `set_power_switch()` or ensure power switch details are provided in the testbed config."
            )

        _LOGGER.info("Power cycling %s...", self.device_name)

        try:
            await self.log_message_to_device(
                message=f"Powering cycling {self.device_name}...",
                level=custom_types.LEVEL.INFO,
            )
        except Exception:  # pylint: disable=broad-except
            # power_cycle can be used as a recovery mechanism when device is
            # unhealthy. So any calls to device prior to power_cycle can
            # fail in such cases and thus ignore them.
            pass

        _LOGGER.info("Powering off %s...", self.device_name)
        self.ffx.notify_intentional_disconnect()
        self._power_switch.power_off(self._power_switch_outlet)
        await asyncio.to_thread(self.wait_for_offline)

        _LOGGER.info("Powering on %s...", self.device_name)
        self._power_switch.power_on(self._power_switch_outlet)
        await self.wait_for_online()

        await self.on_device_boot()

        await self.log_message_to_device(
            message=f"Successfully power cycled {self.device_name}...",
            level=custom_types.LEVEL.INFO,
        )

    def register_on_device_suspend_fn(
        self,
        fn: Callable[[], None] | Callable[[], Awaitable[None]],
    ) -> None:
        """Register a function to be called when device is suspended.

        Args:
            fn: Function to be called when device is suspended.
        """
        self._on_device_suspend_fns.append(fn)

    def register_on_device_resume_fn(
        self,
        fn: Callable[[], None] | Callable[[], Awaitable[None]],
    ) -> None:
        """Register a function to be called when device is resumed.

        Args:
            fn: Function to be called when device is resumed.
        """
        self._on_device_resume_fns.append(fn)

    @property
    def usb_power_hub(self) -> usb_power_hub_interface.UsbPowerHub | None:
        """USB power hub controlling this device, if one has been set."""
        return self._usb_power_hub

    @property
    def usb_power_hub_port(self) -> int | None:
        """Port on the USB power hub where this device is connected, if any."""
        return self._usb_power_hub_port

    def set_usb_power_hub(
        self,
        usb_power_hub: usb_power_hub_interface.UsbPowerHub,
        port: int | None = None,
    ) -> None:
        """Set USB power hub for device.

        Args:
            usb_power_hub: Implementation of UsbPowerHub interface.
            port (int | None): If required by USB power hub hardware, port on
                USB power hub hardware where this fuchsia device is connected.
        """
        self._usb_power_hub = usb_power_hub
        self._usb_power_hub_port = port

    async def suspend(self) -> None:
        """Suspend the device by disconnecting USB power.

        Requires USB power hub to be set using set_usb_power_hub. This will
        run all registered on_device_suspend_fns before disconnecting USB
        power. Note that this does not guarantee the device actually
        suspends, just that it will have the opportunity to.

        Raises:
            NotSupportedError: If USB power hub not set.
        """
        if self._usb_power_hub is None:
            raise errors.NotSupportedError(
                f"Cannot suspend '{self.device_name}': USB power hub is not configured. "
                "Either call `set_usb_power_hub()` or ensure USB power hub details are provided in the testbed config."
            )

        for fn in self._on_device_suspend_fns:
            res = fn()
            if inspect.isawaitable(res):
                await res

        _LOGGER.info("Disconnecting USB from %s...", self.device_name)
        self._pre_suspend_boot_id = await self.boot_id()

        self.fuchsia_controller.before_usb_disconnect()

        self.ffx.notify_intentional_disconnect()
        self._usb_power_hub.power_off(self._usb_power_hub_port)
        await asyncio.to_thread(self.wait_for_offline)

    async def resume(self) -> None:
        """Resume the device by reconnecting USB power.

        Requires USB power hub to be set using set_usb_power_hub. This will
        run all registered on_device_resume_fns after reconnecting USB power.

        Raises:
            NotSupportedError: If USB power hub not set.
            FuchsiaDeviceError: If unexpected reboot detected.
        """
        if self._usb_power_hub is None:
            raise errors.NotSupportedError(
                f"Cannot resume '{self.device_name}': USB power hub is not configured. "
                "Either call `set_usb_power_hub()` or ensure USB power hub details are provided in the testbed config."
            )

        _LOGGER.info("Connecting USB to %s...", self.device_name)
        self._usb_power_hub.power_on(self._usb_power_hub_port)
        await self.wait_for_online()

        self.fuchsia_controller.after_usb_reconnect()

        post_resume_boot_id = await self.boot_id()
        if self._pre_suspend_boot_id != post_resume_boot_id:
            raise errors.FuchsiaDeviceError(
                f"Unexpected reboot detected for {self.device_name}. Boot ID {self._pre_suspend_boot_id} != {post_resume_boot_id}"
            )

        for fn in self._on_device_resume_fns:
            res = fn()
            if inspect.isawaitable(res):
                await res

    async def reboot(self) -> None:
        """Soft reboot the device.

        Raises:
            FuchsiaControllerError: On communications failure.
            Sl4fError: On communications failure.
            FuchsiaDeviceError: If device fails to go offline.
        """
        _LOGGER.info("Lacewing is rebooting %s...", self.device_name)
        await self.log_message_to_device(
            message=f"Rebooting {self.device_name}...",
            level=custom_types.LEVEL.INFO,
        )

        self.ffx.notify_intentional_disconnect()

        # Start wait for offline in background to avoid race conditions.
        _LOGGER.info(
            "Starting background wait for %s to go offline...", self.device_name
        )
        wait_process = self.ffx.wait_for_rcs_disconnection()
        try:
            await self._send_reboot_command()

            _LOGGER.info(
                "Waiting for background offline process to complete..."
            )
            try:
                await asyncio.to_thread(
                    wait_process.wait, timeout=_REBOOT_OFFLINE_TIMEOUT_SEC
                )
            except subprocess.TimeoutExpired as err:
                raise errors.FuchsiaDeviceError(
                    f"'{self.device_name}' failed to go offline within "
                    f"{_REBOOT_OFFLINE_TIMEOUT_SEC} seconds."
                ) from err
        finally:
            if wait_process.poll() is None:
                wait_process.kill()
                wait_process.wait()
        await self.wait_for_online()
        await self.on_device_boot()

        await self.log_message_to_device(
            message=f"Successfully rebooted {self.device_name}...",
            level=custom_types.LEVEL.INFO,
        )

    def register_for_on_device_boot(
        self, fn: Callable[[], None] | Callable[[], Awaitable[None]]
    ) -> None:
        """Register a function that will be called in `on_device_boot()`.

        Args:
            fn: Function that need to be called after FuchsiaDevice boot up.
        """
        self._on_device_boot_fns.append(fn)

    def register_for_on_device_close(
        self, fn: Callable[[], None] | Callable[[], Awaitable[None]]
    ) -> None:
        """Register a function that will be called during device clean up in `close()`.

        Args:
            fn: Function that need to be called during FuchsiaDevice cleanup.
        """
        self._on_device_close_fns.append(fn)

    async def resolve_device_ip(self) -> None:
        """Resolves the IP address of Fuchsia device."""
        # Step #1 - Get the IP address of the Fuchsia device.
        if self._device_info.ip_port is not None:
            _LOGGER.info(
                "'%s' is the IP address of '%s', prior to resolving device ip",
                self._device_info.ip_port,
                self.device_name,
            )
        try:
            cmd: list[str] = _FFX_CMDS["RESOLVE_IP"] + [self.device_name]
            output: str = self.ffx.run(
                cmd=cmd,
                include_target=False,
            )
            targets = json.loads(output)
            if not targets:
                raise ffx_errors.FfxCommandError(
                    f"Target '{self.device_name}' not found in 'ffx target list'"
                )
            target = targets[0]
            if not target.get("addresses"):
                raise ffx_errors.FfxCommandError(
                    f"No addresses found for target '{self.device_name}'"
                )
            address = target["addresses"][0]
            target_addr = custom_types.TargetAddr.from_json(address)

            if isinstance(target_addr, custom_types.IpPort):
                ip_port: custom_types.IpPort = target_addr
                self._device_info = dataclasses.replace(
                    self._device_info,
                    ip_port=ip_port,
                )
                if self._device_info.ip_port is not None:
                    _LOGGER.info(
                        "'%s' is the IP address of '%s', after resolving device ip",
                        self._device_info.ip_port,
                        self.device_name,
                    )

                # Step #2 - Call all of the callback functions that were registered for IP address change.
                for fn in self._on_device_ip_change_fns:
                    _LOGGER.info(
                        "Calling %s with arg %s",
                        fn.__qualname__,
                        ip_port,
                    )
                    res = fn(ip_port)
                    if inspect.isawaitable(res):
                        await res
            elif isinstance(target_addr, custom_types.TargetUsb):
                self.ffx.resolve_target_address()
                self.fuchsia_controller.after_usb_reconnect()
        except Exception as err:
            raise errors.FuchsiaDeviceError(
                f"Failed to resolve IP for '{self.device_name}'"
            ) from err

        # Step #3 - Ensure device is healthy
        self.health_check()

    def register_for_on_device_ip_change(
        self,
        fn: (
            Callable[[custom_types.IpPort], None]
            | Callable[[custom_types.IpPort], Awaitable[None]]
        ),
    ) -> None:
        """Register a function that will be called when an IP address is changed.

        Args:
            fn: Function that need to be called when an IP address is changed.
        """
        self._on_device_ip_change_fns.append(fn)

    async def snapshot(
        self,
        directory: str,
        snapshot_file: str | None = None,
    ) -> str:
        """Captures the snapshot of the device.

        Args:
            directory: Absolute path on the host where snapshot file will be
                saved. If this directory does not exist, this method will create
                it.

            snapshot_file: Name of the output snapshot file.
                If not provided, API will create a name using
                "Snapshot_{device_name}_{'%Y-%m-%d-%I-%M-%S-%p'}" format.

        Returns:
            Absolute path of the snapshot file.

        Raises:
            FuchsiaControllerError: On communications failure.
            Sl4fError: On communications failure.
        """
        _LOGGER.info("Collecting snapshot on %s...", self.device_name)
        # Take the snapshot before creating the directory or file, as
        # _send_snapshot_command may raise an exception.
        snapshot_bytes: bytes = await self._send_snapshot_command()

        directory = os.path.abspath(directory)
        try:
            os.makedirs(directory)
        except FileExistsError:
            pass

        if not snapshot_file:
            timestamp: str = datetime.now().strftime("%Y-%m-%d-%I-%M-%S-%p")
            snapshot_file = f"Snapshot_{self.device_name}_{timestamp}.zip"
        snapshot_file_path: str = os.path.join(directory, snapshot_file)

        with open(snapshot_file_path, "wb") as snapshot_binary_zip:
            snapshot_binary_zip.write(snapshot_bytes)

        _LOGGER.info("Snapshot file has been saved @ '%s'", snapshot_file_path)
        return snapshot_file_path

    def wait_for_offline(self) -> None:
        """Wait for Fuchsia device to go offline.

        Raises:
            errors.FuchsiaDeviceError: If device is not offline.
        """
        _LOGGER.info("Waiting for %s to go offline...", self.device_name)
        try:
            wait_process = self.ffx.wait_for_rcs_disconnection()
            try:
                wait_process.wait(timeout=60)
            except subprocess.TimeoutExpired as err:
                raise errors.FuchsiaDeviceError(
                    f"'{self.device_name}' failed to go offline within 60 seconds."
                ) from err
            finally:
                if wait_process.poll() is None:
                    wait_process.kill()
                    wait_process.wait()
            _LOGGER.info("%s is offline.", self.device_name)
        except (
            errors.DeviceNotConnectedError,
            ffx_errors.FfxCommandError,
        ) as err:
            raise errors.FuchsiaDeviceError(
                f"'{self.device_name}' failed to go offline."
            ) from err

    async def wait_for_online(self) -> None:
        """Wait for Fuchsia device to go online.

        Raises:
            errors.FuchsiaDeviceError: If device is not online.
        """
        _LOGGER.info("Waiting for %s to go online...", self.device_name)
        try:
            if self._is_static_ip:
                # If the device was specified as an address,
                # just continue to use that address. This is the
                # norm when run by builders in Infra, because they
                # specify a static IP address so the address is
                # expected not to change.
                self.ffx.wait_for_rcs_connection(include_target_name=False)
            else:
                # If the device was not specified as an address, it may change
                # IP addresses, so first wait for it by name, then update the
                # IP address. This is the appropriate way when run locally,
                # because in most environments the device may have a new
                # address after reboots, so the target name is the stable
                # query to use.
                self.ffx.wait_for_rcs_connection(include_target_name=True)
                # Now that we know that the device is available, update find
                # out and store its new IP address.
                await self.resolve_device_ip()
            _LOGGER.info("%s is online.", self.device_name)
        except (
            errors.DeviceNotConnectedError,
            ffx_errors.FfxCommandError,
        ) as err:
            raise errors.FuchsiaDeviceError(
                f"'{self.device_name}' failed to go online."
            ) from err

    def is_starnix_device(self) -> bool:
        """Check if the device under test is a starnix device.

        Some operation maybe heavy on starnix device, allow caller to find if running on starnix
        device.

        Raises:
            FuchsiaDeviceError: failed to check the device.
        """

        try:
            _LOGGER.info("%s is a starnix device", self.starnix)
            return True
        except errors.NotSupportedError:
            return False
        except starnix_errors.StarnixError as err:
            raise errors.FuchsiaDeviceError(err)

    # List all private properties
    async def _build_info(self) -> f_buildinfo.BuildInfo:
        """Returns the build information of the device.

        Returns:
            Build info dict.

        Raises:
            FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            buildinfo_provider_proxy = f_buildinfo.ProviderClient(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["BuildInfo"]
                )
            )
            build_info_resp = await buildinfo_provider_proxy.get_build_info()
            return build_info_resp.build_info
        except fcp.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    @properties.persistent_method
    async def _device_info_from_fidl(self) -> f_hwinfo.DeviceInfo:
        """Returns the device information of the device.

        Returns:
            Device info dict.

        Raises:
            FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            hwinfo_device_proxy = f_hwinfo.DeviceClient(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["DeviceInfo"]
                )
            )
            device_info_resp = await hwinfo_device_proxy.get_info()
            return device_info_resp.info
        except fcp.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    @properties.persistent_method
    async def _product_info(self) -> f_hwinfo.ProductInfo:
        """Returns the product information of the device.

        Returns:
            Product info dict.

        Raises:
            FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            hwinfo_product_proxy = f_hwinfo.ProductClient(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["ProductInfo"]
                )
            )
            product_info_resp = await hwinfo_product_proxy.get_info()
            return product_info_resp.info
        except fcp.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    async def boot_id(self) -> str:
        """Gets the boot id from a device.

        Returns:
            The boot id string.

        Raises:
            errors.FuchsiaDeviceError: On failure to get boot ID.
        """
        try:
            rcs_proxy = fd_remotecontrol.RemoteControlClient(
                self.fuchsia_controller.ctx.connect_remote_control_proxy()
            )
            resp = (await rcs_proxy.identify_host()).unwrap()

            if resp.response.boot_id is not None:
                return str(resp.response.boot_id)

            raise errors.FuchsiaDeviceError(
                f"Boot ID not populated in IdentifyHost response for {self.device_name}"
            )
        except Exception as err:
            raise errors.FuchsiaDeviceError(
                f"Failed to get boot ID from {self.device_name}"
            ) from err

    async def _last_reboot_info(self) -> f_feedback.LastReboot:
        """Gets the last reboot reason from a device.

        Returns:
            The last reboot info dictionary.

        Raises:
            FuchsiaControllerError: On FIDL communication failure or on
              data transfer verification failure.
        """
        try:
            proxy = f_feedback.LastRebootInfoProviderClient(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["LastRebootInfo"]
                )
            )
            resp = await proxy.get()
            return resp.last_reboot
        except fcp.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "_last_reboot_info() failed"
            ) from status

    # List all private methods
    async def _send_log_command(
        self, tag: str, message: str, level: custom_types.LEVEL
    ) -> None:
        """Send a device command to write to the syslog.

        Args:
            tag: Tag to apply to the message in the syslog.
            message: Message that need to logged.
            level: Log message level.

        Raises:
            FuchsiaControllerError: On FIDL communication failure.
        """
        _LOGGER.debug(
            "Attempting to log to device %s: %s, message: %s, level: %s",
            self.device_name,
            tag,
            message,
            level,
        )
        try:
            rcs_proxy = fd_remotecontrol.RemoteControlClient(
                self.fuchsia_controller.ctx.connect_remote_control_proxy()
            )
            await rcs_proxy.log_message(
                tag=tag, message=message, severity=_LOG_SEVERITIES[level]
            )
        except fcp.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    async def _send_reboot_command(self) -> None:
        """Send a device command to trigger a soft reboot.

        Raises:
            FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            power_proxy = fhp_statecontrol.AdminClient(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["PowerAdmin"]
                )
            )
            (
                await power_proxy.shutdown(
                    options=fhp_statecontrol.ShutdownOptions(
                        action=fhp_statecontrol.ShutdownAction.REBOOT,
                        reasons=[
                            fhp_statecontrol.ShutdownReason.DEVELOPER_REQUEST
                        ],
                    ),
                )
            )
        except fcp.FcTransportStatus as status:
            # We can't reliably get a message back from the device when
            # requesting shutdown, so we have to check for whether we receive a
            # communication error. This can be represented by either
            # FC_ERR_FDOMAIN which denotes an error with writing/reading from
            # the underlying channel, or FC_ERR_TRANSPORT which is when the
            # whole transport layer fails to communicate with the device. There
            # is a race regarding which can happen first and it is
            # non-deterministic
            fc_transport_status: int | None = (
                status.args[0] if len(status.args) > 0 else None
            )
            if (
                fc_transport_status != fcp.FcTransportStatus.FC_ERR_FDOMAIN
                and fc_transport_status
                != fcp.FcTransportStatus.FC_ERR_TRANSPORT
            ):
                raise fc_errors.FuchsiaControllerError(
                    "Fuchsia Controller FIDL Error"
                ) from status

    async def _read_snapshot_from_channel(
        self, channel_client: fcp.Channel
    ) -> bytes:
        """Read snapshot data from client end of the transfer channel.

        Args:
            channel_client: Client end of the snapshot data channel.

        Raises:
            FuchsiaControllerError: On FIDL communication failure or on
              data transfer verification failure.

        Returns:
            Bytes containing snapshot data as a zip archive.
        """
        # Snapshot is sent over the channel as |fuchsia.io.File|.
        file_proxy = f_io.FileClient(channel_client)

        # Get file size for verification later.
        try:
            attr_resp: f_io.NodeAttributes2 = (
                await file_proxy.get_attributes(
                    query=f_io.NodeAttributesQuery.CONTENT_SIZE
                )
            ).unwrap()
        except (AssertionError, fcp.ZxStatus, fcp.FcTransportStatus) as e:
            # The above `fcp.ZxStatus` is for the possible error returned by
            # calling `unwrap()` on the result of the function call, NOT for
            # the underlying protocol.
            raise fc_errors.FuchsiaControllerError(
                "get_attributes() failed"
            ) from e
        if attr_resp.immutable_attributes.content_size is None:
            raise fc_errors.FuchsiaControllerError(
                "get_attributes() returned empty content size"
            )
        expected_size: int = attr_resp.immutable_attributes.content_size

        # Read until channel is empty.
        ret: bytearray = bytearray()
        try:
            while True:
                response = (await file_proxy.read(count=f_io.MAX_BUF)).unwrap()
                if not response.data:
                    break
                ret.extend(response.data)
        except (AssertionError, fcp.FcTransportStatus, fcp.ZxStatus) as e:
            raise fc_errors.FuchsiaControllerError("read() failed") from e

        # Verify transfer.
        if len(ret) != expected_size:
            raise fc_errors.FuchsiaControllerError(
                f"Expected {expected_size} bytes, but read {len(ret)} bytes"
            )

        return bytes(ret)

    async def _send_snapshot_command(self) -> bytes:
        """Send a device command to take a snapshot.

        Raises:
            FuchsiaControllerError: On FIDL communication failure or on
              data transfer verification failure.

        Returns:
            Bytes containing snapshot data as a zip archive.
        """

        (
            channel_server,
            channel_client,
        ) = self.fuchsia_controller.channel_create()
        params = f_feedback.GetSnapshotParameters(
            # Set timeout to 2 minutes in nanoseconds.
            collection_timeout_per_data=2 * 60 * 10**9,
            response_channel=channel_server.take(),
        )

        try:
            feedback_proxy = f_feedback.DataProviderClient(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["Feedback"]
                )
            )
            # The data channel isn't populated until get_snapshot() returns so
            # there's no need to drain the channel in parallel.
            (await feedback_proxy.get_snapshot(params=params))
        except fcp.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "get_snapshot() failed"
            ) from status
        return await self._read_snapshot_from_channel(channel_client)

    def _get_bluetooth_affordances_implementation(
        self,
        should_exist: bool = True,
    ) -> bluetooth_types.Implementation | None:
        """Parses the bluetooth affordance config information and returns which bluetooth
        implementation to use.

        Returns:
            bluetooth_types.Implementation

        Raises:
            errors.ConfigError: If bluetooth affordance implementation detail is missing or not valid.
        """
        if self._config is None:
            return None

        bluetooth_affordance_implementation: str | None = common.read_from_dict(
            self._config,
            key_path=("affordances", "bluetooth", "implementation"),
            should_exist=should_exist,
        )
        if bluetooth_affordance_implementation is None:
            return None

        try:
            return bluetooth_types.Implementation(
                bluetooth_affordance_implementation
            )
        except ValueError as err:
            raise errors.ConfigError(
                f"Invalid value passed in config['affordances']['bluetooth']['implementation]. "
                f"Valid values are: {list(map(str, bluetooth_types.Implementation))}"
            ) from err

    def _get_tracing_affordance_implementation(
        self,
        should_exist: bool = False,
    ) -> tracing_types.Implementation:
        """Parses the tracing affordance config and returns which implementation to use."""
        if self._config is None:
            return tracing_types.Implementation.FUCHSIA_CONTROLLER

        tracing_affordance_implementation: str | None = common.read_from_dict(
            self._config,
            key_path=("affordances", "tracing", "implementation"),
            should_exist=should_exist,
        )
        if tracing_affordance_implementation is None:
            return tracing_types.Implementation.FUCHSIA_CONTROLLER

        try:
            return tracing_types.Implementation(
                tracing_affordance_implementation
            )
        except ValueError as err:
            raise errors.ConfigError(
                f"Invalid value passed in config['affordances']['tracing']['implementation']. "
                f"Valid values are: {list(map(str, tracing_types.Implementation))}"
            ) from err

    @cached_property
    def _is_sl4f_needed(self) -> bool:
        """Returns whether or not SL4F will be used.

        Returns:
            True if SL4F is needed. False, otherwise.
        """
        if (
            self._get_bluetooth_affordances_implementation(should_exist=False)
            == bluetooth_types.Implementation.SL4F
        ):
            return True
        return False
