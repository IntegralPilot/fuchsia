# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for PowerSource affordance."""

from __future__ import annotations

import unittest
from unittest import mock

import fidl_fuchsia_hardware_power_source as f_power_source
from fidl import GlobalHandleWaker
from honeydew import affordances_capable, errors
from honeydew.affordances.drivers.hardware.power import (
    PowerSource,
    PowerSourceRequestError,
    Role,
    SourceType,
    Spec,
    Status,
)
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)


class PowerSourceTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for PowerSource affordance."""

    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()
        GlobalHandleWaker()._reset_for_testing()

        self.ffx_obj = mock.MagicMock(spec=ffx_transport.FFX, autospec=True)
        self.ffx_obj.run.return_value = "fuchsia.hardware.power.source.Service"

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

        self.power_source = PowerSource(
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
            PowerSource(
                device_name="fuchsia-test-device",
                ffx=self.ffx_obj,
                fuchsia_controller=self.fc_transport_obj,
                reboot_affordance=self.reboot_affordance_obj,
                fuchsia_device_close=self.fuchsia_device_close_obj,
            )

    async def test_get_spec_success(self) -> None:
        """Test get_spec successfully returns Spec."""
        mock_proxy = mock.AsyncMock()
        mock_fidl_spec = f_power_source.Spec(
            name="Main AC Adapter",
            manufacturer="FuchsiaCorp",
            model_name="AC-100",
            type_=f_power_source.SourceType.AC,
        )
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_res.response = mock.MagicMock()
        mock_res.response.spec = mock_fidl_spec
        mock_proxy.get_spec.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        spec = await self.power_source.get_spec()
        self.assertEqual(spec.name, "Main AC Adapter")
        self.assertEqual(spec.manufacturer, "FuchsiaCorp")
        self.assertEqual(spec.model_name, "AC-100")
        self.assertEqual(spec.type, SourceType.AC)

    async def test_get_spec_error(self) -> None:
        """Test get_spec raises PowerSourceRequestError when driver returns error."""
        mock_proxy = mock.AsyncMock()
        mock_res = mock.MagicMock()
        mock_res.err = f_power_source.Error.INTERNAL
        mock_proxy.get_spec.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        with self.assertRaises(PowerSourceRequestError):
            await self.power_source.get_spec()

    async def test_get_status_success(self) -> None:
        """Test get_status successfully returns Status."""
        mock_proxy = mock.AsyncMock()
        mock_sink = f_power_source.SinkRole(
            name="USB Host",
            type_=f_power_source.SourceType.USB,
        )
        mock_fidl_status = f_power_source.Status(
            present=True,
            voltage_uv=5000000,
            current_ua=1500000,
            current_role=f_power_source.Role(sink=mock_sink),
        )
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_res.response = mock.MagicMock()
        mock_res.response.status = mock_fidl_status
        mock_proxy.get_status.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        status = await self.power_source.get_status()
        self.assertTrue(status.present)
        self.assertEqual(status.voltage_uv, 5000000)
        self.assertEqual(status.current_ua, 1500000)
        self.assertIsNotNone(status.current_role)
        assert status.current_role is not None
        self.assertIsNotNone(status.current_role.sink)
        assert status.current_role.sink is not None
        self.assertEqual(status.current_role.sink.name, "USB Host")
        self.assertEqual(status.current_role.sink.type, SourceType.USB)

    async def test_get_status_error(self) -> None:
        """Test get_status raises PowerSourceRequestError when driver returns error."""
        mock_proxy = mock.AsyncMock()
        mock_res = mock.MagicMock()
        mock_res.err = f_power_source.Error.NOT_SUPPORTED
        mock_proxy.get_status.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        with self.assertRaises(PowerSourceRequestError):
            await self.power_source.get_status()

    async def test_set_role_success(self) -> None:
        """Test set_role successfully sends Role to FIDL."""
        mock_proxy = mock.AsyncMock()
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_proxy.set_role.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        role = Role.with_sink(name="Wall Charger", source_type=SourceType.AC)
        await self.power_source.set_role(role)
        mock_proxy.set_role.assert_called_once()

    async def test_set_role_error(self) -> None:
        """Test set_role raises PowerSourceRequestError on error response."""
        mock_proxy = mock.AsyncMock()
        mock_res = mock.MagicMock()
        mock_res.err = f_power_source.Error.INVALID_ARGS
        mock_proxy.set_role.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        with self.assertRaises(PowerSourceRequestError):
            await self.power_source.set_role(Role.with_auto())

    async def test_watch_success(self) -> None:
        """Test watch returns updated Status."""
        mock_proxy = mock.AsyncMock()
        mock_fidl_status = f_power_source.Status(
            present=False,
            voltage_uv=0,
            current_ua=0,
        )
        mock_res = mock.MagicMock()
        mock_res.err = None
        mock_res.response = mock.MagicMock()
        mock_res.response.status = mock_fidl_status
        mock_proxy.watch.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        status = await self.power_source.watch()
        self.assertFalse(status.present)
        self.assertEqual(status.voltage_uv, 0)

    async def test_watch_error(self) -> None:
        """Test watch raises PowerSourceRequestError when already bound."""
        mock_proxy = mock.AsyncMock()
        mock_res = mock.MagicMock()
        mock_res.err = f_power_source.Error.ALREADY_BOUND
        mock_proxy.watch.return_value = mock_res

        self.power_source._proxy = mock_proxy
        self.power_source._ready = True

        with self.assertRaises(PowerSourceRequestError):
            await self.power_source.watch()

    async def test_type_conversions(self) -> None:
        """Test round-trip FIDL type conversions."""
        # Roles
        r_auto = Role.with_auto()
        fidl_auto = r_auto.to_fidl()
        self.assertEqual(Role.from_fidl(fidl_auto), r_auto)

        r_source = Role.with_source()
        fidl_source = r_source.to_fidl()
        self.assertEqual(Role.from_fidl(fidl_source), r_source)

        r_disc = Role.with_disconnected()
        fidl_disc = r_disc.to_fidl()
        self.assertEqual(Role.from_fidl(fidl_disc), r_disc)

        r_sink = Role.with_sink(name="Test", source_type=SourceType.BATTERY)
        fidl_sink = r_sink.to_fidl()
        self.assertEqual(Role.from_fidl(fidl_sink), r_sink)

        # Spec
        spec = Spec(
            name="Power Supply",
            supported_roles=[r_auto, r_sink],
            manufacturer="Vendor",
            model_name="V1",
            type=SourceType.USB,
        )
        fidl_spec = spec.to_fidl()
        self.assertEqual(Spec.from_fidl(fidl_spec), spec)

        # Status
        status = Status(
            present=True,
            voltage_uv=12000000,
            current_ua=3000000,
            current_role=r_sink,
        )
        fidl_status = status.to_fidl()
        self.assertEqual(Status.from_fidl(fidl_status), status)


if __name__ == "__main__":
    unittest.main()
