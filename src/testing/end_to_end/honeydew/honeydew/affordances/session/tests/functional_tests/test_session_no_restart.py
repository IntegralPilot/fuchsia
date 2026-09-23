# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Session affordance."""

import logging

import fuchsia_base_test
from honeydew import errors
from honeydew.affordances.session import errors as session_errors
from honeydew.affordances.session import session_using_ffx
from honeydew.transports.ffx import types as ffx_types
from honeydew.utils import common
from mobly import asserts, test_runner

_LOGGER = logging.getLogger(__name__)

_TILE_URL = (
    "fuchsia-pkg://fuchsia.com/flatland-examples#meta/flatland-rainbow.cm"
)


class SessionNoRestartTestCases(fuchsia_base_test.FuchsiaTestCases):
    """Test logic for Session affordance without restart."""

    async def test_add_component(self) -> None:
        """Test case for session.add_component()"""

        self.dut.session.ensure_started()
        self.dut.session.add_component(_TILE_URL)

    async def test_add_component_wrong_url(self) -> None:
        """Test case for session.add_component() with wrong url."""

        self.dut.session.ensure_started()

        wrong_url = "INVALID_URL"

        with asserts.assert_raises(session_errors.SessionError):
            self.dut.session.add_component(wrong_url)

    async def test_add_component_twice(self) -> None:
        """Test case for session.add_component() called twice."""
        self.dut.session.ensure_started()
        self.dut.session.add_component(_TILE_URL)
        self.dut.session.add_component(_TILE_URL)

    def _elements(self) -> set[str]:
        """Get current components"""
        # Should be using JSON. TODO(b/484355868)
        res = self.dut.ffx.run(
            ["component", "list"], machine=ffx_types.MachineFormat.RAW
        )
        lines = [
            line
            for line in res.splitlines()
            if line.startswith(session_using_ffx.ELEMENT_PREFIX)
        ]
        return set(lines)

    async def test_cleanup(self) -> None:
        """Test case for session.cleanup()."""

        self.dut.session.ensure_started()

        before_add = self._elements()
        self.dut.session.add_component(_TILE_URL)
        after_add = self._elements()

        added_elements = after_add - before_add
        asserts.assert_equal(len(added_elements), 1)
        added_element = list(added_elements)[0]

        _LOGGER.info("added element: %s", added_element)

        session = self.dut.session
        session.cleanup()

        def element_removed() -> bool:
            elements = self._elements()
            _LOGGER.info("current elements: %s", elements)
            return added_element not in self._elements()

        try:
            await common.wait_for_state(
                state_fn=element_removed,
                expected_state=True,
                wait_time=2,  # Time to wait between retries in seconds
            )
        except errors.HoneydewTimeoutError:
            asserts.fail("The added element is not removed.")


class SessionAffordanceNoRestartTests(fuchsia_base_test.FuchsiaBaseTest):
    """Session affordance tests without restart

    This test suite only contains tests that do not restart the session.
    So we can keep test coverage on platform having flaky issue on session
    restart.
    """

    TEST_CASES = [SessionNoRestartTestCases]

    async def setup_class(self) -> None:
        """setup_class is called once before running tests.

        It does the following things:
            * Assigns `dut` variable with FuchsiaDevice object
        """
        await super().setup_class()

    async def teardown_test(self) -> None:
        self.dut.session.cleanup()
        await super().teardown_test()


if __name__ == "__main__":
    test_runner.main()
