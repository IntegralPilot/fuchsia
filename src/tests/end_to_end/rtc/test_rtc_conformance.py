# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""RTC conformance test."""

import contextlib
import datetime
import logging
import random
from typing import Any, Literal

import fuchsia_base_test
from mobly import asserts, test_runner

LOGGER: logging.Logger = logging.getLogger(__name__)


class TimeIt(contextlib.ContextDecorator):
    """A context manager which logs elapsed time of the with-block.

    Any exception raised by the with-block will propagate.

    Attributes:
        time_elapsed: The measured elapsed time, only available after the
            context manager exits.
    """

    time_elapsed: datetime.timedelta

    def __init__(self, msg: str) -> None:
        """Initialized with a descriptive context message."""
        self._msg = msg

    def __enter__(self) -> "TimeIt":
        """Context enter hook."""
        self._started = datetime.datetime.now()
        return self

    def __exit__(
        self, exc_type: Any, exc_value: Any, traceback: Any
    ) -> Literal[False]:
        """Context exit hook."""
        self.time_elapsed = abs(datetime.datetime.now() - self._started)

        LOGGER.info(f"{self._msg}: time elapsed {self.time_elapsed}")
        return False  # Never suppress raised exceptions.


class RtcTest(fuchsia_base_test.FuchsiaBaseTest):
    """fuchsia.hardware.rtc.Device protocol conformance Test."""

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

    async def test_rtc(self) -> None:
        """Test the fuchsia.hardware.rtc.Device protocol.

        This test verifies that the RTC can be written to, read from, and
        re-read post-soft-reset (any reboot which doesn't cut power to the
        chip). When re-reading the time off the RTC, the test verifies the time
        read is within some threshold of the expected time.
        """
        threshold = 5  # Seconds.

        LOGGER.info("Starting RTC conformance test on %s", self.dut.device_name)

        # We'll assume a random year just so that subsequent test invocations do
        # not contain overlapping testing conditions.
        randyear = random.randint(1900, 2099)

        # We'll start by setting the RTC to a known time: YEAR-12-20T23:30:00.
        #
        # The values for month, day, and h/m/s are chosen to try and catch any
        # possible field transposition errors in the driver.
        base_time = datetime.datetime(randyear, 12, 20, 23, 30, 0)
        LOGGER.info("Setting RTC time: %s", base_time)
        with TimeIt("Set()"):
            await self.rtc.set(base_time)

        # Ensure the time was actually set by re-reading the time and ensuring
        # the time elapsed is within some reasonable threshold. The threshold
        # value may need tuning. The value read here will be used as a benchmark
        # later (post-reboot).
        with TimeIt("Get()"):
            rtc_time1 = await self.rtc.get()

        LOGGER.info("Time read off RTC is: %s", rtc_time1)
        asserts.assert_less(
            rtc_time1 - base_time, datetime.timedelta(seconds=threshold)
        )

        # Next, reboot the device, re-read the RTC time, and (again) ensure the
        # total elapsed time is within some reasonable threshold. This needs to
        # account for the time spent actually rebooting the device.
        with TimeIt("Reboot") as reboot:
            await self.dut.reboot()

        with TimeIt("Get()"):
            rtc_time2 = await self.rtc.get()

        LOGGER.info("Expected RTC time %s", rtc_time1 + reboot.time_elapsed)
        LOGGER.info("Time read off RTC is: %s", rtc_time2)

        # Here, we take the time read off the chip, subtract the time spent
        # rebooting, and then subtract the benchmark time above. The delta value
        # should be close to 0.
        delta = abs(rtc_time2 - rtc_time1 - reboot.time_elapsed)
        LOGGER.info("Delta: %s", delta)

        asserts.assert_less(delta, datetime.timedelta(seconds=threshold))


if __name__ == "__main__":
    test_runner.main()
