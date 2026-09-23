# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Host unit tests for the zero_function test harness library."""

import unittest
from typing import Any
from unittest import mock

import fuchsia_base_test
from honeydew.transports.ffx import errors as ffx_errors
from mobly import records
from testusb import TestResult, TestStatus
from zero_function import (
    ALL_SUPPORTED_TEST_IDS,
    KNOWN_TEST_DEVICES,
    USB_ZERO_FUNCTION_DRIVER_URL,
    USB_ZERO_PID,
    USB_ZERO_VID,
    ZeroFunctionBaseTest,
)
from zero_function import zero_function as zf_mod
from zero_function.usbtest_controller import UsbTestController


class ZeroFunctionConstantsTest(unittest.TestCase):
    """Verifies Zero Function identifiers and test catalog constants."""

    def test_vendor_and_product_ids(self) -> None:
        self.assertEqual(USB_ZERO_VID, 0x18D1)
        self.assertEqual(USB_ZERO_PID, 0xA022)
        self.assertIn((USB_ZERO_VID, USB_ZERO_PID), KNOWN_TEST_DEVICES)

    def test_supported_test_ids(self) -> None:
        self.assertEqual(len(ALL_SUPPORTED_TEST_IDS), 27)
        # Excludes isochronous test IDs
        for iso_id in (15, 16, 22, 23, 26):
            self.assertNotIn(iso_id, ALL_SUPPORTED_TEST_IDS)
        # Includes key control and bulk transfer test IDs
        for tid in (0, 1, 2, 3, 5, 7, 9, 10, 14, 19, 21, 27, 31):
            self.assertIn(tid, ALL_SUPPORTED_TEST_IDS)


class UsbTestControllerTest(unittest.TestCase):
    """Tests host usbtest driver controller behavior and execution modes."""

    def test_initialization_defaults(self) -> None:
        controller = UsbTestController()
        self.assertEqual(controller.vendor, 0x18D1)
        self.assertEqual(controller.product, 0xA022)
        self.assertEqual(controller.alt, 0)

    @mock.patch("os.path.exists")
    def test_load_driver_already_loaded(
        self, mock_exists: mock.MagicMock
    ) -> None:
        mock_exists.side_effect = lambda p: p == "/sys/module/usbtest"
        controller = UsbTestController()
        # Should return without attempting execution
        controller.load_driver()

    @mock.patch("subprocess.run")
    @mock.patch("os.path.exists")
    def test_load_driver_via_dmc(
        self, mock_exists: mock.MagicMock, mock_run: mock.MagicMock
    ) -> None:
        mock_exists.side_effect = lambda p: p != "/sys/module/usbtest"
        controller = UsbTestController(vendor=0x18D1, product=0xA022, alt=0)
        controller._dmc_path = "/usr/bin/dmc"
        controller.load_driver()
        mock_run.assert_called_once_with(
            [
                "/usr/bin/dmc",
                "load-usbtest-driver",
                "-vendor",
                "0x18d1",
                "-product",
                "0xa022",
                "-alt",
                "0",
            ],
            check=True,
            capture_output=True,
            text=True,
        )

    @mock.patch("os.path.exists", return_value=False)
    def test_load_driver_without_dmc_raises(
        self, mock_exists: mock.MagicMock
    ) -> None:
        controller = UsbTestController()
        controller._dmc_path = None
        with self.assertRaisesRegex(
            RuntimeError, "usbtest kernel module is not loaded on host"
        ):
            controller.load_driver()

    @mock.patch("subprocess.run")
    def test_unload_driver_via_dmc(self, mock_run: mock.MagicMock) -> None:
        controller = UsbTestController()
        controller._dmc_path = "/usr/bin/dmc"
        with mock.patch("os.path.exists", return_value=True):
            controller.unload_driver()
        mock_run.assert_called_once_with(
            ["/usr/bin/dmc", "unload-usbtest-driver"],
            check=False,
            capture_output=True,
            text=True,
        )


