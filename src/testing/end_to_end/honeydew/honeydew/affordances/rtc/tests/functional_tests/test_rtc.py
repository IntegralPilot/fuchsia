# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for RTC affordance."""

import datetime
import logging
import random

import fuchsia_base_test
from mobly import asserts, test_runner

LOGGER: logging.Logger = logging.getLogger(__name__)


class RtcAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """RTC affordance tests."""

    async def setup_class(self) -> None:
        await super().setup_class()

    async def teardown_class(self) -> None:
        """Post-test teardown logic.

        Because this test affects the real value in the RTC, the value needs to
        be restored to walltime. Otherwise, this test causes interference with
        other tests on the system if/when timekeeper syncs the system clock to
        the value stored in the RTC chip.
        """
        LOGGER.info("Reverting RTC to host walltime")
        await self.rtc.set(datetime.datetime.now())
        LOGGER.info("Walltime is now: %s", await self.rtc.get())
        await super().teardown_class()

    async def setup_test(self) -> None:
        await super().setup_test()
        self.rtc = self.dut.rtc

    async def test_rtc_chip_io(self) -> None:
        """Test the fuchsia.hardware.rtc.Device protocol.

        This test verifies that the RTC can be written to, and subsequently read
        from. When re-reading the time off the RTC, the test verifies the time
        read is within some threshold of the expected time.

        This test does not reboot the device but will clobber the RTC's stored
        value.
        """
        threshold = 2  # Seconds.

        # We'll assume a random year just so that subsequent test invocations do
        # not contain overlapping testing conditions.
        randyear = random.randint(1900, 2099)

        # We'll start by setting the RTC to a known time: YEAR-12-20T23:30:00.
        #
        # The values for month, day, and h/m/s are chosen to try and catch any
        # possible field transposition errors in the driver.
        base_time = datetime.datetime(randyear, 12, 20, 23, 30, 0)
        LOGGER.info("Setting RTC time: %s", base_time)
        await self.rtc.set(base_time)

        # Ensure the time was actually set by re-reading the time and ensuring
        # the time elapsed is within some reasonable threshold. The threshold
        # value may need tuning.
        time = await self.rtc.get()
        delta = abs(time - base_time)
        LOGGER.info("Time read off RTC is: %s", time)
        LOGGER.info("Time delta: %s", delta)
        asserts.assert_less_equal(
            delta,
            datetime.timedelta(seconds=threshold),
            f"Delta exceeded threshold of {threshold} seconds",
        )


if __name__ == "__main__":
    test_runner.main()
