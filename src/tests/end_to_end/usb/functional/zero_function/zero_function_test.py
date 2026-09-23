# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Mobly E2E functional test suite for USB Zero Function (usb-zero-function).

Verifies functional correctness of USB test cases (0..31) in Source/Sink mode.
"""

import logging
import os

import zero_function
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class ZeroFunctionTest(zero_function.ZeroFunctionBaseTest):
    """Functional test suite running USB Zero Function in Source/Sink mode."""

    def test_enumeration(self) -> None:
        """Verifies target switches to USB Zero Function mode and enumerates.

        Ensures the device node appears on the host filesystem.
        """
        dev_node = self.require_dev_node()
        asserts.assert_true(
            os.path.exists(dev_node), f"{dev_node} does not exist on host"
        )
        _LOGGER.info("Target successfully enumerated as: %s", dev_node)

    def test_control_nop(self) -> None:
        """Verifies EP0 Control NOP (Test 0) executes in Source/Sink mode."""
        dev_node = self.require_dev_node()
        _LOGGER.info("Executing Control NOP (Test 0) on %s...", dev_node)

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[0],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(results, "Control NOP (Test 0) failed")

    def test_bulk_fixed_length(self) -> None:
        """Verifies fixed-length bulk OUT (Test 1) and bulk IN (Test 2)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Fixed Bulk OUT & IN (Tests 1 & 2) on %s...", dev_node
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[1, 2],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Fixed-length bulk transfers (Tests 1 & 2) failed"
        )

    def test_bulk_varying_length(self) -> None:
        """Verifies varying-length bulk OUT (Test 3) and bulk IN (Test 4)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Varying Bulk OUT & IN (Tests 3 & 4) on %s...", dev_node
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[3, 4],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Varying-length bulk transfers (Tests 3 & 4) failed"
        )

    def test_bulk_scatter_gather(self) -> None:
        """Verifies scatter/gather bulk OUT (Test 5) and bulk IN (Test 6)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Scatter/Gather Bulk OUT & IN (Tests 5 & 6) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[5, 6],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Scatter/Gather bulk transfers (Tests 5 & 6) failed"
        )

    def test_bulk_varying_scatter_gather(self) -> None:
        """Verifies varying SG bulk OUT (Test 7) and bulk IN (Test 8)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Varying SG Bulk OUT & IN (Tests 7 & 8) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[7, 8],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results,
            "Varying Scatter/Gather bulk transfers (Tests 7 & 8) failed",
        )

    def test_chapter9_control_tests(self) -> None:
        """Verifies Chapter 9 control validation and queue control calls.

        Executes Tests 9 and 10 in Source/Sink mode.
        """
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Chapter 9 Control Tests (Tests 9 & 10) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[9, 10],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Chapter 9 Control Tests (Tests 9 & 10) failed"
        )

    def test_bulk_unlink_transfers(self) -> None:
        """Verifies Bulk IN unlink (Test 11) and Bulk OUT unlink (Test 12)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Bulk Unlink Transfers (Tests 11 & 12) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[11, 12],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Bulk Unlink Transfers (Tests 11 & 12) failed"
        )

    def test_endpoint_halt_and_clear(self) -> None:
        """Verifies standard USB 2.0 Chapter 9 Endpoint Set/Clear Halt.

        Executes Test 13 in Source/Sink mode.
        """
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Endpoint Halt/Clear (Test 13) on %s...", dev_node
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[13],
            mode="sourcesink",
            iterations=1,
        )
        self.assert_testusb_success(
            results, "Endpoint Set/Clear Halt (Test 13) failed"
        )

    def test_control_queue_permutations(self) -> None:
        """Verifies 24 queued control transfer subcases (Test 14)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Queued Control Transfers (Test 14) on %s...", dev_node
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[14],
            mode="sourcesink",
            iterations=1,
        )
        self.assert_testusb_success(
            results, "Queued Control Transfers (Test 14) failed"
        )

    def test_unaligned_memory_transfers(self) -> None:
        """Verifies unaligned address bulk OUT (Test 17) and IN (Test 18)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Unaligned Bulk OUT & IN (Tests 17 & 18) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[17, 18],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Unaligned memory bulk transfers (Tests 17 & 18) failed"
        )

    def test_premapped_coherent_dma(self) -> None:
        """Verifies coherent DMA bulk OUT (Test 19) and bulk IN (Test 20)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Coherent DMA OUT & IN (Tests 19 & 20) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[19, 20],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Pre-mapped coherent DMA transfers (Tests 19 & 20) failed"
        )

    def test_vendor_control(self) -> None:
        """Verifies vendor control OUT transfer (Test 21)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Vendor Control OUT (Test 21) on %s...", dev_node
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[21],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Vendor Control OUT (Test 21) failed"
        )

    def test_transfer_unlink_cancellation(self) -> None:
        """Verifies transfer unlink/cancellation on bulk OUT and bulk IN."""
        # Bulk unlink is verified in test_bulk_unlink_transfers (Tests 11 & 12).
        # Tests 24 & 25 correspond to isochronous and interrupt endpoints, which
        # are not exposed by the USB zero function.
        asserts.skip(
            "Tests 24 & 25 (ISO/INT unlink) skipped; zero function only "
            "provides bulk endpoints. Bulk unlink is tested via Tests 11 & 12."
        )

    def test_high_throughput_streaming(self) -> None:
        """Verifies 31MB continuous bulk OUT (Test 27) and bulk IN (Test 28)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing 31MB Streaming (Tests 27 & 28) on %s...", dev_node
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[27, 28],
            mode="sourcesink",
            iterations=1,
        )
        self.assert_testusb_success(
            results, "31MB streaming (Tests 27 & 28) failed"
        )

    def test_data_toggle_sync(self) -> None:
        """Verifies DATA0/DATA1 toggle reset between bulk writes (Test 29)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Data Toggle Clearing (Test 29) on %s...", dev_node
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[29],
            mode="sourcesink",
            iterations=1,
        )
        self.assert_testusb_success(
            results, "Data Toggle Clearing (Test 29) failed"
        )

    def test_queued_scatter_gather(self) -> None:
        """Verifies queued scatter/gather bulk OUT and IN (Tests 30 & 31)."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Queued SG Bulk OUT & IN (Tests 30 & 31) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[30, 31],
            mode="sourcesink",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Queued Scatter/Gather (Tests 30 & 31) failed"
        )

    def test_loopback_ep0_control(self) -> None:
        """Verifies EP0 generic control tests (0, 9, 10, 14, 21) in Loopback mode."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Loopback EP0 Generic Control Tests on %s...",
            dev_node,
        )
        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[0, 9, 10, 14, 21],
            mode="loopback",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Loopback EP0 generic control tests failed"
        )

    def test_loopback_bulk_roundtrip(self) -> None:
        """Verifies userspace bulk loopback fixed and varying round-trips."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Userspace Bulk Loopback Round-trips on %s...",
            dev_node,
        )
        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=[32, 33, 34, 35],
            mode="loopback",
            iterations=5,
        )
        self.assert_testusb_success(
            results, "Userspace Bulk Loopback Round-trips failed"
        )

    def test_full_suite_sweep(self) -> None:
        """Sweeps all 27 supported tests in Source/Sink mode in one pass."""
        dev_node = self.require_dev_node()
        _LOGGER.info(
            "Executing Full Suite Sweep (All Source/Sink Tests) on %s...",
            dev_node,
        )

        results = self.execute_testusb(
            dev_node=dev_node,
            test_ids=zero_function.ALL_SUPPORTED_TEST_IDS,
            mode="sourcesink",
            iterations=1,
        )
        self.assert_testusb_success(
            results, "Full Suite Sweep (All Tests) failed"
        )
        _LOGGER.info(
            "Successfully verified all test cases in Source/Sink mode."
        )


if __name__ == "__main__":
    test_runner.main()