class ZeroFunctionBaseTestLifecycleTest(unittest.IsolatedAsyncioTestCase):
    """Tests ZeroFunctionBaseTest lifecycle and execution."""

    def setUp(self) -> None:
        super().setUp()
        self._patchers = [
            mock.patch.object(
                fuchsia_base_test.FuchsiaBaseTest,
                "setup_class",
                new_callable=mock.AsyncMock,
            ),
            mock.patch.object(
                fuchsia_base_test.FuchsiaBaseTest,
                "teardown_class",
                new_callable=mock.AsyncMock,
            ),
            mock.patch.object(
                zf_mod, "get_dut_serial", return_value="test_serial"
            ),
            mock.patch.object(
                zf_mod, "get_usb_config", return_value="sourcesink"
            ),
            mock.patch.object(
                zf_mod,
                "parse_usb_config_functions",
                return_value=["sourcesink"],
            ),
            mock.patch.object(zf_mod, "set_usb_config"),
            mock.patch.object(
                zf_mod,
                "find_usb_device_node",
                return_value="/dev/bus/usb/001/002",
            ),
            mock.patch.object(
                zf_mod,
                "wait_for_zero_function",
                return_value="/dev/bus/usb/001/002",
            ),
        ]
        for patcher in self._patchers:
            patcher.start()

    def tearDown(self) -> None:
        for patcher in reversed(self._patchers):
            patcher.stop()
        super().tearDown()

    def _create_test_instance(
        self, user_params: dict[str, Any] | None = None
    ) -> ZeroFunctionBaseTest:
        configs = mock.MagicMock()
        test_instance = ZeroFunctionBaseTest(configs)
        test_instance.user_params = user_params or {}
        test_instance.log_path = "/tmp"
        test_instance.dut = mock.MagicMock()
        test_instance.current_test_info = mock.MagicMock()
        test_instance.current_test_info.name = "test_mock"
        return test_instance

    async def test_teardown_class_without_setup_does_not_crash(self) -> None:
        """Verifies teardown_class handles uninitialized attributes."""
        test_instance = self._create_test_instance()
        with mock.patch.object(
            test_instance, "teardown_class", wraps=test_instance.teardown_class
        ):
            await test_instance.teardown_class()

    async def test_on_fail_delegates_to_super(self) -> None:
        """Verifies on_fail sets failure flag and delegates to super."""
        test_instance = self._create_test_instance()
        record = mock.MagicMock(spec=records.TestResultRecord)
        with mock.patch.object(
            fuchsia_base_test.FuchsiaBaseTest,
            "on_fail",
            new_callable=mock.AsyncMock,
        ) as mock_super_on_fail:
            await test_instance.on_fail(record)
            self.assertTrue(test_instance._any_test_failed)
            mock_super_on_fail.assert_awaited_once_with(record)

    @mock.patch.object(zf_mod, "TestRunner")
    @mock.patch.object(zf_mod, "USBTestIoctlBackend")
    def test_execute_testusb(
        self,
        mock_backend_cls: mock.MagicMock,
        mock_runner_cls: mock.MagicMock,
    ) -> None:
        """Verifies execute_testusb instantiates backend and runner.

        Ensures runner.run() is called with test IDs.
        """
        mock_backend = mock.MagicMock()
        mock_backend_cls.return_value.__enter__.return_value = mock_backend

        expected_results = [
            TestResult(
                test_id=0,
                test_name="Control NOP",
                status=TestStatus.PASS,
                duration_secs=0.01,
            )
        ]
        mock_runner = mock.MagicMock()
        mock_runner.run.return_value = expected_results
        mock_runner_cls.return_value = mock_runner

        test_instance = self._create_test_instance()
        results = test_instance.execute_testusb(
            dev_node="/dev/bus/usb/001/002",
            test_ids=[0],
            mode="sourcesink",
            iterations=5,
        )
        self.assertEqual(results, expected_results)
        mock_backend_cls.assert_called_once_with(
            device_path="/dev/bus/usb/001/002"
        )
        mock_runner.run.assert_called_once_with([0])

    @mock.patch.object(zf_mod, "TestRunner")
    @mock.patch.object(zf_mod, "USBTestIoctlBackend")
    def test_execute_testusb_with_existing_backend(
        self,
        mock_backend_cls: mock.MagicMock,
        mock_runner_cls: mock.MagicMock,
    ) -> None:
        """Verifies execute_testusb reuses existing backend if provided."""
        existing_backend = mock.MagicMock()
        mock_runner = mock.MagicMock()
        mock_runner.run.return_value = []
        mock_runner_cls.return_value = mock_runner

        test_instance = self._create_test_instance()
        results = test_instance.execute_testusb(
            dev_node="/dev/bus/usb/001/002",
            test_ids=[0],
            backend=existing_backend,
        )
        self.assertEqual(results, [])
        mock_backend_cls.assert_not_called()
        mock_runner_cls.assert_called_once()
        self.assertEqual(
            mock_runner_cls.call_args[1]["backend"], existing_backend
        )

    def test_assert_testusb_success_all_pass(self) -> None:
        """Verifies assert_testusb_success succeeds when all results passed."""
        test_instance = self._create_test_instance()
        results = [
            TestResult(
                test_id=0,
                test_name="Control NOP",
                status=TestStatus.PASS,
                duration_secs=0.01,
            ),
            TestResult(
                test_id=1,
                test_name="Bulk OUT",
                status=TestStatus.PASS,
                duration_secs=0.02,
            ),
        ]
        test_instance.assert_testusb_success(results)

    def test_assert_testusb_success_all_skipped(self) -> None:
        """Verifies assert_testusb_success skips when all tests are skipped."""
        test_instance = self._create_test_instance()
        results = [
            TestResult(
                test_id=15,
                test_name="ISO OUT",
                status=TestStatus.SKIP,
                duration_secs=0.0,
                error_message="not supported",
            )
        ]
        with mock.patch("mobly.asserts.skip") as mock_skip:
            test_instance.assert_testusb_success(results)
            mock_skip.assert_called_once()

    def test_assert_testusb_success_failure_raises(self) -> None:
        """Verifies assert_testusb_success fails on test execution errors."""
        test_instance = self._create_test_instance()
        results = [
            TestResult(
                test_id=1,
                test_name="Bulk OUT",
                status=TestStatus.FAIL,
                duration_secs=0.05,
                error_message="I/O error",
            )
        ]
        with mock.patch("mobly.asserts.fail") as mock_fail:
            test_instance.assert_testusb_success(results)
            mock_fail.assert_called_once()

    @mock.patch.object(zf_mod, "USBTestIoctlBackend")
    @mock.patch.object(ZeroFunctionBaseTest, "execute_testusb")
    @mock.patch.object(ZeroFunctionBaseTest, "assert_testusb_success")
    def test_execute_testusb_timed(
        self,
        mock_assert: mock.MagicMock,
        mock_exec: mock.MagicMock,
        mock_backend_cls: mock.MagicMock,
    ) -> None:
        """Verifies execute_testusb_timed runs batches until completion."""
        test_instance = self._create_test_instance()
        mock_exec.return_value = []
        test_instance.execute_testusb_timed(
            dev_node="/dev/bus/usb/001/002",
            test_ids=[0],
            mode="sourcesink",
            duration_sec=0.01,
            iterations_per_batch=1,
        )
        self.assertGreaterEqual(mock_exec.call_count, 1)
        self.assertGreaterEqual(mock_assert.call_count, 1)
        mock_backend_cls.assert_called_once_with(
            device_path="/dev/bus/usb/001/002"
        )

    @mock.patch("zero_function.UsbTestController")
    async def test_setup_class_registers_driver_successfully(
        self, _mock_controller: mock.MagicMock
    ) -> None:
        """Verifies setup_class registers the driver if not already present."""
        test_instance = self._create_test_instance()
        dut: Any = test_instance.dut
        dut.ffx.run.side_effect = (
            lambda cmd, **kwargs: ""
            if cmd[:2] == ["driver", "list"]
            else "Registered"
        )
        with mock.patch.object(
            test_instance,
            "configure_zero_function",
            return_value="/dev/bus/usb/001/002",
        ):
            await test_instance.setup_class()

        calls = [call[0][0] for call in dut.ffx.run.call_args_list]
        self.assertIn(["driver", "list"], calls)
        self.assertIn(
            ["driver", "register", USB_ZERO_FUNCTION_DRIVER_URL],
            calls,
        )

    @mock.patch("zero_function.UsbTestController")
    async def test_setup_class_skips_registration_when_already_in_driver_list(
        self, _mock_controller: mock.MagicMock
    ) -> None:
        """Verifies setup_class skips registration if URL is already present."""
        test_instance = self._create_test_instance()
        dut: Any = test_instance.dut
        dut.ffx.run.return_value = f"Registered: {USB_ZERO_FUNCTION_DRIVER_URL}"
        with mock.patch.object(
            test_instance,
            "configure_zero_function",
            return_value="/dev/bus/usb/001/002",
        ):
            await test_instance.setup_class()

        calls = [call[0][0] for call in dut.ffx.run.call_args_list]
        self.assertIn(["driver", "list"], calls)
        register_calls = [c for c in calls if c[:2] == ["driver", "register"]]
        self.assertEqual(register_calls, [])

    @mock.patch("zero_function.UsbTestController")
    async def test_setup_class_handles_already_exists_error(
        self, _mock_controller: mock.MagicMock
    ) -> None:
        """Verifies setup_class restarts driver when registration exists."""
        test_instance = self._create_test_instance()
        dut: Any = test_instance.dut

        def ffx_run(cmd: list[str], **kwargs: Any) -> str:
            if cmd[:2] == ["driver", "list"]:
                return ""
            if cmd[:2] == ["driver", "register"]:
                raise ffx_errors.FfxCommandError(
                    "ALREADY_EXISTS: driver is already registered"
                )
            return "Restarted"

        dut.ffx.run.side_effect = ffx_run
        with mock.patch.object(
            test_instance,
            "configure_zero_function",
            return_value="/dev/bus/usb/001/002",
        ):
            await test_instance.setup_class()

        calls = [call[0][0] for call in dut.ffx.run.call_args_list]
        self.assertIn(
            ["driver", "restart", USB_ZERO_FUNCTION_DRIVER_URL],
            calls,
        )

    @mock.patch("zero_function.UsbTestController")
    async def test_setup_class_raises_on_invalid_driver_url(
        self, _mock_controller: mock.MagicMock
    ) -> None:
        """Verifies setup_class raises ValueError on invalid driver URL."""
        test_instance = self._create_test_instance(
            user_params={"driver_url": "http://invalid-driver-url"}
        )
        with self.assertRaisesRegex(
            ValueError, "must start with 'fuchsia-pkg://'"
        ):
            await test_instance.setup_class()

    @mock.patch("zero_function.UsbTestController")
    async def test_setup_class_propagates_unexpected_registration_error(
        self, _mock_controller: mock.MagicMock
    ) -> None:
        """Verifies setup_class propagates unexpected FfxCommandError."""
        test_instance = self._create_test_instance()
        dut: Any = test_instance.dut

        def ffx_run(cmd: list[str], **kwargs: Any) -> str:
            if cmd[:2] == ["driver", "list"]:
                return ""
            if cmd[:2] == ["driver", "register"]:
                raise ffx_errors.FfxCommandError(
                    "DEVICE_UNREACHABLE: failed to communicate"
                )
            return ""

        dut.ffx.run.side_effect = ffx_run
        with mock.patch.object(
            test_instance,
            "configure_zero_function",
            return_value="/dev/bus/usb/001/002",
        ):
            with self.assertRaisesRegex(
                ffx_errors.FfxCommandError, "DEVICE_UNREACHABLE"
            ):
                await test_instance.setup_class()


if __name__ == "__main__":
    unittest.main()
