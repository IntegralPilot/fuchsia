# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Shared library for USB Zero Function (usb-zero-function) test suites.

Provides Zero-Function-specific constants, device node discovery, and a reusable
`ZeroFunctionBaseTest` Mobly base class utilizing generic `usb_config` and
`sysfs_usb` modules.
"""

import collections.abc
import logging
import os
import time
from typing import Any

import fuchsia_base_test
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx.types import MachineFormat
from mobly import asserts, records
from testusb import (
    ALL_TEST_CASES,
    TestParams,
    TestResult,
    TestRunner,
    TestStatus,
    USBTestBackend,
    USBTestIoctlBackend,
)
from usb_lib.sysfs_usb import find_usb_device_node, wait_for_usb_device
from usb_lib.usb_config import (
    get_dut_serial,
    get_usb_config,
    parse_usb_config_functions,
    set_usb_config,
)
from zero_function.usbtest_controller import UsbTestController

_LOGGER: logging.Logger = logging.getLogger(__name__)

USB_ZERO_VID: int = 0x18D1
USB_ZERO_PID: int = 0xA022
KNOWN_TEST_DEVICES: tuple[tuple[int, int], ...] = (
    (USB_ZERO_VID, USB_ZERO_PID),  # Fuchsia USB Zero Function
)

USB_ZERO_FUNCTION_DRIVER_URL: str = (
    "fuchsia-pkg://fuchsia.com/usb-zero-function#meta/usb-zero-function.cm"
)

# 27 supported test IDs. Isochronous tests (15, 16, 22, 23, 26) are excluded
# because zero function only exposes bulk/control endpoints. Loopback
# tests (32-35) are tested separately.
ALL_SUPPORTED_TEST_IDS: tuple[int, ...] = (
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    9,
    10,
    11,
    12,
    13,
    14,
    17,
    18,
    19,
    20,
    21,
    24,
    25,
    27,
    28,
    29,
    30,
    31,
)


def find_zero_function_device_node(
    dut: Any | None = None,
    target_serial: str | None = None,
) -> str | None:
    """Scan Linux sysfs to locate the active Zero Function node.

    Locates the active /dev/bus/usb/BBB/DDD node.

    Args:
        dut: Optional Honeydew FuchsiaDevice object (serial is extracted
            automatically if provided).
        target_serial: Optional serial number string override.

    Returns:
        Device path if found, or None.
    """
    return find_usb_device_node(
        dut=dut,
        target_serial=target_serial,
        known_devices=KNOWN_TEST_DEVICES,
    )


def wait_for_zero_function(
    dut: Any | None = None,
    target_serial: str | None = None,
    timeout_sec: float = 30.0,
    poll_interval_sec: float = 0.5,
) -> str:
    """Poll until the Zero Function device node is enumerated and accessible.

    Args:
        dut: Optional Honeydew FuchsiaDevice object (serial is extracted
            automatically if provided).
        target_serial: Optional serial number string override.
        timeout_sec: Maximum wait time in seconds.
        poll_interval_sec: Polling interval in seconds.

    Returns:
        The /dev/bus/usb/BBB/DDD device node path.
    """
    return wait_for_usb_device(
        dut=dut,
        target_serial=target_serial,
        known_devices=KNOWN_TEST_DEVICES,
        timeout_sec=timeout_sec,
        poll_interval_sec=poll_interval_sec,
    )


class ZeroFunctionBaseTest(fuchsia_base_test.FuchsiaBaseTest):
    """Base Mobly test class for USB Zero Function test suites."""

    def __init__(
        self,
        mobly_configs: Any,
        *args: Any,
        **kwargs: Any,
    ) -> None:
        super().__init__(mobly_configs, *args, **kwargs)
        self._any_test_failed: bool = False
        self.dev_node: str | None = None
        self.target_serial: str | None = None
        self._initial_config_raw: str = ""
        self._initial_functions: list[str] = []
        self.driver_controller: UsbTestController | None = None
        self.test_case_path: str = ""

    async def setup_class(self) -> None:
        """Record initial USB configuration and initialize test harness."""
        await super().setup_class()

        self.snapshot_on = fuchsia_base_test.SnapshotOn.NEVER
        self.tracing_on = fuchsia_base_test.TracingOn.NEVER

        # Warm up cached persistent properties needed by Mobly test metadata
        for prop in ("board", "product", "device_name"):
            try:
                _ = getattr(self.dut, prop, None)
            except Exception:
                pass

        self.target_serial = get_dut_serial(self.dut)

        self.driver_controller = UsbTestController(
            vendor=USB_ZERO_VID, product=USB_ZERO_PID, alt=0
        )
        try:
            self.driver_controller.load_driver()
        except Exception as e:
            _LOGGER.warning("Could not load usbtest driver: %s", e)

        self._initial_config_raw = get_usb_config(self.dut)
        self._initial_functions = parse_usb_config_functions(
            self._initial_config_raw
        )
        _LOGGER.info(
            "Saved initial USB peripheral configuration: %s",
            self._initial_functions,
        )
        # Register the usb-zero-function driver package from the test repository
        # before switching USB peripheral mode to Zero Function.
        raw_driver_url = self.user_params.get("driver_url")
        if not raw_driver_url:
            driver_url = USB_ZERO_FUNCTION_DRIVER_URL
        elif isinstance(raw_driver_url, str):
            driver_url = raw_driver_url.strip()
            if not driver_url.startswith("fuchsia-pkg://"):
                raise ValueError(
                    f"Invalid driver_url '{raw_driver_url}': must start with "
                    "'fuchsia-pkg://'"
                )
        else:
            raise ValueError(
                f"Invalid driver_url type '{type(raw_driver_url)}': "
                "expected string"
            )

        # Pre-resolve package blobs into local blobfs while CDC is active,
        # ensuring driver binaries are available after USB peripheral switching
        # drops networking.
        pkg_url = driver_url.split("#")[0]
        ffx_inst = getattr(self.dut, "ffx", None)
        if ffx_inst and hasattr(ffx_inst, "run_ssh_cmd"):
            try:
                _LOGGER.info("Pre-resolving package blobs for %s...", pkg_url)
                ffx_inst.run_ssh_cmd(f"pkgctl resolve {pkg_url}")
            except Exception as resolve_err:
                _LOGGER.warning("pkgctl resolve returned: %s", resolve_err)

        # Query whether driver is already registered before registering.
        is_registered = False
        try:
            driver_list = self.dut.ffx.run(
                ["driver", "list"],
                machine=MachineFormat.RAW,
                timeout=60,
            )
            if driver_url in driver_list:
                _LOGGER.info(
                    "Driver %s is already registered on target.", driver_url
                )
                is_registered = True
        except Exception as list_err:
            _LOGGER.warning("Could not query registered drivers: %s", list_err)

        if not is_registered:
            try:
                _LOGGER.info("Registering ephemeral driver: %s", driver_url)
                res = self.dut.ffx.run(
                    ["driver", "register", driver_url],
                    machine=MachineFormat.RAW,
                    timeout=60,
                )
                _LOGGER.info("Driver registration output: %s", res.strip())
            except ffx_errors.FfxCommandError as e:
                if (
                    "ALREADY_EXISTS" in str(e)
                    or "already exists" in str(e).lower()
                ):
                    _LOGGER.info(
                        "Driver %s already registered; restarting driver "
                        "host: %s",
                        driver_url,
                        e,
                    )
                    try:
                        self.dut.ffx.run(
                            ["driver", "restart", driver_url],
                            machine=MachineFormat.RAW,
                            timeout=60,
                        )
                    except Exception as restart_err:
                        _LOGGER.warning(
                            "Driver restart failed (may not be bound yet): %s",
                            restart_err,
                        )
                else:
                    _LOGGER.error(
                        "Failed to register driver %s: %s", driver_url, e
                    )
                    raise

        self.dev_node = self.configure_zero_function("sourcesink;loopback")
        _LOGGER.info("Configured Zero Function device node: %s", self.dev_node)

    async def setup_test(self) -> None:
        """Override to avoid FuchsiaBaseTest logging in Zero Function mode."""
        self._devices_not_healthy = False
        self.test_case_path = os.path.join(
            self.log_path, self.current_test_info.name
        )
        os.makedirs(self.test_case_path, exist_ok=True)
        _LOGGER.info("Starting test case '%s'", self.current_test_info.name)

    async def teardown_test(self) -> None:
        """Override to avoid FuchsiaBaseTest health checks in test mode."""
        _LOGGER.info("Finished test case '%s'", self.current_test_info.name)
        if hasattr(self, "test_case_path") and os.path.exists(
            self.test_case_path
        ):
            try:
                if not os.listdir(self.test_case_path):
                    os.rmdir(self.test_case_path)
            except OSError:
                pass

    async def on_fail(self, record: records.TestResultRecord) -> None:
        """Override to record failure while preserving error propagation."""
        self._any_test_failed = True
        _LOGGER.warning("Test case failed: %s", record)
        await super().on_fail(record)

    async def _collect_snapshot(self, directory: str) -> None:
        """Bypass network snapshot collection in USB peripheral test mode."""
        _LOGGER.info("Bypassing snapshot collection in Zero Function mode.")

    async def teardown_class(self) -> None:
        """Restore initial USB configuration and clean up test resources."""
        try:
            # Restoring initial functions unbinds usb-zero-function. Ephemerally
            # registered drivers remain cached until reboot and do not persist.
            if self._initial_functions:
                restore_str = ",".join(self._initial_functions)
                _LOGGER.info(
                    "Restoring initial USB peripheral configuration: %s",
                    restore_str,
                )
                try:
                    set_usb_config(
                        self.dut, restore_str, reboot_if_needed=False
                    )
                except Exception as e:
                    _LOGGER.warning(
                        "Failed to restore initial USB configuration: %s",
                        e,
                    )
            if self.driver_controller:
                try:
                    self.driver_controller.unload_driver()
                except Exception as e:
                    _LOGGER.warning("Failed unloading usbtest driver: %s", e)
            _LOGGER.info("Teardown class completed.")
        finally:
            await super().teardown_class()

    def configure_zero_function(self, config_name: str = "sourcesink") -> str:
        """Switch device to Zero Function mode and wait for host enumeration."""
        old_node = self.dev_node
        set_usb_config(self.dut, config_name)
        # If an old device node was already present, wait for it to disconnect
        # before polling.
        if old_node and os.path.exists(old_node):
            disconnect_deadline = time.monotonic() + 5.0
            while time.monotonic() < disconnect_deadline:
                if not os.path.exists(old_node):
                    break
                time.sleep(0.1)
        dev_node = wait_for_zero_function(
            dut=self.dut, target_serial=self.target_serial
        )
        self.dev_node = dev_node
        return dev_node

    def require_dev_node(self) -> str:
        """Return active device node path, asserting it is not None."""
        asserts.assert_is_not_none(
            self.dev_node,
            "Expected active USB Zero Function device node in sysfs/devfs",
        )
        assert self.dev_node is not None
        return self.dev_node

    def execute_testusb(
        self,
        dev_node: str,
        test_ids: collections.abc.Sequence[int] | None = None,
        mode: str = "both",
        iterations: int = 10,
        length: int = 1024,
        vary: int = 1024,
        sglen: int = 32,
        timeout_ms: int = 5000,
        quiet: bool = False,
        backend: USBTestBackend | None = None,
    ) -> list[TestResult]:
        """Run the testusb runner against the specified device node.

        Args:
            dev_node: Character device node (/dev/bus/usb/BBB/DDD).
            test_ids: Optional sequence of test IDs to execute.
            mode: Operating mode ('sourcesink', 'loopback', 'both', or 'auto').
            iterations: Number of transfer iterations per test.
            length: Buffer length in bytes.
            vary: Transfer size variation in bytes.
            sglen: Scatter-gather entries count.
            timeout_ms: I/O completion timeout in milliseconds.
            quiet: If True, suppresses runner stdout progress spam.
            backend: Optional existing USBTestBackend instance to reuse.

        Returns:
            List of TestResult instances representing executed test outcomes.
        """
        _LOGGER.info(
            "Running testusb on %s: test_ids=%s, mode='%s', iterations=%d",
            dev_node,
            test_ids,
            mode,
            iterations,
        )
        params = TestParams(
            iterations=iterations,
            length=length,
            vary=vary,
            sglen=sglen,
            timeout_ms=timeout_ms,
        )
        target_ids = (
            list(test_ids)
            if test_ids is not None
            else list(ALL_TEST_CASES.keys())
        )

        if backend is not None:
            runner = TestRunner(
                backend=backend,
                params=params,
                mode=mode,
                quiet=quiet,
            )
            results = runner.run(target_ids)
        else:
            with USBTestIoctlBackend(device_path=dev_node) as created_backend:
                runner = TestRunner(
                    backend=created_backend,
                    params=params,
                    mode=mode,
                    quiet=quiet,
                )
                results = runner.run(target_ids)

        for r in results:
            if r.error_message:
                _LOGGER.info(
                    "Test %d (%s): %s in %.3fs - %s",
                    r.test_id,
                    r.test_name,
                    r.status.value,
                    r.duration_secs,
                    r.error_message,
                )
            else:
                _LOGGER.debug(
                    "Test %d (%s): %s in %.3fs",
                    r.test_id,
                    r.test_name,
                    r.status.value,
                    r.duration_secs,
                )
        return results

    def execute_testusb_timed(
        self,
        dev_node: str,
        test_ids: collections.abc.Sequence[int] | None = None,
        mode: str = "both",
        duration_sec: float = 120.0,
        iterations_per_batch: int = 10,
        length: int = 1024,
        vary: int = 1024,
        sglen: int = 32,
        timeout_ms: int = 5000,
    ) -> None:
        """Run testusb repeatedly in batches until duration_sec has elapsed.

        Args:
            dev_node: Character device node (/dev/bus/usb/BBB/DDD).
            test_ids: Optional sequence of test numbers.
            mode: Mode ('sourcesink', 'loopback', 'both', or 'auto').
            duration_sec: Duration to sustain the stress loop in seconds.
            iterations_per_batch: Number of iterations per testusb invocation.
            length: Buffer length in bytes.
            vary: Transfer size variation in bytes.
            sglen: Scatter-gather entries count.
            timeout_ms: Transfer timeout in milliseconds.
        """
        start_time = time.monotonic()
        batch = 0
        total_iterations = 0

        _LOGGER.info(
            "Starting timed stress execution: test_ids=%s, mode='%s', "
            "target_duration=%.1fs, iterations_per_batch=%d",
            test_ids,
            mode,
            duration_sec,
            iterations_per_batch,
        )

        with USBTestIoctlBackend(device_path=dev_node) as backend:
            while True:
                elapsed = time.monotonic() - start_time
                if elapsed >= duration_sec:
                    break

                batch += 1
                remaining = duration_sec - elapsed
                _LOGGER.info(
                    "[Batch %d] Running testusb (Elapsed: %.1fs / %.1fs, "
                    "Remaining: %.1fs)...",
                    batch,
                    elapsed,
                    duration_sec,
                    remaining,
                )

                results = self.execute_testusb(
                    dev_node=dev_node,
                    test_ids=test_ids,
                    mode=mode,
                    iterations=iterations_per_batch,
                    length=length,
                    vary=vary,
                    sglen=sglen,
                    timeout_ms=timeout_ms,
                    backend=backend,
                )
                self.assert_testusb_success(
                    results,
                    f"Stress test batch {batch} failed for test_ids={test_ids} "
                    f"at elapsed {elapsed:.1f}s",
                )
                total_iterations += iterations_per_batch
                time.sleep(0.01)

        total_elapsed = time.monotonic() - start_time
        _LOGGER.info(
            "Completed timed stress test successfully: %d batches, "
            "%d total iterations across %.2fs.",
            batch,
            total_iterations,
            total_elapsed,
        )

    def assert_testusb_success(
        self,
        results: list[TestResult],
        msg: str | None = None,
    ) -> None:
        """Assert all executed testusb test cases succeeded or were skipped.

        Args:
            results: List of TestResult objects returned by execute_testusb.
            msg: Optional failure message prefix.
        """
        asserts.assert_true(
            len(results) > 0,
            f"{msg or 'No test results returned'}: results list is empty",
        )

        all_skipped = all(r.status == TestStatus.SKIP for r in results)
        if all_skipped:
            skip_msgs = [
                f"Test {r.test_id}: {r.error_message or 'unsupported'}"
                for r in results
            ]
            asserts.skip(
                "All tests skipped (unsupported by host kernel usbtest "
                f"driver): {'; '.join(skip_msgs)}"
            )

        failed = [
            r
            for r in results
            if r.status in (TestStatus.FAIL, TestStatus.ERROR)
        ]
        if failed:
            details = "\n".join(
                f"Test {r.test_id} ({r.test_name}): {r.status.value} - "
                f"{r.error_message or 'Unknown error'}"
                for r in failed
            )
            asserts.fail(f"{msg or 'testusb execution failed'}:\n{details}")
