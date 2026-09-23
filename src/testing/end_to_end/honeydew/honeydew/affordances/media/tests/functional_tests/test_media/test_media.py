# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Media affordance."""

import logging

import fuchsia_base_test
from honeydew.affordances.media import media
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class MediaAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """Media affordance tests"""

    async def test_get_active_session_status(self) -> None:
        """Test case for Media.get_active_session_status()"""
        status = await self.dut.media.get_active_session_status()
        if status is not None:
            assert isinstance(status, media.PlayerState)
        # Verify that a subsequent call does not hang.
        status = await self.dut.media.get_active_session_status()
        if status is not None:
            assert isinstance(status, media.PlayerState)


if __name__ == "__main__":
    test_runner.main()
