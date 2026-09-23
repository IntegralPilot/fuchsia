#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for testusb."""

import argparse
import collections.abc
import contextlib
import ctypes
import errno
import io
import json
import os
import tempfile
import unittest
from typing import Any
from unittest.mock import MagicMock, patch

from testusb import backend, cli, ioctl
from testusb.cli import main, parse_test_selection
from testusb.models import (
    TestParams,
    TestResult,
    TestStatus,
)
from testusb.runner import (
    JSONReporter,
    TestRunner,
    TextReporter,
)


class TestParseTestSelection(unittest.TestCase):
    """Tests for parse_test_selection parsing ranges, lists, keywords."""

    def test_parse_test_selection_single_id(self) -> None:
        self.assertEqual(parse_test_selection("0"), [0])
        self.assertEqual(parse_test_selection("31"), [31])

    def test_parse_test_selection_comma_separated_ids(self) -> None:
        self.assertEqual(parse_test_selection("0,1,2"), [0, 1, 2])
        self.assertEqual(parse_test_selection("5, 2, 10"), [2, 5, 10])

    def test_range_of_tests(self) -> None:
        self.assertEqual(parse_test_selection("0-5"), [0, 1, 2, 3, 4, 5])
        self.assertEqual(parse_test_selection("0-31"), list(range(32)))

    def test_all_keyword(self) -> None:
        self.assertEqual(parse_test_selection("all"), list(range(36)))
        self.assertEqual(parse_test_selection("ALL"), list(range(36)))

    def test_none_keyword(self) -> None:
        self.assertEqual(parse_test_selection("none"), [])
        self.assertEqual(parse_test_selection("NONE"), [])

    def test_complex_selection(self) -> None:
        self.assertEqual(
            parse_test_selection("1,2,5-10, 15"),
            [1, 2, 5, 6, 7, 8, 9, 10, 15],
        )

    def test_parse_test_selection_invalid_raises_value_error(self) -> None:
        invalid_inputs = [
            "foo",
            "1-",
            "-5",
            "5-2",  # start > end
            "-1",
            "36",  # out of range 0..35
            "1,2,36",
            "",
            "   ",
        ]
        for invalid in invalid_inputs:
            with self.subTest(invalid=invalid):
                with self.assertRaises(ValueError):
                    parse_test_selection(invalid)


class TestTestParams(unittest.TestCase):
    """Tests for TestParams dataclass validation and default values."""

    def test_params_default_values(self) -> None:
        params = TestParams()
        self.assertEqual(params.iterations, 1000)
        self.assertEqual(params.length, 1024)
        self.assertEqual(params.buffer_size, 1024)
        self.assertEqual(params.vary, 1024)
        self.assertEqual(params.sglen, 32)
        self.assertEqual(params.timeout_ms, 5000)

    def test_buffer_size_property_sync(self) -> None:
        params = TestParams(length=1024)
        self.assertEqual(params.buffer_size, 1024)
        params.buffer_size = 2048
        self.assertEqual(params.length, 2048)
        self.assertEqual(params.buffer_size, 2048)
        with self.assertRaises(ValueError):
            params.buffer_size = 0
        with self.assertRaises(ValueError):
            params.buffer_size = -1

    def test_invalid_parameters(self) -> None:
        with self.assertRaises(ValueError):
            TestParams(iterations=0)
        with self.assertRaises(ValueError):
            TestParams(iterations=-1)
        with self.assertRaises(ValueError):
            TestParams(length=0)
        with self.assertRaises(ValueError):
            TestParams(length=-1)
        with self.assertRaises(ValueError):
            TestParams(vary=-1)
        with self.assertRaises(ValueError):
            TestParams(sglen=0)
        with self.assertRaises(ValueError):
            TestParams(sglen=-1)
        with self.assertRaises(ValueError):
            TestParams(timeout_ms=0)


