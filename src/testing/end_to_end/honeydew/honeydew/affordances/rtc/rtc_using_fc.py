# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Real time clock (RTC) affordance using the FuchsiaController."""

import datetime

import fidl_fuchsia_hardware_rtc as frtc
from fuchsia_controller_py import FcTransportStatus, ZxStatus
from honeydew import affordances_capable
from honeydew.affordances.rtc import rtc
from honeydew.affordances.rtc.errors import HoneydewRtcError
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fuchsia_controller_lib,
)
from honeydew.typing import custom_types

CAPABILITY = "fuchsia.hardware.rtc.Service/default/device"


class RtcUsingFc(rtc.Rtc):
    """Async affordance for the fuchsia.hardware.rtc.Device protocol."""

    # TODO(b/316959472) Use toolbox once RTC service lands in the toolbox realm.
    #
    # Service moniker for the NXP PCF8563 chip on a Khadas vim3 board.
    # Currently, this is board-specific. Once the RTC protocol lands and is
    # routable from the toolbox realm, this affordance can be made
    # board-agnostic.

    MONIKER = "/bootstrap/base-drivers:i2c-5000.aml-i2c.i2c.i2c-0-81.rtc-51"

    def __init__(
        self,
        fuchsia_controller: fuchsia_controller_lib.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        """Initializer."""
        self._controller = fuchsia_controller
        self.verify_supported()
        # This needs to be called once upon __init__(), and any time the device
        # is rebooted. On reboot, the connection is lost and needs to be
        # re-established.
        self._connect_proxy()
        reboot_affordance.register_for_on_device_boot(self._connect_proxy)

    def verify_supported(self) -> None:
        """Check if RTC is supported on the DUT.
        Raises:
            NotSupportedError: RTC affordance is not supported by Fuchsia device.
        """
        # TODO(http://b/409624089): Implement the method logic

    def _connect_proxy(self) -> None:
        """Connect the RTC Device protocol proxy."""
        ep = custom_types.FidlEndpoint(RtcUsingFc.MONIKER, CAPABILITY)
        try:
            self._proxy: frtc.DeviceClient = frtc.DeviceClient(
                self._controller.connect_device_proxy(ep)
            )
        except RuntimeError:
            raise HoneydewRtcError(
                f"Failed to connect to moniker {RtcUsingFc.MONIKER}"
            ) from None

    # Protocol methods.
    async def get(self) -> datetime.datetime:
        """See base class."""
        try:
            response = (await self._proxy.get()).unwrap()
        except (AssertionError, FcTransportStatus, ZxStatus) as e:
            msg = f"Device.Get() error {e}"
            raise HoneydewRtcError(msg) from e

        time = response.rtc
        return datetime.datetime(
            time.year,
            time.month,
            time.day,
            time.hours,
            time.minutes,
            time.seconds,
        )

    async def set(self, time: datetime.datetime) -> None:
        """See base class."""
        ftime = frtc.Time(
            time.second, time.minute, time.hour, time.day, time.month, time.year
        )
        try:
            (await self._proxy.set2(rtc=ftime)).unwrap()
        except (AssertionError, ZxStatus, FcTransportStatus) as e:
            msg = f"Device.Set2() error {e}"
            raise HoneydewRtcError(msg) from e
