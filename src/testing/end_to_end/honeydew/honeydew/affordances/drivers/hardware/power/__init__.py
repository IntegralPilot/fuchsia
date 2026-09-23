# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""PowerSource affordance implementation using Fuchsia Controller."""

from __future__ import annotations

import logging

import fidl_fuchsia_hardware_power_source as f_power_source
from fuchsia_controller_py import FcTransportStatus, ZxStatus
from honeydew import affordances_capable, errors
from honeydew.affordances.affordance import AsyncLazyReady, ensure_ready
from honeydew.affordances.drivers.hardware.power.utils.errors import (
    HoneydewPowerSourceError,
    PowerSourceDeviceNotFoundError,
    PowerSourceRequestError,
)
from honeydew.affordances.drivers.hardware.power.utils.types import (
    Auto,
    Disconnected,
    Error,
    Role,
    SinkRole,
    SourceRole,
    SourceType,
    Spec,
    Status,
)
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import FidlEndpoint

_LOGGER: logging.Logger = logging.getLogger(__name__)

_DEFAULT_MONIKER = "bootstrap/base-drivers:fake-battery"
_SOURCE_CAPABILITY = "fuchsia.hardware.power.source.Service/default/source"
_BATTERY_SOURCE_CAPABILITY = (
    "fuchsia.hardware.power.battery.Service/default/power_source"
)
_SERVICE_CAPABILITY = "fuchsia.hardware.power.source.Service"
_BATTERY_SERVICE_CAPABILITY = "fuchsia.hardware.power.battery.Service"

__all__ = [
    "Auto",
    "Disconnected",
    "Error",
    "HoneydewPowerSourceError",
    "PowerSource",
    "PowerSourceDeviceNotFoundError",
    "PowerSourceRequestError",
    "Role",
    "SinkRole",
    "SourceRole",
    "SourceType",
    "Spec",
    "Status",
]


