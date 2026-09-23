# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Starnix affordance."""

import fuchsia_base_test
from honeydew import errors
from mobly import asserts, test_runner


class StarnixAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """Starnix affordance tests"""

    async def test_run_console_shell_cmd(self) -> None:
        """Test case for Starnix.run_console_shell_cmd()"""
        if self.user_params["is_starnix_supported"]:
            await self.dut.starnix.run_console_shell_cmd(["echo", "hello"])
        else:
            with asserts.assert_raises(errors.NotSupportedError):
                await self.dut.starnix.run_console_shell_cmd(["echo", "hello"])


if __name__ == "__main__":
    test_runner.main()
