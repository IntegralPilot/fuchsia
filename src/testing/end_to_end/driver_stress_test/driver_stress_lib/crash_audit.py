# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Driver system health and crash auditing module."""

import logging

from honeydew.fuchsia_device import fuchsia_device
from honeydew.transports.ffx import types as ffx_types
from mobly import signals

_LOGGER: logging.Logger = logging.getLogger(__name__)


def audit_driver_crashes(
    dut: fuchsia_device.FuchsiaDevice,
    moniker: str,
    start_time: str | None = None,
) -> None:
    """Audit system logs for unhandled crashes or fatal errors matching the driver.

    Scans recent logs for critical ERROR strings or symbolized backtrace syntax.

    Args:
        dut: FuchsiaDevice object.
        moniker: Component moniker of the driver (e.g., 'gpio').
        start_time: Optional start timestamp string for filtering logs via '--since'.

    Raises:
        TestFailure: If fatal errors or target crashes are detected.
    """
    _LOGGER.info("Auditing system logs for driver '%s' crashes...", moniker)

    cmd_exceptions = ["log", "--filter", "exceptions", "--severity", "error"]
    if start_time:
        cmd_exceptions.extend(["--since", start_time])
    cmd_exceptions.append("dump")

    exception_logs = dut.ffx.run(
        cmd=cmd_exceptions,
        log_output=False,
        machine=ffx_types.MachineFormat.RAW,
    )

    if moniker in exception_logs:
        _LOGGER.error(
            "Fatal exception logged by Zircon exception broker for '%s'!",
            moniker,
        )
        raise signals.TestFailure(
            f"Fatal unhandled exception detected for driver '{moniker}'."
        )

    cmd_logs = ["log", "--symbolize", "off"]
    if start_time:
        cmd_logs.extend(["--since", start_time])
    cmd_logs.append("dump")

    logs = dut.ffx.run(
        cmd=cmd_logs,
        log_output=False,
        machine=ffx_types.MachineFormat.RAW,
    )

    if "{{{bt:" in logs:
        _LOGGER.error("Symbolized backtrace markup detected in system logs!")
        raise signals.TestFailure(
            f"Fatal backtrace detected during stress testing of '{moniker}'."
        )

    for line in logs.splitlines():
        if "ERROR" in line and f"[{moniker}]" in line:
            _LOGGER.error("Target driver error log detected: %s", line)
            raise signals.TestFailure(
                f"Critical error logged by driver '{moniker}': {line}"
            )

    _LOGGER.info(
        "Crash audit completed successfully. No fatal errors detected."
    )