class PowerSource(AsyncLazyReady):
    """PowerSource affordance implemented with Fuchsia Controller."""

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
        fuchsia_device_close: affordances_capable.FuchsiaDeviceClose,
    ) -> None:
        """Initialize PowerSource affordance.

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
        self._proxy: f_power_source.SourceClient | None = None
        self._active_capability: str = _SOURCE_CAPABILITY

        self.verify_supported()

        self._reboot_affordance.register_for_on_device_boot(self.make_ready)
        self._fuchsia_device_close.register_for_on_device_close(self._close)

    def verify_supported(self) -> None:
        """Verifies that the PowerSource affordance is supported on the target device."""
        for capability in (_SERVICE_CAPABILITY, _BATTERY_SERVICE_CAPABILITY):
            output = self._ffx.run(
                ["component", "capability", capability],
                machine=ffx_types.MachineFormat.RAW,
            )
            if capability in output:
                return

        raise errors.NotSupportedError(
            f"Neither '{_SERVICE_CAPABILITY}' nor '{_BATTERY_SERVICE_CAPABILITY}' "
            f"supported on {self._device_name}"
        )

    def _discover_endpoint(self) -> FidlEndpoint:
        """Determines the active power source moniker and capability on the target."""
        # First check for dedicated fuchsia.hardware.power.source.Service
        output = self._ffx.run(
            ["component", "capability", _SERVICE_CAPABILITY],
            machine=ffx_types.MachineFormat.RAW,
        )
        if _SERVICE_CAPABILITY in output:
            for line in output.splitlines():
                line = line.strip()
                if (
                    f"declared capability `{_SERVICE_CAPABILITY}`" in line
                    and "devfs_driver" not in line
                ) or (
                    f"exposed capability `{_SERVICE_CAPABILITY}` from self to parent"
                    in line
                ):
                    parts = line.split("`")
                    if len(parts) >= 2:
                        return FidlEndpoint(parts[1], _SOURCE_CAPABILITY)

        # Next check for fuchsia.hardware.power.battery.Service which includes power_source member
        battery_output = self._ffx.run(
            ["component", "capability", _BATTERY_SERVICE_CAPABILITY],
            machine=ffx_types.MachineFormat.RAW,
        )
        if _BATTERY_SERVICE_CAPABILITY in battery_output:
            for line in battery_output.splitlines():
                line = line.strip()
                if (
                    f"declared capability `{_BATTERY_SERVICE_CAPABILITY}`"
                    in line
                    and "devfs_driver" not in line
                ) or (
                    f"exposed capability `{_BATTERY_SERVICE_CAPABILITY}` from self to parent"
                    in line
                ):
                    parts = line.split("`")
                    if len(parts) >= 2:
                        return FidlEndpoint(
                            parts[1], _BATTERY_SOURCE_CAPABILITY
                        )

        return FidlEndpoint(_DEFAULT_MONIKER, _SOURCE_CAPABILITY)

    async def make_ready(self) -> None:
        """Establishes connection to the PowerSource FIDL service."""
        await super().make_ready()
        try:
            endpoint = self._discover_endpoint()
            channel = self._fc_transport.connect_device_proxy(endpoint)
            self._proxy = f_power_source.SourceClient(channel)
        except Exception as err:
            raise PowerSourceDeviceNotFoundError(
                f"Failed to connect to PowerSource proxy at {endpoint}"
            ) from err

    async def _close(self) -> None:
        """Releases the client connection."""
        self._proxy = None

    @ensure_ready
    async def get_spec(self) -> Spec:
        """Retrieves static hardware characteristics of the power source.

        Returns:
            Spec containing power source hardware characteristics.

        Raises:
            PowerSourceRequestError: If driver returns an error.
            HoneydewPowerSourceError: If transport fails.
        """
        assert self._proxy is not None
        try:
            res = await self._proxy.get_spec()
            if res.err is not None:
                raise PowerSourceRequestError("get_spec", res.err)
            assert res.response is not None
            return Spec.from_fidl(res.response.spec)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewPowerSourceError(
                "Transport error during get_spec"
            ) from err

    @ensure_ready
    async def get_status(self) -> Status:
        """Retrieves dynamic status of the power source immediately.

        Returns:
            Status containing current power source status.

        Raises:
            PowerSourceRequestError: If driver returns an error.
            HoneydewPowerSourceError: If transport fails.
        """
        assert self._proxy is not None
        try:
            res = await self._proxy.get_status()
            if res.err is not None:
                raise PowerSourceRequestError("get_status", res.err)
            assert res.response is not None
            return Status.from_fidl(res.response.status)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewPowerSourceError(
                "Transport error during get_status"
            ) from err

    @ensure_ready
    async def set_role(self, role: Role) -> None:
        """Sets the role of this specific power node.

        Args:
            role: The Role to assign to the node.

        Raises:
            PowerSourceRequestError: If driver returns an error.
            HoneydewPowerSourceError: If transport fails.
        """
        assert self._proxy is not None
        try:
            res = await self._proxy.set_role(role=role.to_fidl())
            if res.err is not None:
                raise PowerSourceRequestError("set_role", res.err)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewPowerSourceError(
                "Transport error during set_role"
            ) from err

    @ensure_ready
    async def watch(
        self,
        interest: Status | None = None,
        wake_on: Status | None = None,
    ) -> Status:
        """Change notification for power source state using hanging-get.

        Args:
            interest: Status mask specifying fields to be notified about.
            wake_on: Status mask specifying fields that should trigger system wake.

        Returns:
            Status containing the updated fields.

        Raises:
            PowerSourceRequestError: If driver returns an error.
            HoneydewPowerSourceError: If transport fails.
        """
        assert self._proxy is not None
        fidl_interest = (
            interest.to_fidl() if interest else f_power_source.Status()
        )
        fidl_wake_on = wake_on.to_fidl() if wake_on else f_power_source.Status()
        try:
            res = await self._proxy.watch(
                interest=fidl_interest,
                wake_on=fidl_wake_on,
                lease=None,
            )
            if res.err is not None:
                raise PowerSourceRequestError("watch", res.err)
            assert res.response is not None
            return Status.from_fidl(res.response.status)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewPowerSourceError(
                "Transport error during watch"
            ) from err
