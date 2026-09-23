# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.fuchsia_controller.location."""

import unittest
from typing import TypeVar
from unittest import mock

import fidl_fuchsia_location_namedplace as f_location_namedplace
from fuchsia_controller_py import FcTransportStatus
from honeydew import affordances_capable
from honeydew.affordances import location
from honeydew.affordances.connectivity.wlan.utils.types import CountryCode
from honeydew.affordances.location.errors import HoneydewLocationError
from honeydew.errors import NotSupportedError
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)

_T = TypeVar("_T")


async def _async_response(response: _T) -> _T:
    return response


# pylint: disable=protected-access
class LocationFCTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.affordances.fuchsia_controller.location."""

    def setUp(self) -> None:
        super().setUp()

        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice,
            autospec=True,
        )
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController,
            autospec=True,
        )
        self.ffx_transport_obj = mock.MagicMock(
            spec=ffx_transport.FFX,
            autospec=True,
        )

        self.ffx_transport_obj.run.return_value = "".join(
            location._REQUIRED_CAPABILITIES
        )

        self.location_obj = location.Location(
            device_name="fuchsia-emulator",
            ffx=self.ffx_transport_obj,
            fuchsia_controller=self.fc_transport_obj,
            reboot_affordance=self.reboot_affordance_obj,
        )

    def test_verify_supported(self) -> None:
        """Test if verify_supported works."""
        self.ffx_transport_obj.run.return_value = ""

        with self.assertRaises(NotSupportedError):
            self.location_obj = location.Location(
                device_name="fuchsia-emulator",
                ffx=self.ffx_transport_obj,
                fuchsia_controller=self.fc_transport_obj,
                reboot_affordance=self.reboot_affordance_obj,
            )

    def test_init_register_for_on_device_boot(self) -> None:
        """Test if Location registers on_device_boot."""
        self.reboot_affordance_obj.register_for_on_device_boot.assert_called_once_with(
            self.location_obj._connect_proxy
        )

    def test_init_connect_proxy(self) -> None:
        """Test if Location connects to
        fuchsia.location.namedplace/RegulatoryRegionConfigurator."""
        self.assertIsNotNone(self.location_obj._regulatory_region_configurator)

    async def test_set_region_works(self) -> None:
        """Test if set_region works with valid input."""
        self.location_obj._regulatory_region_configurator = mock.MagicMock(
            spec=f_location_namedplace.RegulatoryRegionConfiguratorClient
        )
        self.location_obj._regulatory_region_configurator.set_region.return_value = (
            None
        )
        await self.location_obj.set_region(CountryCode("AT"))

    async def test_set_region_fails_internal_error(self) -> None:
        """Verify set_region fails when the location stack errors."""
        self.location_obj._regulatory_region_configurator = mock.MagicMock(
            spec=f_location_namedplace.RegulatoryRegionConfiguratorClient
        )
        self.location_obj._regulatory_region_configurator.set_region.side_effect = FcTransportStatus(
            FcTransportStatus.FC_ERR_INTERNAL
        )
        with self.assertRaises(HoneydewLocationError):
            await self.location_obj.set_region(CountryCode("AT"))


if __name__ == "__main__":
    unittest.main()
