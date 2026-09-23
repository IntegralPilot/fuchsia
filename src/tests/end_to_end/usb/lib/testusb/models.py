#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Data models, enums, and test case definitions for testusb.

Leaf module with no dependencies on runner, cli, or backends, ensuring a clean
directed acyclic graph (DAG) across the testusb package.
"""

from __future__ import annotations

import dataclasses
import enum
from typing import Any


class UsbDescriptorType(enum.IntEnum):
    """USB Descriptor Types per USB 2.0 / 3.x Specs."""

    DEVICE = 0x01
    CONFIG = 0x02
    STRING = 0x03
    INTERFACE = 0x04
    ENDPOINT = 0x05
    DEVICE_QUALIFIER = 0x06
    OTHER_SPEED_CONFIG = 0x07
    INTERFACE_POWER = 0x08


USB_DT_DEVICE = UsbDescriptorType.DEVICE
USB_DT_CONFIG = UsbDescriptorType.CONFIG
USB_DT_STRING = UsbDescriptorType.STRING
USB_DT_INTERFACE = UsbDescriptorType.INTERFACE
USB_DT_ENDPOINT = UsbDescriptorType.ENDPOINT
USB_DT_DEVICE_QUALIFIER = UsbDescriptorType.DEVICE_QUALIFIER
USB_DT_OTHER_SPEED_CONFIG = UsbDescriptorType.OTHER_SPEED_CONFIG
USB_DT_INTERFACE_POWER = UsbDescriptorType.INTERFACE_POWER

# Recognized USB Test Device VENDOR:PRODUCT IDs
KNOWN_TEST_DEVICES: tuple[tuple[int, int], ...] = (
    (0x18D1, 0xA022),  # Fuchsia USB Test Gadget (Source/Sink)
    (0x18D1, 0xA023),  # Fuchsia USB Test Gadget (Loopback)
)

# Standard Chapter 9 / EP0 generic control tests applicable across configs
EP0_GENERIC_TESTS: frozenset[int] = frozenset({0, 9, 10, 14, 21})


class TestStatus(str, enum.Enum):
    """Outcome status for a test execution."""

    PASS = "PASS"
    FAIL = "FAIL"
    SKIP = "SKIP"
    ERROR = "ERROR"


@dataclasses.dataclass
class TestParams:
    """Parameters controlling USB test execution.

    Attributes:
        iterations: Number of transfer repetitions to execute per test.
        length: Transfer buffer length in bytes.
        vary: Transfer size variation increment in bytes for varying tests.
        sglen: Number of scatter-gather entries per transfer request.
        timeout_ms: I/O completion timeout in milliseconds.
    """

    iterations: int = 1000
    length: int = 1024
    vary: int = 1024
    sglen: int = 32
    timeout_ms: int = 5000

    @property
    def buffer_size(self) -> int:
        """Alias for length representing transfer buffer size in bytes."""
        return self.length

    @buffer_size.setter
    def buffer_size(self, val: int) -> None:
        """Set transfer buffer size in bytes.

        Args:
            val: Transfer buffer length in bytes. Must be > 0.

        Raises:
            ValueError: If val is <= 0.
        """
        if val <= 0:
            raise ValueError("buffer_size must be > 0")
        self.length = val

    def __post_init__(self) -> None:
        if self.iterations <= 0:
            raise ValueError("iterations must be > 0")
        if self.length <= 0:
            raise ValueError("length must be > 0")
        if self.vary < 0:
            raise ValueError("vary must be >= 0")
        if self.sglen <= 0:
            raise ValueError("sglen must be > 0")
        if self.timeout_ms <= 0:
            raise ValueError("timeout_ms must be > 0")


@dataclasses.dataclass
class TestResult:
    """Execution result and metrics for a single USB test case.

    Attributes:
        test_id: Numerical identifier of the executed test.
        test_name: Name of the executed test.
        status: Pass/fail outcome status.
        duration_secs: Execution duration in seconds.
        error_message: Optional error message if the test failed or errored.
        iterations_completed: Count of completed test iterations.
        bytes_transferred: Total bytes transferred during test execution.
        throughput_mbps: Calculated transfer throughput in Megabits/sec.
        throughput_mbs: Calculated transfer throughput in Megabytes/sec.
    """

    test_id: int
    test_name: str
    status: TestStatus
    duration_secs: float
    error_message: str | None = None
    iterations_completed: int = 0
    bytes_transferred: int = 0
    throughput_mbps: float = 0.0
    throughput_mbs: float = 0.0

    def to_dict(self) -> dict[str, Any]:
        """Convert test result to a JSON-serializable dictionary.

        Returns:
            Dictionary mapping result attribute names to their serialized values.
        """
        return dict(
            test_id=self.test_id,
            test_name=self.name_without_id(),
            status=self.status.value,
            duration_secs=round(self.duration_secs, 6),
            error_message=self.error_message,
            iterations_completed=self.iterations_completed,
            bytes_transferred=self.bytes_transferred,
            throughput_mbps=round(self.throughput_mbps, 3),
            throughput_mbs=round(self.throughput_mbs, 3),
        )

    def name_without_id(self) -> str:
        """Return the test name without the numerical test ID prefix."""
        prefix, sep, suffix = self.test_name.partition(":")
        return suffix.strip() if sep else prefix


@dataclasses.dataclass
class TestCase:
    """Definition of a USB test case.

    Attributes:
        id: Numerical test identifier matching Linux kernel usbtest.
        name: Human-readable test name.
        description: Summary of test operations and endpoint expectations.
    """

    id: int
    name: str
    description: str


ALL_TEST_CASES: dict[int, TestCase] = {
    0: TestCase(
        0,
        "Test 0: NOP / Speed check",
        "Checks device speed and responsiveness",
    ),
    1: TestCase(1, "Test 1: OUT4K", "4KB OUT transfers to bulk endpoint"),
    2: TestCase(2, "Test 2: IN4K", "4KB IN transfers from bulk endpoint"),
    3: TestCase(3, "Test 3: OUT_VARY", "OUT transfers with varying sizes"),
    4: TestCase(4, "Test 4: IN_VARY", "IN transfers with varying sizes"),
    5: TestCase(5, "Test 5: OUT_SG", "OUT transfers with scatter/gather"),
    6: TestCase(6, "Test 6: IN_SG", "IN transfers with scatter/gather"),
    7: TestCase(
        7,
        "Test 7: OUT_VARY_SG",
        "OUT transfers with varying sizes & scatter/gather",
    ),
    8: TestCase(
        8,
        "Test 8: IN_VARY_SG",
        "IN transfers with varying sizes & scatter/gather",
    ),
    9: TestCase(9, "Test 9: CTRL_CH9", "Chapter 9 standard control tests"),
    10: TestCase(10, "Test 10: CTRL_QUEUE_32", "Queue 32 control calls"),
    11: TestCase(11, "Test 11: UNLINK_IN", "Unlink bulk IN read transfers"),
    12: TestCase(12, "Test 12: UNLINK_OUT", "Unlink bulk OUT write transfers"),
    13: TestCase(13, "Test 13: SET_HALT", "Set and clear 1000 endpoint halts"),
    14: TestCase(
        14, "Test 14: CTRL_QUEUE", "Control queue transfer permutations"
    ),
    15: TestCase(
        15,
        "Test 15: ISO_OUT",
        "Isochronous OUT transfers",
    ),
    16: TestCase(
        16,
        "Test 16: ISO_IN",
        "Isochronous IN transfers",
    ),
    17: TestCase(
        17,
        "Test 17: OUT_UNALIGNED",
        "OUT unaligned/odd address transfers",
    ),
    18: TestCase(
        18, "Test 18: IN_UNALIGNED", "IN unaligned/odd address transfers"
    ),
    19: TestCase(
        19,
        "Test 19: OUT_PREMAPPED",
        "OUT pre-mapped coherent DMA transfers",
    ),
    20: TestCase(
        20,
        "Test 20: IN_PREMAPPED",
        "IN pre-mapped coherent DMA transfers",
    ),
    21: TestCase(21, "Test 21: CTRL_WRITE", "Vendor control write transfer"),
    22: TestCase(
        22, "Test 22: ISO_OUT_VARY", "Isochronous OUT with varying sizes"
    ),
    23: TestCase(
        23, "Test 23: ISO_IN_VARY", "Isochronous IN with varying sizes"
    ),
    24: TestCase(
        24,
        "Test 24: ISO_UNLINK",
        "Isochronous transfer unlink and cancel",
    ),
    25: TestCase(
        25,
        "Test 25: INT_OUT",
        "Interrupt OUT transfer",
    ),
    26: TestCase(
        26,
        "Test 26: INT_IN",
        "Interrupt IN transfer",
    ),
    27: TestCase(
        27,
        "Test 27: STREAM_OUT_31M",
        "Bulk OUT 31Mbytes continuous stream",
    ),
    28: TestCase(
        28,
        "Test 28: STREAM_IN_31M",
        "Bulk IN 31Mbytes continuous stream",
    ),
    29: TestCase(
        29,
        "Test 29: TOGGLE_CLEAR",
        "Clear toggle between bulk writes 1000 times",
    ),
    30: TestCase(30, "Test 30: SG_QUEUE_OUT", "Scatter-gather write queue"),
    31: TestCase(31, "Test 31: SG_QUEUE_IN", "Scatter-gather read queue"),
    32: TestCase(
        32,
        "Test 32: LOOPBACK_BULK_FIXED",
        "Bidirectional bulk loopback fixed buffer size",
    ),
    33: TestCase(
        33,
        "Test 33: LOOPBACK_BULK_VARY",
        "Bidirectional bulk loopback varying buffer size",
    ),
    34: TestCase(
        34,
        "Test 34: LOOPBACK_BULK_SHORT",
        "Bidirectional bulk loopback short packets",
    ),
    35: TestCase(
        35,
        "Test 35: LOOPBACK_BULK_ZLP",
        "Bidirectional bulk loopback zero-length packets",
    ),
}
