# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Battery affordance implementation using Fuchsia Controller."""

from __future__ import annotations

import logging

import fidl_fuchsia_hardware_power_battery as f_battery
from fuchsia_controller_py import FcTransportStatus, ZxStatus
from honeydew import affordances_capable, errors
from honeydew.affordances.affordance import AsyncLazyReady, ensure_ready
from honeydew.affordances.drivers.battery_manager.utils.errors import (
    BatteryDeviceNotFoundError,
    BatteryRequestError,
    HoneydewBatteryError,
)
from honeydew.affordances.drivers.battery_manager.utils.types import (
    BatterySpec,
    BatteryStatus,
    ChargeStatus,
    HealthStatus,
    PowerSourceSpec,
    PowerSourceStatus,
    PowerSourceType,
    WatchOptions,
)
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import FidlEndpoint

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Capability or moniker definition
_BATTERY_MONIKER = "bootstrap/base-drivers:fake-battery"
_BATTERY_CAPABILITY = "fuchsia.hardware.power.battery.Service/default/battery"
_REQUIRED_CAPABILITIES = ["fuchsia.hardware.power.battery.Service"]

__all__ = [
    "Battery",
    "BatteryDeviceNotFoundError",
    "BatteryRequestError",
    "BatterySpec",
    "BatteryStatus",
    "ChargeStatus",
    "HealthStatus",
    "HoneydewBatteryError",
    "PowerSourceSpec",
    "PowerSourceStatus",
    "PowerSourceType",
    "WatchOptions",
]


class Battery(AsyncLazyReady):
    """Battery affordance implemented with Fuchsia Controller."""

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
        fuchsia_device_close: affordances_capable.FuchsiaDeviceClose,
    ) -> None:
        """Initialize Battery affordance.

        Args:
            device_name: Target device name.
            ffx: FFX transport instance.
            fuchsia_controller: Fuchsia Controller transport instance.
            reboot_affordance: Callback registry for reboot events.
            fuchsia_device_close: Callback registry for close events.
        """
        super().__init__()
        self._device_name: str = device_name
        self._ffx: ffx_transport.FFX = ffx
        self._fc_transport: fc_transport.FuchsiaController = fuchsia_controller
        self._reboot_affordance = reboot_affordance
        self._fuchsia_device_close = fuchsia_device_close
        self._proxy: f_battery.BatteryClient | None = None

        self.verify_supported()

        self._reboot_affordance.register_for_on_device_boot(self.make_ready)
        self._fuchsia_device_close.register_for_on_device_close(self._close)

    def verify_supported(self) -> None:
        """Verifies that the Battery affordance is supported on the target device."""
        for capability in _REQUIRED_CAPABILITIES:
            output = self._ffx.run(
                ["component", "capability", capability],
                machine=ffx_types.MachineFormat.RAW,
            )
            if capability not in output:
                raise errors.NotSupportedError(
                    f"Capability '{capability}' not supported on {self._device_name}"
                )

    def _get_battery_moniker(self) -> str:
        """Determines the active battery driver moniker on the target."""
        output = self._ffx.run(
            [
                "component",
                "capability",
                "fuchsia.hardware.power.battery.Service",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )
        for line in output.splitlines():
            line = line.strip()
            if (
                "declared capability `fuchsia.hardware.power.battery.Service`"
                in line
                and "devfs_driver" not in line
            ):
                parts = line.split("`")
                if len(parts) >= 2:
                    return parts[1]
            if (
                "exposed capability `fuchsia.hardware.power.battery.Service` from self to parent"
                in line
            ):
                parts = line.split("`")
                if len(parts) >= 2:
                    return parts[1]
        return _BATTERY_MONIKER

    async def make_ready(self) -> None:
        """Establishes connection to the Battery FIDL service."""
        await super().make_ready()
        try:
            moniker = self._get_battery_moniker()
            endpoint = FidlEndpoint(moniker, _BATTERY_CAPABILITY)
            channel = self._fc_transport.connect_device_proxy(endpoint)
            self._proxy = f_battery.BatteryClient(channel)
        except Exception as err:
            raise BatteryDeviceNotFoundError(
                f"Failed to connect to Battery proxy at {_BATTERY_CAPABILITY}"
            ) from err

    async def _close(self) -> None:
        """Releases the client connection."""
        self._proxy = None

    @ensure_ready
    async def get_spec(self) -> BatterySpec:
        """Retrieves static hardware characteristics of the battery.

        Returns:
            BatterySpec containing battery hardware metadata.

        Raises:
            BatteryRequestError: If driver returns an error.
            HoneydewBatteryError: If transport fails.
        """
        assert self._proxy is not None
        try:
            res = await self._proxy.get_spec()
            if res.err is not None:
                raise BatteryRequestError("get_spec", res.err)
            assert res.response is not None
            return BatterySpec.from_fidl(res.response.spec)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewBatteryError(
                "Transport error during get_spec"
            ) from err

    @ensure_ready
    async def get_status(self) -> BatteryStatus:
        """Retrieves dynamic telemetry and charge status of the battery.

        Returns:
            BatteryStatus containing current battery metrics.

        Raises:
            BatteryRequestError: If driver returns an error.
            HoneydewBatteryError: If transport fails.
        """
        assert self._proxy is not None
        try:
            res = await self._proxy.get_status()
            if res.err is not None:
                raise BatteryRequestError("get_status", res.err)
            assert res.response is not None
            return BatteryStatus.from_fidl(res.response.status)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewBatteryError(
                "Transport error during get_status"
            ) from err

    @ensure_ready
    async def configure_watch(
        self,
        interest: BatteryStatus | None = None,
        wake_on: BatteryStatus | None = None,
    ) -> WatchOptions:
        """Configures fields of interest and wake triggers for watching battery state.

        Args:
            interest: BatteryStatus mask specifying fields to be notified about.
            wake_on: BatteryStatus mask specifying fields that should trigger system wake.

        Returns:
            WatchOptions containing effective options enabled by the driver.

        Raises:
            BatteryRequestError: If driver returns an error.
            HoneydewBatteryError: If transport fails.
        """
        assert self._proxy is not None
        fidl_options = f_battery.WatchOptions(
            interest=interest.to_fidl() if interest else None,
            wake_on=wake_on.to_fidl() if wake_on else None,
        )
        try:
            res = await self._proxy.configure_watch(options=fidl_options)
            if res.err is not None:
                raise BatteryRequestError("configure_watch", res.err)
            assert res.response is not None
            return WatchOptions.from_fidl(res.response.effective_options)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewBatteryError(
                "Transport error during configure_watch"
            ) from err

    @ensure_ready
    async def watch(
        self,
        interest: BatteryStatus | None = None,
        wake_on: BatteryStatus | None = None,
        lease: int | None = None,
    ) -> BatteryStatus:
        """Hanging get for battery status updates.

        Args:
            interest: BatteryStatus mask specifying fields to be notified about.
            wake_on: BatteryStatus mask specifying fields that should trigger system wake.
            lease: Optional wake lease token handle for suspend coordination.

        Returns:
            BatteryStatus containing the updated fields.

        Raises:
            BatteryRequestError: If driver returns an error.
            HoneydewBatteryError: If transport fails.
        """
        assert self._proxy is not None
        if interest is not None or wake_on is not None:
            await self.configure_watch(interest=interest, wake_on=wake_on)
        try:
            res = await self._proxy.watch(lease=lease)
            if res.err is not None:
                raise BatteryRequestError("watch", res.err)
            assert res.response is not None
            return BatteryStatus.from_fidl(res.response.status)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewBatteryError("Transport error during watch") from err
