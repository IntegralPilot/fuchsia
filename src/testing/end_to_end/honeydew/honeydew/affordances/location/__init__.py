# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fidl_fuchsia_location_namedplace as f_location_namedplace
from fuchsia_controller_py import FcTransportStatus
from honeydew import affordances_capable
from honeydew import errors as honeydew_errors
from honeydew.affordances.connectivity.wlan.utils.types import (
    CountryCode,
)
from honeydew.affordances.location.errors import HoneydewLocationError
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import FidlEndpoint

# List of required FIDLs for this affordance.
_REQUIRED_CAPABILITIES = [
    "fuchsia.location.namedplace",
]

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Fuchsia Controller proxies
_REGULATORY_REGION_CONFIGURATOR_PROXY = FidlEndpoint(
    "core/regulatory_region",
    "fuchsia.location.namedplace.RegulatoryRegionConfigurator",
)


class Location:
    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        """Create an Location affordance.

        Args:
            device_name: Device name returned by `ffx target list`.
            ffx: FFX transport.
            fuchsia_controller: Fuchsia Controller transport.
            reboot_affordance: Object that implements RebootCapableDevice.
        """
        super().__init__()

        self._fc_transport = fuchsia_controller
        self._reboot_affordance = reboot_affordance
        self.device = device_name
        self.ffx = ffx

        self.verify_supported()

        self._connect_proxy()
        self._reboot_affordance.register_for_on_device_boot(self._connect_proxy)

    def verify_supported(self) -> None:
        """Check if location is supported on the DUT.

        Raises:
            NotSupportedError: Location affordance is not supported by Fuchsia device.
        """
        for capability in _REQUIRED_CAPABILITIES:
            # TODO(http://b/359342196): This is a maintenance burden; find a
            # better way to detect FIDL component capabilities.
            if capability not in self.ffx.run(
                ["component", "capability", capability],
                # TODO(b/474143046) update to JSON when ffx supports it
                machine=ffx_types.MachineFormat.RAW,
            ):
                _LOGGER.warning(
                    "All available location component capabilities:\n%s",
                    self.ffx.run(
                        ["component", "capability", "fuchsia.location"],
                        # TODO(b/474143046) update to JSON when ffx supports it
                        machine=ffx_types.MachineFormat.RAW,
                    ),
                )
                raise honeydew_errors.NotSupportedError(
                    f'Component capability "{capability}" not exposed by device '
                    f"{self.device}; this build of Fuchsia does not support the "
                    "location affordance."
                )

    def _connect_proxy(self) -> None:
        """Re-initializes connection to the location stack."""
        self._regulatory_region_configurator = (
            f_location_namedplace.RegulatoryRegionConfiguratorClient(
                self._fc_transport.connect_device_proxy(
                    _REGULATORY_REGION_CONFIGURATOR_PROXY
                )
            )
        )

    async def set_region(self, region_code: CountryCode) -> None:
        """Set regulatory region.

        Args:
            region_code: 2-byte country code

        Raises:
            HoneydewLocationError: Error from location stack
            TypeError: Invalid region_code format
        """
        try:
            self._regulatory_region_configurator.set_region(
                region=str(region_code)
            )
        except FcTransportStatus as status:
            _LOGGER.error("set_region error = %s", status)
            raise HoneydewLocationError(
                f"RegulatoryRegionConfigurator.SetRegion() error {status}"
            ) from status

        # TODO(http://b/370600007): Validate region was set using
        # RegulatoryRegionWatcher.GetRegionUpdate()
