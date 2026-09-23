#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for ADB transport."""

import fuchsia_base_test
from honeydew.transports.adb import adb as adb_transport
from mobly import asserts, signals, test_runner


class AdbTest(fuchsia_base_test.FuchsiaBaseTest):
    """ADB transport functional tests."""

    adb: adb_transport.Adb

    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()
        if self.dut.adb is None or not await self.dut.adb.is_supported():
            raise signals.TestAbortClass("ADB is not supported on this target")
        self.adb = self.dut.adb
        await self.adb.run(["wait-for-device"])

    async def test_list_devices(self) -> None:
        """Test case for ADB.run(["devices", "-l"])."""
        output = await self.adb.run(["devices", "-l"])
        asserts.assert_in("device:", output)

    async def test_adb_shell(self) -> None:
        """Test case for ADB.run(["shell", ...])."""
        output = await self.adb.run(["shell", 'echo "hello"'])
        asserts.assert_in("hello", output)

    async def test_adb_reboot(self) -> None:
        """Test case for ADB.run(["reboot"])."""
        await self.adb.run(["reboot"])
        await self.dut.wait_for_online()
        await self.dut.on_device_boot()
        await self.adb.run(["wait-for-device"])
        output = await self.adb.run(["shell", 'echo "hello"'])
        asserts.assert_in("hello", output)

    async def test_fuchsia_reboot(self) -> None:
        """Ensure ADB transport is functional after FuchsiaDevice reboot."""
        await self.dut.reboot()
        await self.adb.run(["wait-for-device"])
        output = await self.adb.run(["shell", 'echo "hello"'])
        asserts.assert_in("hello", output)

    async def test_adb_root_unroot(self) -> None:
        """Test case for ADB root and unroot commands."""
        try:
            # Ensure we start as non-root
            await self.adb.run(["unroot"])
            await self.adb.run(["wait-for-device"])
            output = await self.adb.run(["shell", "id"])
            asserts.assert_in("uid=2000(shell)", output)

            # Switch to root
            await self.adb.run(["root"])
            await self.adb.run(["wait-for-device"])
            output = await self.adb.run(["shell", "id"])
            asserts.assert_in("uid=0(root)", output)
        finally:
            # Always restore non-root state
            await self.adb.run(["unroot"])
            await self.adb.run(["wait-for-device"])
            output = await self.adb.run(["shell", "id"])
            asserts.assert_in("uid=2000(shell)", output)


if __name__ == "__main__":
    test_runner.main()
