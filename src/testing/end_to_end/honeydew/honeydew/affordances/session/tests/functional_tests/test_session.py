# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Session affordance."""

import logging
import time

import fuchsia_base_test
from honeydew.affordances.session import errors as session_errors
from mobly import asserts, test_runner

_LOGGER = logging.getLogger(__name__)

_TILE_URL = (
    "fuchsia-pkg://fuchsia.com/flatland-examples#meta/flatland-rainbow.cm"
)


class SessionAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """Session affordance tests"""

    async def setup_class(self) -> None:
        """setup_class is called once before running tests.

        It does the following things:
            * Assigns `dut` variable with FuchsiaDevice object
        """
        await super().setup_class()

    async def test_add_component_without_started_session(self) -> None:
        """Test case for calling session.add_component() without started
        session.

        Ensure it is not a timeout error.
        """

        self.dut.session.stop()

        with asserts.assert_raises(session_errors.SessionError):
            self.dut.session.add_component(_TILE_URL)

    async def test_start_multiple(self) -> None:
        """Test case for session.start() called multiple times."""

        self.dut.session.ensure_started()

        # start new session
        self.dut.session.start()

        # Give the system a chance to fully start the session before starting
        # the second session.
        _LOGGER.info("Waiting for session to fully start up...")
        time.sleep(10)

        self.dut.session.add_component(_TILE_URL)

    async def test_stop_stopped_session(self) -> None:
        """Test case for session.stop() called multiple times."""

        self.dut.session.ensure_started()
        self.dut.session.stop()
        self.dut.session.stop()

    async def test_restart_session_stopped_session(self) -> None:
        """Test case for session.restart() starting a stopped session."""

        self.dut.session.ensure_started()
        started = self.dut.session.is_started()
        asserts.assert_true(started, "after session start")

        self.dut.session.stop()

        started = self.dut.session.is_started()
        asserts.assert_false(started, "after session stop")

        with asserts.assert_raises(session_errors.SessionError):
            # restart when session is stopped will get error: Not Running
            self.dut.session.restart()

    async def test_restart_session_started_session(self) -> None:
        """Test case for session.restart() restarting a started session."""

        self.dut.session.ensure_started()
        self.dut.session.restart()

        # Give the system a chance to fully start the session before starting
        # the second session.
        _LOGGER.info("Waiting for session to fully start up...")
        time.sleep(10)

        self.dut.session.add_component(_TILE_URL)


if __name__ == "__main__":
    test_runner.main()
