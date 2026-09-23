# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for netstack affordance."""

import logging
from datetime import timedelta

import fuchsia_base_test
from honeydew.affordances.connectivity.netstack.errors import (
    HoneydewNetstackError,
)
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class NetstackTests(fuchsia_base_test.FuchsiaBaseTest):
    """Netstack affordance tests"""

    async def test_list_interfaces(self) -> None:
        """Verify list_interfaces() works on device."""
        interfaces = await self.dut.netstack.list_interfaces()
        asserts.assert_greater(len(interfaces), 0)

    async def test_ping_success(self) -> None:
        """Verify pinging localhost succeeds."""
        res = await self.dut.netstack.ping("127.0.0.1")
        asserts.assert_equal(res.requested, 3, "Requested 3 pings by default")
        asserts.assert_true(
            res.any_pings_received,
            "Expected at least one ping to be received",
        )
        asserts.assert_true(
            res.all_pings_received,
            "Expected all pings to be received",
        )

    async def test_ping_timeout(self) -> None:
        """Verify pinging non-existent IP raises error."""
        with asserts.assert_raises(HoneydewNetstackError):
            await self.dut.netstack.ping(
                "192.0.2.1", count=1, timeout=timedelta(seconds=1)
            )

    async def test_ping_invalid_host(self) -> None:
        """Verify pinging invalid host raises error."""
        with asserts.assert_raises(HoneydewNetstackError):
            await self.dut.netstack.ping("foo.invalid", count=1)


if __name__ == "__main__":
    test_runner.main()
