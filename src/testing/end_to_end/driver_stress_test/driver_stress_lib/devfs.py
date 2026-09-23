# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Devfs node presence validation module."""

import logging

from honeydew import errors
from honeydew.fuchsia_device import fuchsia_device
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.utils import common
from mobly import signals

_LOGGER: logging.Logger = logging.getLogger(__name__)


def check_devfs_node(dut: fuchsia_device.FuchsiaDevice, path: str) -> bool:
    """Check if a specific devfs path exists on the device.

    Args:
        dut: FuchsiaDevice object.
        path: Relative path under /dev/ (e.g., 'class/gpio/000').

    Returns:
        True if the node exists, False otherwise.
    """
    full_path = f"/dev/{path}"
    _LOGGER.debug("Checking existence of devfs node '%s'...", full_path)
    try:
        dut.ffx.run_ssh_cmd(f"ls {full_path}")
        return True
    except ffx_errors.FfxCommandError:
        return False


async def assert_devfs_presence(
    dut: fuchsia_device.FuchsiaDevice,
    path: str,
    expected: bool,
    timeout: float = 10.0,
) -> None:
    """Assert that a devfs node matches the expected presence state.

    Includes a brief retry mechanism for dynamic nodes settling during recovery.

    Args:
        dut: FuchsiaDevice object.
        path: Relative path under /dev/.
        expected: True to assert presence, False to assert absence.
        timeout: Timeout in seconds to wait for expected state.

    Raises:
        TestFailure: If node fails to achieve expected state.
    """
    _LOGGER.info("Asserting devfs node '%s' presence is %s...", path, expected)
    try:
        await common.wait_for_state(
            state_fn=lambda: check_devfs_node(dut, path),
            expected_state=expected,
            timeout=timeout,
            wait_time=1.0,
        )
        _LOGGER.info(
            "Successfully verified devfs node '%s' presence is %s.",
            path,
            expected,
        )
    except errors.HoneydewTimeoutError as err:
        raise signals.TestFailure(
            f"Devfs node '{path}' failed to reach expected presence state: {expected}"
        ) from err