class TestEndpointParsing(unittest.TestCase):
    """Tests for USB configuration descriptor endpoint parsing."""

    def test_single_interface_both_endpoints(self) -> None:
        # Config (9 bytes) + Interface 0 (9 bytes) + EP1 OUT (7 bytes) +
        # EP1 IN (7 bytes)
        # fmt: off
        raw = bytes([
            # Config descriptor (9 bytes)
            0x09, 0x02, 0x20, 0x00, 0x01, 0x01, 0x00, 0xC0, 0x64,
            # Interface 0 descriptor (9 bytes)
            0x09, 0x04, 0x00, 0x00, 0x02, 0xFF, 0x00, 0x00, 0x00,
            # EP1 OUT descriptor (7 bytes)
            0x07, 0x05, 0x01, 0x02, 0x40, 0x00, 0x00,
            # EP1 IN descriptor (7 bytes)
            0x07, 0x05, 0x81, 0x02, 0x40, 0x00, 0x00,
        ])
        # fmt: on
        out_ep, in_ep, ifnum = ioctl.parse_endpoints_from_config_descriptor(raw)
        self.assertEqual(out_ep, 0x01)
        self.assertEqual(in_ep, 0x81)
        self.assertEqual(ifnum, 0)

    def test_fallback_does_not_cross_interfaces(self) -> None:
        # Config with 2 interfaces: IF0 has only EP1 OUT; IF1 has only EP2 IN.
        # Fallback must return from one interface only, never mixing IF0
        # OUT with IF1 IN.
        # fmt: off
        raw = bytes([
            # Config descriptor (9 bytes)
            0x09, 0x02, 0x29, 0x00, 0x02, 0x01, 0x00, 0xC0, 0x64,
            # Interface 0: EP1 OUT
            0x09, 0x04, 0x00, 0x00, 0x01, 0xFF, 0x00, 0x00, 0x00,
            0x07, 0x05, 0x01, 0x02, 0x40, 0x00, 0x00,
            # Interface 1: EP2 IN
            0x09, 0x04, 0x01, 0x00, 0x01, 0xFF, 0x00, 0x00, 0x00,
            0x07, 0x05, 0x82, 0x02, 0x40, 0x00, 0x00,
        ])
        # fmt: on
        out_ep, in_ep, ifnum = ioctl.parse_endpoints_from_config_descriptor(raw)
        self.assertEqual(ifnum, 0)
        self.assertEqual(out_ep, 0x01)
        self.assertIsNone(in_ep)

    def test_no_endpoints_returns_none(self) -> None:
        # fmt: off
        raw = bytes([
            # Config descriptor (9 bytes)
            0x09, 0x02, 0x12, 0x00, 0x01, 0x01, 0x00, 0xC0, 0x64,
            # Interface descriptor (9 bytes)
            0x09, 0x04, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00,
        ])
        # fmt: on
        out_ep, in_ep, ifnum = ioctl.parse_endpoints_from_config_descriptor(raw)
        self.assertIsNone(out_ep)
        self.assertIsNone(in_ep)
        self.assertIsNone(ifnum)


class MockUSBTestBackend(backend.USBTestBackend):
    """Mock USB backend for testing TestRunner without hardware."""

    def __init__(
        self,
        device_path: str | None = "/dev/bus/usb/001/002",
        fail_tests: collections.abc.Sequence[int] | None = None,
    ) -> None:
        super().__init__(device_path)
        self.fail_tests = list(fail_tests) if fail_tests else []
        self.executed_tests: list[int] = []
        self.configs_set: list[int] = []
        self.custom_run_test: (
            collections.abc.Callable[[int, TestParams], TestResult] | None
        ) = None

    def open(self) -> None:
        """Sets mock device state to open."""
        self.is_open = True
        self.speed_name = "high"

    def close(self) -> None:
        """Sets mock device state to closed."""
        self.is_open = False

    def get_speed(self) -> str:
        """Returns mock connection speed."""
        return "high"

    def set_configuration(self, config_val: int) -> bool:
        """Sets mock device configuration."""
        self.current_config = config_val
        self.configs_set.append(config_val)
        return True

    def run_bulk_loopback(
        self,
        params: TestParams,
        length_vary: bool = False,
        is_short: bool = False,
        is_zlp: bool = False,
        test_id: int = 0,
        test_name: str = "Loopback: BULK_ROUNDTRIP",
        ep_out: int | None = None,
        ep_in: int | None = None,
    ) -> TestResult:
        """Executes mock bulk loopback transfer."""
        self.executed_tests.append(test_id)
        if test_id in self.fail_tests:
            return TestResult(
                test_id=test_id,
                test_name=test_name,
                status=TestStatus.FAIL,
                duration_secs=0.01,
                error_message="Mock loopback failure",
            )
        return TestResult(
            test_id=test_id,
            test_name=test_name,
            status=TestStatus.PASS,
            duration_secs=0.005,
            iterations_completed=params.iterations,
            bytes_transferred=params.iterations * params.length,
            throughput_mbps=120.0,
        )

    @contextlib.contextmanager
    def claimed_interface(self, ifnum: int) -> collections.abc.Iterator[None]:
        """Context manager for claiming mock interface."""
        yield

    def run_test(self, test_id: int, params: TestParams) -> TestResult:
        """Executes mock test case by ID."""
        self.executed_tests.append(test_id)
        if self.custom_run_test is not None:
            return self.custom_run_test(test_id, params)
        if test_id in self.fail_tests:
            return TestResult(
                test_id=test_id,
                test_name=f"Test {test_id}",
                status=TestStatus.FAIL,
                duration_secs=0.01,
                error_message="Mock failure",
            )
        return TestResult(
            test_id=test_id,
            test_name=f"Test {test_id}",
            status=TestStatus.PASS,
            duration_secs=0.005,
            iterations_completed=params.iterations,
            bytes_transferred=params.iterations * params.length,
            throughput_mbps=100.0,
        )


