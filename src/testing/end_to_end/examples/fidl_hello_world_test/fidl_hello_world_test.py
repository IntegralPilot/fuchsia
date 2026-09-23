# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Example test demonstrating direct FIDL interaction via Fuchsia Controller."""

import logging

import fidl_fuchsia_buildinfo as f_buildinfo
import fuchsia_base_test
from honeydew.typing.custom_types import FidlEndpoint
from mobly import asserts, test_runner

logger = logging.getLogger(__name__)


class FidlHelloWorldTest(fuchsia_base_test.FuchsiaBaseTest):
    """Example test that directly connects to and queries a FIDL protocol."""

    async def setup_class(self) -> None:
        await super().setup_class()
        self.build_info_proxy = f_buildinfo.ProviderClient(
            self.dut.fuchsia_controller.connect_device_proxy(
                FidlEndpoint(
                    moniker="/core/build-info",
                    protocol="fuchsia.buildinfo.Provider",
                )
            )
        )

    async def test_get_build_info(self) -> None:
        """Queries build info directly from fuchsia.buildinfo.Provider."""
        response = await self.build_info_proxy.get_build_info()
        build_info = response.build_info
        logger.info(f"Retrieved build info via FIDL: {build_info}")
        asserts.assert_is_not_none(
            build_info, "Expected build info to be present."
        )


if __name__ == "__main__":
    test_runner.main()
