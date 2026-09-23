# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


import logging
import typing

import fidl_fuchsia_hardware_power_statecontrol as power_statecontrol
import fuchsia_async_extension
from fuchsia_controller_py import FcTransportStatus
from mobly import test_runner
from mobly_controller import fuchsia_device

_LOGGER = logging.getLogger("reboot_test")


class FuchsiaControllerTests(fuchsia_async_extension.AsyncBaseTestClass):
    async def setup_class(self) -> None:
        self.fuchsia_devices: typing.List[
            fuchsia_device.FuchsiaDevice
        ] = await self.register_controller(fuchsia_device)
        self.device = self.fuchsia_devices[0]
        self.device.set_ctx(self)

    async def test_fuchsia_device_reboot(self) -> None:
        """Attempts to reboot a device."""
        if self.device.ctx is None:
            raise ValueError(f"Device: {self.device.target} has no context")
        # [START reboot_example]
        ch = self.device.ctx.connect_device_proxy(
            "bootstrap/shutdown_shim", power_statecontrol.AdminMarker
        )
        admin = power_statecontrol.AdminClient(ch)
        # Makes a coroutine to ensure that a PEER_CLOSED isn't received from attempting
        # to write to the channel.
        coro = admin.shutdown(
            options=power_statecontrol.ShutdownOptions(
                action=power_statecontrol.ShutdownAction.REBOOT,
                reasons=[power_statecontrol.ShutdownReason.DEVELOPER_REQUEST],
            ),
        )
        try:
            _LOGGER.info("Issuing reboot command")
            self.device.notify_intentional_disconnect()
            await coro
        except FcTransportStatus as status:
            valid_reboot_errs = [
                FcTransportStatus.FC_ERR_FDOMAIN,
                FcTransportStatus.FC_ERR_TRANSPORT,
            ]
            if status.code() not in valid_reboot_errs:
                raise status
            _LOGGER.info("Device reboot command sent")
        # [END reboot_example]
        await self.device.wait_offline()
        await self.device.wait_online()


if __name__ == "__main__":
    test_runner.main()
