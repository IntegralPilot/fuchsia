# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for Battery affordance."""

from __future__ import annotations

import unittest
from unittest import mock

import fidl_fuchsia_hardware_power_battery as f_battery
from fidl import GlobalHandleWaker
from honeydew import affordances_capable, errors
from honeydew.affordances.drivers.battery_manager import Battery, BatteryStatus
from honeydew.affordances.drivers.battery_manager.utils.errors import (
    BatteryRequestError,
)
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)


class BatteryTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for Battery affordance."""

    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()
        GlobalHandleWaker()._reset_for_testing()

        self.ffx_obj = mock.MagicMock(spec=ffx_transport.FFX, autospec=True)
        self.ffx_obj.run.return_value = "fuchsia.hardware.power.battery.Service"

        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice,
            autospec=True,
        )
        self.fuchsia_device_close_obj = mock.MagicMock(
            spec=affordances_capable.FuchsiaDeviceClose,
            autospec=True,
        )
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController,
            autospec=True,
        )

        self.battery = Battery(
            device_name="fuchsia-test-device",
            ffx=self.ffx_obj,
            fuchsia_controller=self.fc_transport_obj,
            reboot_affordance=self.reboot_affordance_obj,
            fuchsia_device_close=self.fuchsia_device_close_obj,
        )

    async def test_verify_supported_failure(self) -> None:
        """Test verify_supported raises NotSupportedError when capability missing."""
        self.ffx_obj.run.return_value = ""
        with self.assertRaises(errors.NotSupportedError):
            Battery(
                device_name="fuchsia-test-device",
                ffx=self.ffx_obj,
                fuchsia_controller=self.fc_transport_obj,
                reboot_affordance=self.reboot_affordance_obj,
                fuchsia_device_close=self.fuchsia_device_close_obj,
            )

    async def test_get_spec_success(self) -> None:
        """Test get_spec successfully returns BatterySpec."""
        mock_proxy = mock.AsyncMock()
        mock_fidl_spec = f_battery.Spec(
            design_capacity_uah=4000000,
            design_voltage_uv=3800000,
            chemistry="Li-ion",
        )
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_res.response = mock.MagicMock()
        mock_res.response.spec = mock_fidl_spec
        mock_proxy.get_spec.return_value = mock_res

        self.battery._proxy = mock_proxy
        self.battery._ready = True

        spec = await self.battery.get_spec()
        self.assertEqual(spec.design_capacity_uah, 4000000)
        self.assertEqual(spec.chemistry, "Li-ion")

    async def test_get_status_success(self) -> None:
        """Test get_status successfully returns BatteryStatus."""
        mock_proxy = mock.AsyncMock()
        mock_fidl_status = f_battery.Status(
            charge_status=f_battery.ChargeStatus.CHARGING,
            level_percent=85.5,
            remaining_capacity_uah=3400000,
        )
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_res.response = mock.MagicMock()
        mock_res.response.status = mock_fidl_status
        mock_proxy.get_status.return_value = mock_res

        self.battery._proxy = mock_proxy
        self.battery._ready = True

        status = await self.battery.get_status()
        self.assertEqual(status.charge_status, 2)
        self.assertEqual(status.level_percent, 85.5)

    async def test_get_status_error(self) -> None:
        """Test get_status raises BatteryRequestError when driver returns error."""
        mock_proxy = mock.AsyncMock()
        mock_res = mock.MagicMock()
        mock_res.err = 20
        mock_proxy.get_status.return_value = mock_res

        self.battery._proxy = mock_proxy
        self.battery._ready = True

        with self.assertRaises(BatteryRequestError):
            await self.battery.get_status()

    async def test_configure_watch_success(self) -> None:
        """Test configure_watch successfully returns WatchOptions."""
        mock_proxy = mock.AsyncMock()
        mock_fidl_effective = f_battery.WatchOptions(
            interest=f_battery.Status(level_percent=0.0),
        )
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_res.response = mock.MagicMock()
        mock_res.response.effective_options = mock_fidl_effective
        mock_proxy.configure_watch.return_value = mock_res

        self.battery._proxy = mock_proxy
        self.battery._ready = True

        options = await self.battery.configure_watch(
            interest=BatteryStatus(level_percent=0.0)
        )
        self.assertIsNotNone(options.interest)
        mock_proxy.configure_watch.assert_awaited_once()

    async def test_watch_success(self) -> None:
        """Test watch successfully returns BatteryStatus."""
        mock_proxy = mock.AsyncMock()
        mock_fidl_status = f_battery.Status(
            charge_status=f_battery.ChargeStatus.FULL,
            level_percent=100.0,
            present=True,
        )
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_res.response = mock.MagicMock()
        mock_res.response.status = mock_fidl_status
        mock_proxy.watch.return_value = mock_res

        self.battery._proxy = mock_proxy
        self.battery._ready = True

        status = await self.battery.watch()
        self.assertEqual(status.charge_status, 4)
        self.assertEqual(status.level_percent, 100.0)
        self.assertTrue(status.present)
        mock_proxy.watch.assert_awaited_once_with(lease=None)
