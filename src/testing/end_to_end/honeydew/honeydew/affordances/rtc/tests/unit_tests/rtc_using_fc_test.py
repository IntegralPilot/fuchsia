# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.fuchsia_controller.rtc.py."""

import datetime
import unittest
from unittest import mock

import fidl_fuchsia_hardware_rtc as frtc
import fuchsia_controller_py
from honeydew import affordances_capable
from honeydew.affordances.rtc import rtc_using_fc
from honeydew.affordances.rtc.errors import HoneydewRtcError
from honeydew.transports.fuchsia_controller import fuchsia_controller

# Alias for convenience.
ZX_OK = fuchsia_controller_py.ZxStatus.ZX_OK
ZX_ERR_INTERNAL = fuchsia_controller_py.ZxStatus.ZX_ERR_INTERNAL


class RtcFcTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for the rtc_using_fc.RtcUsingFc class."""

    async def asyncSetUp(self) -> None:
        self.m_proxy = mock.create_autospec(frtc.DeviceClient, spec_set=True)
        self.enterContext(
            mock.patch.object(frtc, "DeviceClient", return_value=self.m_proxy)
        )
        self.m_proxy.get = mock.AsyncMock()
        self.m_proxy.set2 = mock.AsyncMock()

        self.transport = mock.create_autospec(
            fuchsia_controller.FuchsiaController
        )
        self.reboot_af = mock.create_autospec(
            affordances_capable.RebootCapableDevice
        )

        self.rtc = rtc_using_fc.RtcUsingFc(self.transport, self.reboot_af)
        self.transport.connect_device_proxy.assert_called_once()
        self.reboot_af.register_for_on_device_boot.assert_called_once()

    async def test_rtc_setup_fallback(self) -> None:
        self.transport.reset_mock()
        self.reboot_af.reset_mock()

        self.transport.connect_device_proxy.return_value = ZX_OK

        _ = rtc_using_fc.RtcUsingFc(self.transport, self.reboot_af)
        self.assertEqual(self.transport.connect_device_proxy.call_count, 1)
        self.reboot_af.register_for_on_device_boot.assert_called_once()

        (ep,), _ = self.transport.connect_device_proxy.call_args_list[0]

        self.assertEqual(rtc_using_fc.RtcUsingFc.MONIKER, ep.moniker)
        self.assertEqual(rtc_using_fc.CAPABILITY, ep.protocol)

    async def test_rtc_get(self) -> None:
        chip_time = frtc.Time(23, 50, 15, 5, 2, 2022)
        self.m_proxy.get.return_value = frtc.DeviceGetResult(
            response=frtc.DeviceGetResponse(rtc=chip_time)
        )

        want = datetime.datetime(
            chip_time.year,
            chip_time.month,
            chip_time.day,
            chip_time.hours,
            chip_time.minutes,
            chip_time.seconds,
        )

        self.assertEqual(want, await self.rtc.get())
        self.m_proxy.get.assert_called_once()

    async def test_rtc_get_exception(self) -> None:
        self.m_proxy.get.side_effect = fuchsia_controller_py.ZxStatus(
            fuchsia_controller_py.ZxStatus.ZX_ERR_INTERNAL
        )

        msg = r"Device\.Get\(\) error"
        with self.assertRaisesRegex(HoneydewRtcError, msg):
            await self.rtc.get()

        self.m_proxy.get.assert_called_once()

    async def test_rtc_set(self) -> None:
        time = datetime.datetime(2022, 2, 5, 15, 50, 23)
        self.m_proxy.set2.return_value = frtc.DeviceSet2Result()
        await self.rtc.set(time)
        self.m_proxy.set2.assert_called_once()

    async def test_rtc_set_error(self) -> None:
        """Test errors returned by Set2() are handled."""
        time = datetime.datetime(2022, 2, 5, 15, 50, 23)
        self.m_proxy.set2.return_value = frtc.DeviceSet2Result(
            err=ZX_ERR_INTERNAL
        )

        msg = r"Device\.Set2\(\) error"
        with self.assertRaisesRegex(HoneydewRtcError, msg):
            await self.rtc.set(time)

        self.m_proxy.set2.assert_called_once()

    async def test_rtc_set_exception(self) -> None:
        """Test that we gracefully handle transport errors when invoking Set2()."""
        time = datetime.datetime(2022, 2, 5, 15, 50, 23)
        self.m_proxy.set2.side_effect = fuchsia_controller_py.ZxStatus(
            fuchsia_controller_py.ZxStatus.ZX_ERR_INTERNAL
        )

        msg = r"Device\.Set2\(\) error"
        with self.assertRaisesRegex(HoneydewRtcError, msg):
            await self.rtc.set(time)

        self.m_proxy.set2.assert_called_once()


if __name__ == "__main__":
    unittest.main()