class TestTestRunner(unittest.TestCase):
    """Tests for TestRunner orchestration, loops, modes, configurations."""

    def test_single_loop_execution(self) -> None:
        backend = MockUSBTestBackend()
        params = TestParams(iterations=10, length=512)
        runner = TestRunner(backend, params, loop_count=1, quiet=True)

        results = runner.run([0, 1, 2])
        self.assertEqual(len(results), 3)
        self.assertEqual(backend.executed_tests, [0, 1, 2])
        self.assertEqual(
            [r.status for r in results],
            [TestStatus.PASS] * len(results),
        )

    def test_multiple_loops(self) -> None:
        backend = MockUSBTestBackend()
        params = TestParams()
        runner = TestRunner(backend, params, loop_count=3, quiet=True)

        results = runner.run([0, 1])
        self.assertEqual(len(results), 6)
        self.assertEqual(backend.executed_tests, [0, 1, 0, 1, 0, 1])

    def test_random_order(self) -> None:
        backend = MockUSBTestBackend()
        params = TestParams()
        runner = TestRunner(
            backend, params, loop_count=10, random_order=True, quiet=True
        )

        results = runner.run(list(range(10)))
        self.assertEqual(len(results), 100)
        # Check that it wasn't strictly sequential all 10 times
        self.assertNotEqual(backend.executed_tests, list(range(10)) * 10)

    def test_sourcesink_mode(self) -> None:
        backend = MockUSBTestBackend()
        params = TestParams()
        runner = TestRunner(backend, params, mode="sourcesink", quiet=True)

        runner.run([1, 2, 3])
        self.assertEqual(backend.configs_set, [1])
        self.assertEqual(backend.executed_tests, [1, 2, 3])

    def test_loopback_mode(self) -> None:
        backend = MockUSBTestBackend()
        params = TestParams()
        runner = TestRunner(backend, params, mode="loopback", quiet=True)

        runner.run([9, 10])
        self.assertEqual(backend.configs_set, [2])
        self.assertEqual(backend.executed_tests, [9, 10])

    def test_both_mode_dual_phase(self) -> None:
        backend = MockUSBTestBackend()
        params = TestParams()
        runner = TestRunner(backend, params, mode="both", quiet=True)

        # Test selection: 1 (source/sink), 9 (control)
        results = runner.run([1, 9])
        self.assertEqual(backend.configs_set, [1, 2])
        self.assertIn(1, backend.executed_tests)
        # Phase 1 runs [1, 9] (2) on Config 1 + Phase 2 runs [9] (1) on
        # Config 2 = 3 total
        self.assertEqual(len(results), 3)

    def test_explicit_config_override(self) -> None:
        backend = MockUSBTestBackend()
        params = TestParams()
        runner = TestRunner(backend, params, config=2, quiet=True)

        runner.run([0])
        self.assertEqual(backend.configs_set, [2])


class TestReporters(unittest.TestCase):
    """Tests for TextReporter and JSONReporter formatted report output."""

    def setUp(self) -> None:
        self.results = [
            TestResult(
                test_id=0,
                test_name="Test 0: NOP",
                status=TestStatus.PASS,
                duration_secs=0.001,
                iterations_completed=1000,
                bytes_transferred=1024000,
                throughput_mbps=8192.0,
            ),
            TestResult(
                test_id=1,
                test_name="Test 1: OUT4K",
                status=TestStatus.FAIL,
                duration_secs=0.05,
                error_message="Timeout",
            ),
            TestResult(
                test_id=5,
                test_name="Test 5: OUT_SG",
                status=TestStatus.SKIP,
                duration_secs=0.0,
                error_message="Not supported",
            ),
        ]
        self.params = TestParams()

    def test_text_reporter(self) -> None:
        report = TextReporter.report(
            results=self.results,
            device_path="/dev/bus/usb/001/002",
            speed="high",
            backend_name="ioctl",
            total_duration=0.1,
        )
        self.assertIn("USB Test Results", report)
        self.assertIn("/dev/bus/usb/001/002", report)
        self.assertIn("high", report)
        self.assertIn("ioctl", report)
        self.assertIn("PASS", report)
        self.assertIn("FAIL", report)
        self.assertIn("SKIP", report)
        self.assertIn(
            "Summary: 3 executed, 1 passed, 1 failed, 1 skipped, 0 errors",
            report,
        )

    def test_json_reporter(self) -> None:
        report_dict = JSONReporter.report(
            results=self.results,
            device_path="/dev/bus/usb/001/002",
            speed="high",
            backend_name="ioctl",
            total_duration=0.1,
            params=self.params,
        )
        self.assertEqual(
            report_dict["metadata"]["device_path"], "/dev/bus/usb/001/002"
        )
        self.assertEqual(report_dict["metadata"]["speed"], "high")
        self.assertEqual(report_dict["metadata"]["backend"], "ioctl")
        self.assertEqual(report_dict["summary"]["total_executed"], 3)
        self.assertEqual(report_dict["summary"]["passed"], 1)
        self.assertEqual(report_dict["summary"]["failed"], 1)
        self.assertEqual(report_dict["summary"]["skipped"], 1)
        self.assertEqual(report_dict["summary"]["success_rate"], 50.0)
        self.assertEqual(len(report_dict["results"]), 3)

        # Check result structure
        res0 = report_dict["results"][0]
        self.assertEqual(res0["test_id"], 0)
        self.assertEqual(res0["test_name"], "NOP")
        self.assertEqual(res0["status"], "PASS")


# fmt: off
MOCK_CONFIG2_DESC = bytes([
    # Config descriptor (9 bytes)
    0x09, 0x02, 0x20, 0x00, 0x01, 0x02, 0x00, 0xC0, 0xFA,
    # Interface descriptor (9 bytes)
    0x09, 0x04, 0x00, 0x00, 0x02, 0xFF, 0x00, 0x00, 0x00,
    # Endpoint OUT (7 bytes): EP2 OUT
    0x07, 0x05, 0x02, 0x02, 0x00, 0x02, 0x00,
    # Endpoint IN (7 bytes): EP2 IN (0x82)
    0x07, 0x05, 0x82, 0x02, 0x00, 0x02, 0x00,
])
# fmt: on


