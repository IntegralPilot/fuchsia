# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fuchsia_base_test
from honeydew.transports.ffx import types as ffx_types
from mobly import asserts

_LOGGER: logging.Logger = logging.getLogger(__name__)


class RebootReasonTestCases(fuchsia_base_test.FuchsiaTestCases):
    """Test cases for reboot reasons."""

    async def test_reboot_reason(self) -> None:
        boot_id_before_reboot_file = self.output_file_path(
            "boot_id_before_reboot.txt"
        )
        previous_boot_id_after_reboot_file = self.output_file_path(
            "previous_boot_id_after_reboot.txt"
        )

        self.dut.ffx.run(
            [
                "component",
                "copy",
                "core/feedback::/data/boot_id.txt",
                str(boot_id_before_reboot_file),
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

        _LOGGER.info("[test_reboot_reason] Rebooting device...")
        # Under the hood, this makes a FIDL call over
        # fuchsia.hardware.power.statecontrol/Admin::Shutdown() with shutdown reason
        # DEVELOPER_REQUEST.
        await self.dut.reboot()
        _LOGGER.info("[test_reboot_reason] Device has rebooted successfully")

        self.dut.ffx.run(
            [
                "component",
                "copy",
                "core/feedback::/tmp/boot_id.txt",
                str(previous_boot_id_after_reboot_file),
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

        # We always expect "DEVELOPER_REQUEST", but now that we have set up the test, we are finding
        # instances of true bugs where something goes wrong during shutdown. To help with
        # https://fxbug.dev/432864757, we want a different message on failure depending on what
        # the reason is to better inform whoemever is looking at the test failure.
        last_reboot_reason = await self.dut.last_reboot_reason()
        if last_reboot_reason == "ROOT_JOB_TERMINATION":
            asserts.assert_equal(
                last_reboot_reason,
                "DEVELOPER_REQUEST",
                msg=(
                    "There was likely a driver hang during userspace shutdown. See"
                    " serial_log.txt and https://fxbug.dev/432968401"
                ),
            )
        elif last_reboot_reason in [
            "COLD",
            "BRIEF_POWER_LOSS",
        ]:
            with open(boot_id_before_reboot_file, "r") as bid_file:
                with open(previous_boot_id_after_reboot_file, "r") as pbid_file:
                    boot_id_before_reboot = bid_file.read()
                    previous_boot_id_after_reboot = pbid_file.read()
                    asserts.assert_equal(
                        boot_id_before_reboot,
                        previous_boot_id_after_reboot,
                        msg="There was at least one extra spurious reboot. See serial_log.txt and https://fxbug.dev/479305824",
                    )
            asserts.assert_equal(
                last_reboot_reason,
                "DEVELOPER_REQUEST",
                msg=(
                    "There was likely a hardware reboot due to a driver action during"
                    " userspace shutdown. See serial_log.txt and"
                    " https://fxbug.dev/433253369"
                ),
            )
        else:
            asserts.assert_equal(last_reboot_reason, "DEVELOPER_REQUEST")
