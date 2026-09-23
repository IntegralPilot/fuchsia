# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Driver liveness tracking module."""

import logging

from honeydew.fuchsia_device import fuchsia_device
from honeydew.transports.ffx import types as ffx_types
from honeydew.utils import common

_LOGGER: logging.Logger = logging.getLogger(__name__)


async def verify_driver_loaded(
    dut: fuchsia_device.FuchsiaDevice,
    driver_url: str,
    timeout: float = 30.0,
) -> None:
    """Verify that the target driver component URL enters the loaded state.

    Args:
        dut: FuchsiaDevice object.
        driver_url: Absolute component URL of the driver.
        timeout: Timeout in seconds to wait for driver recovery.

    Raises:
        HoneydewTimeoutError: If driver does not reload within timeout.
    """
    _LOGGER.info("Verifying driver '%s' enters loaded state...", driver_url)

    def _check() -> bool:
        output = dut.ffx.run(
            cmd=["driver", "list", "--loaded"],
            log_output=False,
            machine=ffx_types.MachineFormat.RAW,
        )
        return driver_url in output

    await common.wait_for_state(
        state_fn=_check,
        expected_state=True,
        timeout=timeout,
        wait_time=2.0,
    )
    _LOGGER.info(
        "Driver '%s' successfully verified in loaded state.", driver_url
    )