class TestIoctlBackendLoopback(unittest.TestCase):
    """Tests for USBTestIoctlBackend loopback and descriptor parsing."""

    def test_ioctl_set_configuration_sysfs(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True

        with patch.object(
            backend, "_get_sysfs_device_path", return_value="/fake/sysfs"
        ):
            with (
                patch("os.path.exists", return_value=True),
                patch("os.access", return_value=True),
                patch("builtins.open", unittest.mock.mock_open()) as mock_file,
            ):
                ret = backend.set_configuration(2)
                self.assertTrue(ret)
                self.assertEqual(backend.current_config, 2)
                mock_file().write.assert_called_with("2")

    def test_ioctl_set_configuration_ioctl(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True

        mock_cdll = MagicMock()
        mock_cdll.ioctl.return_value = 0
        with patch.object(backend, "_get_sysfs_device_path", return_value=None):
            with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
                ret = backend.set_configuration(2)
                self.assertTrue(ret)
                self.assertEqual(backend.current_config, 2)
                self.assertGreaterEqual(mock_cdll.ioctl.call_count, 1)

    def test_ioctl_run_bulk_loopback_success(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True
        backend.speed_name = "high"
        backend.current_config = 2

        mock_cdll = MagicMock()

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_CONTROL:
                ctrl = arg_ptr._obj
                if ctrl.bRequestType & 0x80 and ctrl.bRequest == 0x06:
                    desc_type = (ctrl.wValue >> 8) & 0xFF
                    if desc_type == 0x02:
                        to_copy = min(ctrl.wLength, len(MOCK_CONFIG2_DESC))
                        buf = ctypes.cast(
                            ctrl.data, ctypes.POINTER(ctypes.c_char * to_copy)
                        )
                        buf.contents.raw = MOCK_CONFIG2_DESC[:to_copy]
                        return to_copy
                return 0
            if request == ioctl.USBDEVFS_BULK:
                bulk = arg_ptr._obj
                if bulk.ep & 0x80:
                    # In endpoint: copy data back to in_buf
                    in_buf_ptr = ctypes.cast(
                        bulk.data, ctypes.POINTER(ctypes.c_char * bulk.len)
                    )
                    # For iteration 0 and 1, match payload
                    in_buf_ptr.contents.raw = bytes(
                        (j + 0) % 256 for j in range(bulk.len)
                    )
                return bulk.len
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            params = TestParams(iterations=1, length=512)
            res = backend.run_bulk_loopback(
                params, length_vary=False, test_id=9
            )
            self.assertEqual(res.status, TestStatus.PASS)
            self.assertEqual(res.iterations_completed, 1)
            self.assertEqual(res.bytes_transferred, 512 * 2)

    def test_ioctl_run_bulk_loopback_mismatch_failure(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True
        backend.speed_name = "high"
        backend.current_config = 2

        mock_cdll = MagicMock()

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_CONTROL:
                ctrl = arg_ptr._obj
                if ctrl.bRequestType & 0x80 and ctrl.bRequest == 0x06:
                    desc_type = (ctrl.wValue >> 8) & 0xFF
                    if desc_type == 0x02:
                        to_copy = min(ctrl.wLength, len(MOCK_CONFIG2_DESC))
                        buf = ctypes.cast(
                            ctrl.data, ctypes.POINTER(ctypes.c_char * to_copy)
                        )
                        buf.contents.raw = MOCK_CONFIG2_DESC[:to_copy]
                        return to_copy
                return 0
            if request == ioctl.USBDEVFS_BULK:
                bulk = arg_ptr._obj
                if bulk.ep & 0x80:
                    # In endpoint: corrupt data
                    in_buf_ptr = ctypes.cast(
                        bulk.data, ctypes.POINTER(ctypes.c_char * bulk.len)
                    )
                    in_buf_ptr.contents.raw = b"\x00" * bulk.len
                return bulk.len
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            params = TestParams(iterations=1, length=512)
            res = backend.run_bulk_loopback(
                params, length_vary=False, test_id=9
            )
            self.assertEqual(res.status, TestStatus.FAIL)
            self.assertIsNotNone(res.error_message)
            self.assertIn("Loopback data corruption", res.error_message or "")

    def test_ioctl_run_bulk_loopback_zlp(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True
        backend.speed_name = "high"
        backend.current_config = 2

        mock_cdll = MagicMock()

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_CONTROL:
                ctrl = arg_ptr._obj
                if ctrl.bRequestType & 0x80 and ctrl.bRequest == 0x06:
                    desc_type = (ctrl.wValue >> 8) & 0xFF
                    if desc_type == 0x02:
                        to_copy = min(ctrl.wLength, len(MOCK_CONFIG2_DESC))
                        buf = ctypes.cast(
                            ctrl.data, ctypes.POINTER(ctypes.c_char * to_copy)
                        )
                        buf.contents.raw = MOCK_CONFIG2_DESC[:to_copy]
                        return to_copy
                return 0
            if request == ioctl.USBDEVFS_BULK:
                bulk = arg_ptr._obj
                if bulk.ep & 0x80:
                    if bulk.len > 0:
                        in_buf_ptr = ctypes.cast(
                            bulk.data, ctypes.POINTER(ctypes.c_char * bulk.len)
                        )
                        in_buf_ptr.contents.raw = bytes(
                            (j + 0) % 256 for j in range(bulk.len)
                        )
                return bulk.len
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            params = TestParams(iterations=1, length=512)
            res = backend.run_bulk_loopback(
                params,
                is_zlp=True,
                test_id=35,
                test_name="Test 35: LOOPBACK_BULK_ZLP",
            )
            self.assertEqual(res.status, TestStatus.PASS)
            self.assertEqual(res.test_id, 35)

    def test_dynamic_endpoint_discovery(self) -> None:
        """Verify that _find_endpoints parses non-standard endpoints."""
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True

        # fmt: off
        custom_desc = bytes([
            # Config descriptor (9 bytes)
            0x09, 0x02, 0x20, 0x00, 0x01, 0x02, 0x00, 0xC0, 0xFA,
            # Interface descriptor (9 bytes)
            0x09, 0x04, 0x01, 0x00, 0x02, 0xFF, 0x00, 0x00, 0x00,
            # Endpoint OUT (7 bytes): 0x04
            0x07, 0x05, 0x04, 0x02, 0x00, 0x02, 0x00,
            # Endpoint IN (7 bytes): 0x85
            0x07, 0x05, 0x85, 0x02, 0x00, 0x02, 0x00,
        ])
        # fmt: on

        mock_cdll = MagicMock()

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_CONTROL:
                ctrl = arg_ptr._obj
                if ctrl.bRequestType & 0x80 and ctrl.bRequest == 0x06:
                    desc_type = (ctrl.wValue >> 8) & 0xFF
                    if desc_type == 0x02:
                        to_copy = min(ctrl.wLength, len(custom_desc))
                        buf = ctypes.cast(
                            ctrl.data, ctypes.POINTER(ctypes.c_char * to_copy)
                        )
                        buf.contents.raw = custom_desc[:to_copy]
                        return to_copy
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            ep_out, ep_in, ifnum = backend._find_endpoints(config_val=2)
            self.assertEqual(ep_out, 0x04)
            self.assertEqual(ep_in, 0x85)
            self.assertEqual(ifnum, 1)

    def test_ioctl_reconnect_drivers_on_close(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True

        mock_cdll = MagicMock()
        connected_interfaces = []

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_IOCTL:
                wrapper = (
                    arg_ptr.contents
                    if hasattr(arg_ptr, "contents")
                    else arg_ptr._obj
                )
                if wrapper.ioctl_code == ioctl.USBDEVFS_CONNECT:
                    connected_interfaces.append(wrapper.ifno)
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            with patch("os.close"):
                backend.close()

        self.assertFalse(backend.is_open)
        self.assertIsNone(backend._fd)
        self.assertEqual(connected_interfaces, list(range(8)))

    def test_ioctl_get_num_configurations_ep0(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True

        mock_cdll = MagicMock()

        def mock_ioctl(fd: int, req: int, arg: Any) -> int:
            if req == ioctl.USBDEVFS_CONTROL:
                ctrl = ctypes.cast(
                    arg, ctypes.POINTER(ioctl.UsbdevfsCtrltransfer)
                ).contents
                if (
                    ctrl.bRequest == 0x06 and ctrl.wValue == 0x0100
                ):  # GET_DESCRIPTOR Device
                    buf = (ctypes.c_char * 18).from_address(ctrl.data)
                    # Set bNumConfigurations at index 17
                    buf[17] = b"\x02"
                    return 18
            return 0

        mock_cdll.ioctl.side_effect = mock_ioctl
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            self.assertEqual(backend.get_num_configurations(), 2)

    def test_ioctl_get_num_configurations_sysfs(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True

        # EP0 fails, fallback to sysfs
        mock_cdll = MagicMock()
        mock_cdll.ioctl.side_effect = OSError("Control transfer failed")

        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            with patch.object(
                backend,
                "_get_sysfs_device_path",
                return_value="/sys/bus/usb/devices/1-2",
            ):
                with patch("os.path.exists", return_value=True):
                    with patch(
                        "builtins.open",
                        unittest.mock.mock_open(read_data="3\n"),
                    ):
                        self.assertEqual(backend.get_num_configurations(), 3)

    def test_parse_endpoints_from_sysfs_isolated_interfaces(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")

        # Interface 0 has only IN (0x81), Interface 1 has only OUT (0x02)
        # Verify it does NOT mix them into a single (0x02, 0x81, 1) pair!
        def mock_listdir(path: str) -> list[str]:
            if path == "/sys/bus/usb/devices/1-2":
                return ["1-2:2.0", "1-2:2.1"]
            elif path.endswith("1-2:2.0"):
                return ["ep_81"]
            elif path.endswith("1-2:2.1"):
                return ["ep_02"]
            return []

        def mock_open_impl(path: str, *args: Any, **kwargs: Any) -> Any:
            if "ep_81" in path and "type" in path:
                return unittest.mock.mock_open(read_data="Bulk\n")()
            elif "ep_81" in path and "bEndpointAddress" in path:
                return unittest.mock.mock_open(read_data="81\n")()
            elif "ep_02" in path and "type" in path:
                return unittest.mock.mock_open(read_data="Bulk\n")()
            elif "ep_02" in path and "bEndpointAddress" in path:
                return unittest.mock.mock_open(read_data="02\n")()
            return unittest.mock.mock_open(read_data="")()

        with patch("os.listdir", side_effect=mock_listdir):
            with patch("os.path.isdir", return_value=True):
                with patch("os.path.exists", return_value=True):
                    with patch("builtins.open", side_effect=mock_open_impl):
                        (
                            ep_out,
                            ep_in,
                            ifnum,
                        ) = backend._parse_endpoints_from_sysfs(
                            "/sys/bus/usb/devices/1-2", 2
                        )
                        # Neither interface has BOTH in and out, so
                        # fallback returns one interface's ep
                        self.assertTrue(ep_out is None or ep_in is None)

    def test_ioctl_run_test_enotty_fails(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True
        backend.current_config = 1

        mock_cdll = MagicMock()
        mock_cdll.ioctl.return_value = -1

        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            with patch("ctypes.get_errno", return_value=errno.ENOTTY):
                params = TestParams(iterations=1, length=512)
                res = backend.run_test(1, params)
                self.assertEqual(res.status, TestStatus.FAIL)
                self.assertIn(
                    "usbtest driver is not bound", res.error_message or ""
                )

    def test_ioctl_run_test_eopnotsupp_skips(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True
        backend.current_config = 1

        mock_cdll = MagicMock()
        mock_cdll.ioctl.return_value = -1

        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            with patch("ctypes.get_errno", return_value=errno.EOPNOTSUPP):
                params = TestParams(iterations=1, length=512)
                res = backend.run_test(1, params)
                self.assertEqual(res.status, TestStatus.SKIP)
                self.assertIn(
                    "Operation not supported", res.error_message or ""
                )

    def test_ioctl_short_read_detection(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True
        backend.speed_name = "high"
        backend.current_config = 2

        mock_cdll = MagicMock()

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_CONTROL:
                ctrl = arg_ptr._obj
                if ctrl.bRequestType & 0x80 and ctrl.bRequest == 0x06:
                    desc_type = (ctrl.wValue >> 8) & 0xFF
                    if desc_type == 0x02:
                        to_copy = min(ctrl.wLength, len(MOCK_CONFIG2_DESC))
                        buf = ctypes.cast(
                            ctrl.data, ctypes.POINTER(ctypes.c_char * to_copy)
                        )
                        buf.contents.raw = MOCK_CONFIG2_DESC[:to_copy]
                        return to_copy
                return 0
            if request == ioctl.USBDEVFS_BULK:
                bulk = arg_ptr._obj
                if bulk.ep & 0x80:
                    # Return short read: 256 instead of requested 512
                    return 256
                return bulk.len
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            params = TestParams(iterations=1, length=512)
            res = backend.run_bulk_loopback(
                params, length_vary=False, test_id=9
            )
            self.assertEqual(res.status, TestStatus.FAIL)
            self.assertIn("Loopback short read", res.error_message or "")

    def test_ioctl_set_configuration_1_failure(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        backend._fd = 123
        backend.is_open = True
        backend.current_config = 2

        with patch.object(backend, "get_configuration", return_value=2):
            with patch.object(backend, "set_configuration", return_value=False):
                params = TestParams(iterations=1, length=512)
                res = backend.run_test(1, params)
                self.assertEqual(res.status, TestStatus.FAIL)
                self.assertIn(
                    "Failed to set Configuration 1 for test",
                    res.error_message or "",
                )

    def test_ioctl_non_bus_path_sysfs(self) -> None:
        backend = ioctl.USBTestIoctlBackend("/dev/usbtest")
        self.assertIsNone(backend._get_sysfs_device_path())


class TestCLI(unittest.TestCase):
    """Tests for testusb CLI argument parsing, flags, and error codes."""

    @patch.object(cli, "_create_backend")
    def test_cli_basic_run(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"):
            exit_code = main(["-t", "0,1", "-c", "100", "-b", "2048"])

        self.assertEqual(exit_code, 0)
        self.assertEqual(mock_backend.executed_tests, [0, 1])

    @patch.object(cli, "_create_backend")
    def test_cli_json_output(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        mock_backend_cls.return_value = mock_backend

        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = os.path.join(tmpdir, "report.json")
            with patch("sys.stdout"):
                exit_code = main(["-t", "0", "--json-file", tmp_path])
            self.assertEqual(exit_code, 0)

            with open(tmp_path, "r", encoding="utf-8") as f:
                data = json.load(f)
            self.assertEqual(data["summary"]["passed"], 1)
            self.assertEqual(data["results"][0]["test_id"], 0)

    def test_cli_list_tests(self) -> None:
        with patch("sys.stdout") as mock_stdout:
            exit_code = main(["--list-tests"])
            self.assertEqual(exit_code, 0)
            mock_stdout.write.assert_called()

    @patch.object(cli, "_create_backend")
    def test_cli_loopback_flag(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"):
            exit_code = main(["--loopback"])

        self.assertEqual(exit_code, 0)
        self.assertEqual(mock_backend.configs_set, [2])
        self.assertEqual(
            mock_backend.executed_tests,
            [0, 9, 10, 14, 21, 32, 33, 34, 35],
        )

    @patch.object(cli, "_create_backend")
    def test_cli_mode_both(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"):
            exit_code = main(["-m", "both", "-t", "0,1,9"])

        self.assertEqual(exit_code, 0)
        self.assertEqual(mock_backend.configs_set, [1, 2])
        self.assertIn(0, mock_backend.executed_tests)
        self.assertIn(1, mock_backend.executed_tests)
        self.assertIn(9, mock_backend.executed_tests)

    @patch.object(cli, "_create_backend")
    def test_cli_failure_exit_code(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        mock_backend.fail_tests = [1]
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"):
            exit_code = main(["-t", "1"])
        self.assertEqual(exit_code, 1)

    @patch.object(cli, "_create_backend")
    def test_cli_all_skipped_exit_code(
        self, mock_backend_cls: MagicMock
    ) -> None:
        mock_backend = MockUSBTestBackend()

        def run_skip(test_id: int, params: TestParams) -> TestResult:
            return TestResult(
                test_id=test_id,
                test_name=f"Test {test_id}",
                status=TestStatus.SKIP,
                duration_secs=0.01,
                error_message="Skipped test",
            )

        mock_backend.custom_run_test = run_skip
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"):
            exit_code = main(["-t", "1"])
        self.assertEqual(exit_code, 1)

    def test_cli_invalid_args(self) -> None:
        with patch("sys.stderr"):
            exit_code = main(["-t", "invalid"])
        self.assertEqual(exit_code, 1)

    @patch.object(cli, "_create_backend")
    def test_cli_json_stdout(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        mock_backend_cls.return_value = mock_backend

        f = io.StringIO()
        with patch("sys.stdout", f):
            exit_code = main(["-t", "0", "--json"])
        self.assertEqual(exit_code, 0)
        data = json.loads(f.getvalue())
        self.assertEqual(data["summary"]["passed"], 1)

    @patch.object(
        backend.USBTestBackend,
        "discover_devices",
        return_value=["/dev/bus/usb/001/002"],
    )
    def test_cli_list_devices(self, mock_discover: MagicMock) -> None:
        with patch("sys.stdout") as mock_stdout:
            exit_code = main(["--list-devices"])
        self.assertEqual(exit_code, 0)
        mock_stdout.write.assert_called()

    @patch.object(cli, "_create_backend")
    def test_cli_keyboard_interrupt(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()

        def raise_kb(test_id: int, params: TestParams) -> TestResult:
            raise KeyboardInterrupt()

        mock_backend.custom_run_test = raise_kb
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"), patch("sys.stderr"):
            exit_code = main(["-t", "0"])
        self.assertEqual(exit_code, 130)

    @patch.object(cli, "_create_backend")
    def test_cli_all_preserves_tests(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"):
            exit_code = main(["-a", "-t", "1,2"])
        self.assertEqual(exit_code, 0)
        self.assertEqual(mock_backend.executed_tests, [1, 2])

    @patch.object(cli, "_create_backend")
    def test_cli_timeout_flag(self, mock_backend_cls: MagicMock) -> None:
        mock_backend = MockUSBTestBackend()
        received_params = []

        def record_params(test_id: int, params: TestParams) -> TestResult:
            received_params.append(params)
            return TestResult(
                test_id=test_id,
                test_name="T",
                status=TestStatus.PASS,
                duration_secs=0.01,
            )

        mock_backend.custom_run_test = record_params
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"):
            exit_code = main(["-t", "0", "--timeout", "12345"])
        self.assertEqual(exit_code, 0)
        self.assertEqual(len(received_params), 1)
        self.assertEqual(received_params[0].timeout_ms, 12345)

    @patch.object(cli, "_create_backend")
    def test_cli_keyboard_interrupt_after_pass_returns_130(
        self, mock_backend_cls: MagicMock
    ) -> None:
        mock_backend = MockUSBTestBackend()
        call_count = 0

        def interrupt_after_first(
            test_id: int, params: TestParams
        ) -> TestResult:
            nonlocal call_count
            call_count += 1
            if call_count > 1:
                raise KeyboardInterrupt()
            return TestResult(
                test_id=test_id,
                test_name="Test",
                status=TestStatus.PASS,
                duration_secs=0.01,
            )

        mock_backend.custom_run_test = interrupt_after_first
        mock_backend_cls.return_value = mock_backend

        with patch("sys.stdout"), patch("sys.stderr"):
            exit_code = main(["-t", "0,1"])
        # Must return 130 (POSIX SIGINT) even though test 0 passed!
        self.assertEqual(exit_code, 130)

    def test_cli_invalid_param_values(self) -> None:
        with patch("sys.stderr"):
            exit_code = main(["-c", "0"])
        self.assertEqual(exit_code, 1)


class TestUSBTestBackend(unittest.TestCase):
    """Tests for USBTestBackend base class and device discovery."""

    def test_abstract_class_cannot_instantiate(self) -> None:
        with self.assertRaises(TypeError):
            backend.USBTestBackend()  # type: ignore[abstract]

    def test_claimed_interface_is_abstract(self) -> None:
        class IncompleteBackend(backend.USBTestBackend):
            def open(self) -> None:
                pass

            def close(self) -> None:
                pass

            def get_speed(self) -> str:
                return "high"

            def set_configuration(self, config_val: int) -> bool:
                return True

            def run_test(self, test_id: int, params: TestParams) -> TestResult:
                return TestResult(
                    test_id=test_id,
                    test_name="min",
                    status=TestStatus.PASS,
                    duration_secs=0.001,
                )

        # Incomplete backend omitting claimed_interface cannot be instantiated
        with self.assertRaises(TypeError):
            IncompleteBackend()  # type: ignore[abstract]

        # Implementing claimed_interface allows clean instantiation and usage
        class CompleteBackend(IncompleteBackend):
            @contextlib.contextmanager
            def claimed_interface(
                self, ifnum: int
            ) -> collections.abc.Iterator[None]:
                yield

        backend_inst = CompleteBackend()
        with backend_inst.claimed_interface(0):
            pass


class TestParseLoopCount(unittest.TestCase):
    """Tests for _parse_loop_count CLI argument parsing."""

    def test_parse_loop_count_valid_integers(self) -> None:
        self.assertEqual(cli._parse_loop_count("1"), 1)
        self.assertEqual(cli._parse_loop_count("100"), 100)
        self.assertEqual(cli._parse_loop_count("0"), 0)

    def test_forever_and_infinite(self) -> None:
        self.assertEqual(cli._parse_loop_count("forever"), 0)
        self.assertEqual(cli._parse_loop_count("FOREVER"), 0)
        self.assertEqual(cli._parse_loop_count("infinite"), 0)
        self.assertEqual(cli._parse_loop_count("Infinite"), 0)

    def test_invalid_values(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError):
            cli._parse_loop_count("-1")
        with self.assertRaises(argparse.ArgumentTypeError):
            cli._parse_loop_count("invalid")


class TestMaxPacketExtraction(unittest.TestCase):
    """Tests for wMaxPacketSize extraction from descriptors and sysfs."""

    def test_find_endpoints_extracts_maxpacket(self) -> None:
        dev = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        dev._fd = 123

        mock_cdll = MagicMock()

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_CONTROL:
                ctrl = arg_ptr._obj
                if ctrl.bRequestType & 0x80 and ctrl.bRequest == 0x06:
                    desc_type = (ctrl.wValue >> 8) & 0xFF
                    if desc_type == 0x02:
                        to_copy = min(ctrl.wLength, len(MOCK_CONFIG2_DESC))
                        buf = ctypes.cast(
                            ctrl.data, ctypes.POINTER(ctypes.c_char * to_copy)
                        )
                        buf.contents.raw = MOCK_CONFIG2_DESC[:to_copy]
                        return to_copy
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
            dev._find_endpoints(config_val=2)
            self.assertEqual(dev._maxpacket, 512)

    def test_sysfs_extracts_maxpacket(self) -> None:
        dev = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create sysfs structure: 1-1:2.0/ep_01, 1-1:2.0/ep_81
            intf_dir = os.path.join(tmpdir, "1-1:2.0")
            ep_out = os.path.join(intf_dir, "ep_01")
            ep_in = os.path.join(intf_dir, "ep_81")
            os.makedirs(ep_out)
            os.makedirs(ep_in)

            with open(os.path.join(ep_out, "type"), "w", encoding="utf-8") as f:
                f.write("bulk\n")
            with open(
                os.path.join(ep_out, "bEndpointAddress"), "w", encoding="utf-8"
            ) as f:
                f.write("01\n")
            with open(
                os.path.join(ep_out, "wMaxPacketSize"), "w", encoding="utf-8"
            ) as f:
                f.write("0400\n")  # 1024 bytes (SuperSpeed)

            with open(os.path.join(ep_in, "type"), "w", encoding="utf-8") as f:
                f.write("bulk\n")
            with open(
                os.path.join(ep_in, "bEndpointAddress"), "w", encoding="utf-8"
            ) as f:
                f.write("81\n")
            with open(
                os.path.join(ep_in, "wMaxPacketSize"), "w", encoding="utf-8"
            ) as f:
                f.write("0400\n")

            out_ep, in_ep, ifnum = dev._parse_endpoints_from_sysfs(
                tmpdir, curr_cfg=2
            )
            self.assertEqual(out_ep, 1)
            self.assertEqual(in_ep, 0x81)
            self.assertEqual(ifnum, 0)
            self.assertEqual(dev._maxpacket, 1024)

    def test_run_bulk_loopback_honors_discovered_maxpacket(self) -> None:
        dev = ioctl.USBTestIoctlBackend("/dev/bus/usb/001/002")
        dev._fd = 123
        dev.is_open = True
        dev.speed_name = "unknown"
        dev._maxpacket = 512
        dev.current_config = 2

        mock_cdll = MagicMock()

        def ioctl_side_effect(fd: int, request: int, arg_ptr: Any) -> int:
            if request == ioctl.USBDEVFS_CONTROL:
                return 0
            if request == ioctl.USBDEVFS_BULK:
                bulk = arg_ptr._obj
                if bulk.ep & 0x80:
                    in_buf_ptr = ctypes.cast(
                        bulk.data, ctypes.POINTER(ctypes.c_char * bulk.len)
                    )
                    in_buf_ptr.contents.raw = bytes(
                        (j + 0) % 256 for j in range(bulk.len)
                    )
                return bulk.len
            return 0

        mock_cdll.ioctl.side_effect = ioctl_side_effect
        with patch.object(dev, "_find_endpoints", return_value=(1, 0x81, 0)):
            with patch.object(ioctl, "_get_libc", return_value=mock_cdll):
                params = TestParams(iterations=1, length=512)
                res = dev.run_bulk_loopback(
                    params, length_vary=False, test_id=9
                )
                self.assertEqual(res.status, TestStatus.PASS)


if __name__ == "__main__":
    unittest.main()
