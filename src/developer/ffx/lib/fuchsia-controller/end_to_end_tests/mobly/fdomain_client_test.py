# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import List

import fuchsia_async_extension
from mobly import asserts, test_runner
from mobly_controller import fuchsia_device

from fidl import AsyncChannel


class FuchsiaControllerTests(fuchsia_async_extension.AsyncBaseTestClass):
    async def setup_class(self) -> None:
        self.fuchsia_devices: List[
            fuchsia_device.FuchsiaDevice
        ] = await self.register_controller(fuchsia_device)
        self.device = self.fuchsia_devices[0]
        self.device.set_ctx(self)

    async def test_channel_write_and_read(self) -> None:
        """Ensures creating a channel on a device is possible through FDomain.

        Most/all other tests run through the remote-control proxy. This is just
        directly on the device.
        """
        # There are no tests that just attempt to create a channel.
        (a, b) = self.device.channel_create()
        a.write((bytes([1, 2, 3, 4]), []))
        b_async = AsyncChannel(b)
        recv = await b_async.read()
        asserts.assert_equal(recv[0], bytes([1, 2, 3, 4]))


if __name__ == "__main__":
    test_runner.main()
