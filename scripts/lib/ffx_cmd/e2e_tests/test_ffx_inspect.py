# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import ffx_cmd
import fuchsia_base_test
import fuchsia_inspect
from mobly import asserts, test_runner

_LOGGER = logging.getLogger(__name__)


class TestFfxInspect(fuchsia_base_test.FuchsiaBaseTest):
    async def test_ffx_inspect_all_data(self) -> None:
        """ensure that ffx_cmd.inspect can read data from a real device"""
        inner = ffx_cmd.FfxCmd.create_test_inner(
            *self.dut.ffx.generate_ffx_cmd(cmd=[])
        )
        _LOGGER.info("Selecting inspect data")

        ret: fuchsia_inspect.InspectDataCollection = await ffx_cmd.inspect(
            inner=inner
        ).sync()

        asserts.assert_greater(len(ret.data), 1)

        val = next(
            (val for val in ret.data if val.moniker == "bootstrap/archivist")
        )
        asserts.assert_is_not_none(val, "Expected to find archivist data")

        assert val.payload is not None
        asserts.assert_equal(
            val.payload["root"]["fuchsia.inspect.Health"]["status"], "OK"
        )


if __name__ == "__main__":
    test_runner.main()
