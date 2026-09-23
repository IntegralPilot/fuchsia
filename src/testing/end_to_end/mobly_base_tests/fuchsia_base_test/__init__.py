# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Async Fuchsia base test class."""

from __future__ import annotations

import enum
import importlib
import logging
import os
import pathlib
import typing
from typing import Any, Dict, ParamSpec, TypeVar, Union

import fuchsia_async_extension
import mobly_logger_extension
from honeydew import errors
from honeydew.auxiliary_devices.power_switch import (
    power_switch,
    power_switch_using_dmc,
)
from honeydew.auxiliary_devices.usb_power_hub import (
    usb_power_hub,
    usb_power_hub_using_dmc,
)
from honeydew.fuchsia_device import fuchsia_device
from honeydew.typing import custom_types
from mobly import config_parser as mobly_config_parser
from mobly import signals
from mobly.records import TestResultRecord
from mobly_controller import fuchsia_device as fuchsia_device_mobly_controller

_LOGGER: logging.Logger = logging.getLogger(__name__)

P = ParamSpec("P")
T = TypeVar("T")


# LINT.IfChange
HEALTH_CHECK_FAILURE_MESSAGE = (
    "One or more FuchsiaDevice's health check failed in "
    "teardown_test. So failing the test case..."
)
# LINT.ThenChange(//tools/testing/tefmocheck/string_in_log_check.go)


class SnapshotOn(enum.StrEnum):
    """How often we need to collect the snapshot"""

    # Once per test case.
    TEARDOWN_TEST = "teardown_test"

    # Once per test case on failure only.
    TEARDOWN_TEST_ON_FAIL = "teardown_test_on_fail"

    # Once per test class.
    TEARDOWN_CLASS = "teardown_class"

    # Once per test class on failure only.
    TEARDOWN_CLASS_ON_FAIL = "teardown_class_on_fail"

    # Do not collect snapshot
    NEVER = "never"


class TracingOn(enum.StrEnum):
    """Tracing behavior for tests.

    This user param does not support any tests that reboot the device.
    """

    # Once per test case.
    TEARDOWN_TEST = "teardown_test"

    # Once per test case on failure only.
    TEARDOWN_TEST_ON_FAIL = "teardown_test_on_fail"

    # Once per test class.
    TEARDOWN_CLASS = "teardown_class"

    # Once per test class on failure only.
    TEARDOWN_CLASS_ON_FAIL = "teardown_class_on_fail"

    # Do not collect
    NEVER = "never"


class FuchsiaTestCases(fuchsia_async_extension.TestCases):
    TAG: str
    tests: list[str]
    root_output_path: str
    log_path: str
    user_params: dict[str, Any]
    controller_configs: dict[str, Any]
    current_test_info: Any

    fuchsia_devices: list[fuchsia_device.FuchsiaDevice]
    dut: fuchsia_device.FuchsiaDevice
    test_case_path: str
    snapshot_on: SnapshotOn
    tracing_on: TracingOn

    @property
    def test(self) -> FuchsiaBaseTest:
        # Downcast super().test to FuchsiaBaseTest. This avoids the
        # complexity of making FuchsiaTestCases a generic type which
        # leads to a paradox where we want FuchsiaTestCases to be
        # covariant but including the generic in any method makes
        # FuchsiaTestCases contravariant.
        test = super().test
        assert isinstance(test, FuchsiaBaseTest)
        return test

    async def setup_test(self) -> None:
        await super().setup_test()

        # Copy public fields from BaseTestClass
        self.TAG = self.test.TAG
        self.tests = self.test.tests
        self.root_output_path = self.test.root_output_path
        self.log_path = self.test.log_path
        self.user_params = self.test.user_params
        self.controller_configs = self.test.controller_configs
        self.current_test_info = self.test.current_test_info

        # Copy public fields from FuchsiaBaseTest
        self.fuchsia_devices = self.test.fuchsia_devices
        self.test_case_path = self.test.test_case_path
        self.snapshot_on = self.test.snapshot_on
        self.tracing_on = self.test.tracing_on
        self.dut = self.fuchsia_devices[0]

    # Copy public methods from FuchsiaBaseTest
    def output_file_path(self, file_name: str) -> pathlib.Path:
        return self.test.output_file_path(file_name)


class FuchsiaBaseTest(fuchsia_async_extension.AsyncBaseTestClass):
    """Async Fuchsia-specific base test class

    Attributes:
        fuchsia_devices: List of FuchsiaDevice objects.
        dut: The first FuchsiaDevice object in `fuchsia_devices`.
        test_case_path: Directory pointing to a specific test case artifacts.
        snapshot_on: `snapshot_on` test param value converted into SnapshotOn Enum.
        tracing_on: `tracing_on` test param value converted into TracingOn Enum.

    Optional Mobly Test Params:
        snapshot_on (str): One of "teardown_class", "teardown_class_on_fail",
            "teardown_test", "on_fail".
            Default value is "teardown_class_on_fail".
        tracing_on (str): One of "teardown_class", "teardown_class_on_fail",
            "teardown_test", "on_fail", "never".
            Default value is "never"
    """

    fuchsia_devices: list[fuchsia_device.FuchsiaDevice] = []
    dut: fuchsia_device.FuchsiaDevice
    test_case_path: str = ""
    snapshot_on: SnapshotOn = SnapshotOn.TEARDOWN_CLASS_ON_FAIL
    tracing_on: TracingOn = TracingOn.NEVER
    trace_categories: list[str] | None = None

    def __init__(
        self, mobly_configs: mobly_config_parser.TestRunConfig
    ) -> None:
        super().__init__(configs=mobly_configs)
        # We define teardown_class artifacts path here so it can be used by
        # child test classes in teardown_class before calling the super() teardown
        self._teardown_class_artifacts: str = f"{self.log_path}/teardown_class"
        self._any_test_failed: bool = False
        self._devices_not_healthy: bool = False

    async def setup_class(self) -> None:
        """setup_class is called once before running tests.

        It does the following things:
            * Reads user params passed to the test
            * Instantiates all fuchsia devices into self.fuchsia_devices (as async devices)
            * Instantiates and starts tracing if specified in the user params
        """

        self._process_metric_user_params()  # Sets self.tracing_on and self.snapshot_on

        self.fuchsia_devices = await self.register_controller(
            fuchsia_device_mobly_controller,
        )
        self.dut = self.fuchsia_devices[0]

        for device in self.fuchsia_devices:
            switch, outlet = self._lookup_power_switch(device)
            if switch is not None:
                device.set_power_switch(power_switch=switch, outlet=outlet)

            hub, port = self._lookup_usb_power_hub(device)
            if hub is not None:
                device.set_usb_power_hub(usb_power_hub=hub, port=port)
        if (
            self.tracing_on == TracingOn.TEARDOWN_CLASS
            or self.tracing_on == TracingOn.TEARDOWN_CLASS_ON_FAIL
        ):
            for device in self.fuchsia_devices:
                device.tracing.initialize(categories=self.trace_categories)
                await device.tracing.start()

    async def setup_test(self) -> None:
        """setup_test is called once before running each test.

        It does the following things:
            * Stores the current test case path into self.test_case_path
            * Logs a info message onto device that test case has started.
            * Instantiates and starts tracing if specified in the user params
        """
        self._devices_not_healthy = False

        self.test_case_path = f"{self.log_path}/{self.current_test_info.name}"
        os.mkdir(self.test_case_path)

        await self._log_message_to_devices(
            message=f"Started executing '{self.current_test_info.name}' "
            f"Lacewing test case...",
            level=custom_types.LEVEL.INFO,
        )
        for device in self.fuchsia_devices:
            if (
                not device.tracing.is_active()
                and not device.tracing.is_session_initialized()
            ):
                if (
                    self.tracing_on == TracingOn.TEARDOWN_TEST
                    or self.tracing_on == TracingOn.TEARDOWN_TEST_ON_FAIL
                ):
                    device.tracing.initialize(categories=self.trace_categories)
                    await device.tracing.start()

    async def teardown_test(self) -> None:
        """teardown_test is called once after running each test.

        It does the following things:
            * Takes snapshot of all the fuchsia devices and stores it under
              test case directory if `snapshot_on` test param is set to
              "teardown_test"
            * Logs a info message onto device that test case has ended.
        """
        await self._health_check_and_recover()

        if self.snapshot_on == SnapshotOn.TEARDOWN_TEST:
            await self._collect_snapshot(directory=self.test_case_path)

        _LOGGER.info("Closing any active tracing sessions.")
        for device in self.fuchsia_devices:
            if (
                device.tracing.is_active()
                and device.tracing.is_session_initialized()
            ):
                if self.tracing_on == TracingOn.TEARDOWN_TEST:
                    await device.tracing.stop()
                    await device.tracing.terminate_and_download(
                        directory=self.test_case_path
                    )

        _LOGGER.info("Completed closing active tracing sessions.")
        await self._log_message_to_devices(
            message=f"Finished executing '{self.current_test_info.name}' "
            f"Lacewing test case...",
            level=custom_types.LEVEL.INFO,
        )
        if len(os.listdir(self.test_case_path)) == 0:
            os.rmdir(self.test_case_path)

        if self._devices_not_healthy:
            _LOGGER.warning(HEALTH_CHECK_FAILURE_MESSAGE)
            raise signals.TestFailure(HEALTH_CHECK_FAILURE_MESSAGE)

    async def teardown_class(self) -> None:
        """teardown_class is called once after running all tests.

        It does the following things:
            * Takes snapshot of all the fuchsia devices and stores it under
              "<log_path>/teardown_class<_on_fail>" directory if `snapshot_on`
              test param is set to "teardown_class" or "teardown_class_on_fail".
            * Stops, terminates and downloads the trace data for all devices and stores
              it under "<log_path>/teardown_class<_on_fail>" directory if `tracing_on`
              test param is set to "teardown_class" or "teardown_class_on_fail".
            * Powers on any configured power switch outlets and USB power hub ports
              for all Fuchsia devices.
        """
        for device in self.fuchsia_devices:
            if (
                device.tracing.is_active()
                and device.tracing.is_session_initialized()
            ):
                if self.tracing_on == TracingOn.TEARDOWN_CLASS:
                    await device.tracing.stop()
                    await device.tracing.terminate_and_download(
                        directory=self._teardown_class_artifacts
                    )
                elif (
                    self.tracing_on == TracingOn.TEARDOWN_CLASS_ON_FAIL
                    and self._any_test_failed
                ):
                    await device.tracing.stop()
                    await device.tracing.terminate_and_download(
                        directory=self._teardown_class_artifacts
                    )

        if self.snapshot_on == SnapshotOn.TEARDOWN_CLASS:
            await self._collect_snapshot(
                directory=self._teardown_class_artifacts
            )
        elif (
            self.snapshot_on == SnapshotOn.TEARDOWN_CLASS_ON_FAIL
            and self._any_test_failed
        ):
            await self._collect_snapshot(
                directory=self._teardown_class_artifacts
            )

        for device in self.fuchsia_devices:
            switch, outlet = self._lookup_power_switch(device)
            if switch is not None:
                try:
                    switch.power_on(outlet=outlet)
                except Exception as err:
                    _LOGGER.warning(
                        "Failed to power on power switch for %s with error: %s",
                        device.device_name,
                        err,
                    )
            usb_power_hub, port = self._lookup_usb_power_hub(device)
            if usb_power_hub is not None:
                try:
                    usb_power_hub.power_on(port=port)
                except Exception as err:
                    _LOGGER.warning(
                        "Failed to power on USB power hub for %s with error: %s",
                        device.device_name,
                        err,
                    )

    async def on_fail(self, record: TestResultRecord) -> None:
        """on_fail is called once when a test case fails.

        It does the following things:
            * Aborts all remaining tests if the failure was caused by a
              FatalDeviceError.
            * Takes snapshot of all the fuchsia devices and stores it under
              test case directory if `snapshot_on` test param is set to
              "on_fail"
        """
        self._any_test_failed = True

        if record.termination_signal is not None and isinstance(
            record.termination_signal.exception, errors.FatalDeviceError
        ):
            _LOGGER.error(
                "Aborting all remaining tests due to fatal device error: %s",
                record.termination_signal.exception,
            )
            raise signals.TestAbortAll(
                "Aborting all remaining tests due to fatal device error: "
                f"{record.termination_signal.exception}"
            )

        if self.snapshot_on == SnapshotOn.TEARDOWN_TEST_ON_FAIL:
            await self._collect_snapshot(directory=self.test_case_path)

        for device in self.fuchsia_devices:
            if (
                device.tracing.is_active()
                and device.tracing.is_session_initialized()
            ):
                if self.tracing_on == TracingOn.TEARDOWN_TEST_ON_FAIL:
                    await device.tracing.stop()
                    await device.tracing.terminate_and_download(
                        directory=self.test_case_path
                    )

    def output_dir(self) -> pathlib.Path:
        if getattr(self, "test_case_path", None):
            return pathlib.Path(self.test_case_path)
        elif getattr(self, "log_path", None):
            return pathlib.Path(self.log_path)
        else:
            raise RuntimeError(
                "Neither self.test_case_path nor self.log_path exist: Has setup_class or setup_test been called yet?"
            )

    def output_file_path(self, file_name: str) -> pathlib.Path:
        return self.output_dir().joinpath(file_name)

    async def on_pass(self, record: TestResultRecord) -> None:
        pass

    async def on_skip(self, record: TestResultRecord) -> None:
        pass

    async def _collect_snapshot(self, directory: str) -> None:
        """Collects snapshots for all the FuchsiaDevice objects and stores them
        in the directory specified.

        Args:
            directory: Absolute path on the host where snapshot file need to be
                saved.
        """
        if not hasattr(self, "fuchsia_devices"):
            return

        _LOGGER.info(
            "Collecting snapshots of all the FuchsiaDevice objects in '%s'...",
            self.snapshot_on.value,
        )
        for fx_device in self.fuchsia_devices:
            try:
                await fx_device.snapshot(directory=directory)
            except Exception as err:
                _LOGGER.exception(
                    "Unable to take snapshot of %s. Failed with error: %s",
                    fx_device.device_name,
                    err,
                )

    def _get_controller_configs(
        self, controller_type: str
    ) -> list[dict[str, object]]:
        for (
            controller_name,
            controller_configs,
        ) in self.controller_configs.items():
            if controller_name == controller_type:
                return controller_configs
        return []

    def _get_device_config(
        self, controller_type: str, identifier_key: str, identifier_value: str
    ) -> dict[str, object]:
        for controller_config in self._get_controller_configs(controller_type):
            if controller_config[identifier_key] == identifier_value:
                _LOGGER.info(
                    "Device configuration associated with %s is %s",
                    identifier_value,
                    controller_config,
                )
                return controller_config
        return {}

    def _get_device_config_value(
        self,
        key: str,
        identifier_key: str,
        identifier_value: str,
        controller_type: str = "FuchsiaDevice",
    ) -> Any | None:
        config: Dict[str, Any] = self._get_device_config(
            controller_type=controller_type,
            identifier_key=identifier_key,
            identifier_value=identifier_value,
        )

        return config.get(key) if config else None

    async def _health_check_and_recover(self) -> None:
        """Ensure all FuchsiaDevice objects are healthy and if unhealthy perform
        a power_cycle in an attempt to recover.
        """
        _LOGGER.info(
            "Performing health checks on all the FuchsiaDevice objects..."
        )

        for fx_device in self.fuchsia_devices:
            try:
                fx_device.health_check()
            except errors.HealthCheckError as err:
                self._devices_not_healthy = True
                _LOGGER.warning(
                    "Health check on %s failed with error '%s', will try to recover the device",
                    fx_device.device_name,
                    err,
                )
                await self._recover_device(fx_device)

        _LOGGER.info(
            "Successfully performed health checks and/or recoveries on all the "
            "FuchsiaDevice objects..."
        )

    async def _recover_device(
        self, fx_device: fuchsia_device.FuchsiaDevice
    ) -> None:
        """Try to recover the fuchsia device by power cycling it if the test has
        access to a power switch.

        Args:
            fx_device: FuchsiaDevice object
        """
        try:
            await fx_device.power_cycle()
        except errors.NotSupportedError as err:
            _LOGGER.warning(
                "Unable to power cycle %s as power switch is not configured.",
                fx_device.device_name,
            )
            raise errors.FatalDeviceError(
                f"{fx_device.device_name} is unhealthy and unable to recover it"
            ) from err
        except (power_switch.PowerSwitchError, errors.HoneydewError) as err:
            _LOGGER.warning(
                "Power cycling %s failed with error '%s'.",
                fx_device.device_name,
                err,
            )
            raise errors.FatalDeviceError(
                f"{fx_device.device_name} is unhealthy and failed to recover it"
            ) from err

    def _lookup_power_switch(
        self, fx_device: fuchsia_device.FuchsiaDevice
    ) -> tuple[power_switch.PowerSwitch | None, int | None]:
        device_config: dict[str, object] = self._get_device_config(
            controller_type="FuchsiaDevice",
            identifier_key="name",
            identifier_value=fx_device.device_name,
        )
        power_switch_hw = typing.cast(
            dict[str, str], device_config.get("power_switch_hw", {})
        )
        power_switch_impl = typing.cast(
            dict[str, str], device_config.get("power_switch_impl", {})
        )
        power_switch_outlet = typing.cast(
            Union[int, None], device_config.get("power_switch_outlet", None)
        )
        if power_switch_hw and power_switch_impl:
            power_switch_class: type[power_switch.PowerSwitch] = getattr(
                importlib.import_module(power_switch_impl["module"]),
                power_switch_impl["class"],
            )
            return (
                power_switch_class(**power_switch_hw),
                power_switch_outlet,
            )
        elif power_switch_using_dmc.DMC_PATH_KEY in os.environ:
            return (
                power_switch_using_dmc.PowerSwitchUsingDmc(
                    device_name=fx_device.device_name,
                ),
                None,
            )
        else:
            return (None, None)

    def _lookup_usb_power_hub(
        self, fx_device: fuchsia_device.FuchsiaDevice
    ) -> tuple[usb_power_hub.UsbPowerHub | None, int | None]:
        device_config: dict[str, object] = self._get_device_config(
            controller_type="FuchsiaDevice",
            identifier_key="name",
            identifier_value=fx_device.device_name,
        )
        usb_power_hub_hw = typing.cast(
            dict[str, str], device_config.get("usb_power_hub_hw", {})
        )
        usb_power_hub_impl = typing.cast(
            dict[str, str], device_config.get("usb_power_hub_impl", {})
        )
        usb_power_hub_port = typing.cast(
            Union[int, None], device_config.get("usb_power_hub_port", None)
        )
        if usb_power_hub_hw and usb_power_hub_impl:
            usb_power_hub_class: type[usb_power_hub.UsbPowerHub] = getattr(
                importlib.import_module(usb_power_hub_impl["module"]),
                usb_power_hub_impl["class"],
            )
            return (
                usb_power_hub_class(**usb_power_hub_hw),
                usb_power_hub_port,
            )
        elif usb_power_hub_using_dmc.DMC_PATH_KEY in os.environ:
            return (
                usb_power_hub_using_dmc.UsbPowerHubUsingDmc(
                    device_name=fx_device.device_name,
                ),
                None,
            )
        else:
            return (None, None)

    async def _log_message_to_devices(
        self, message: str, level: custom_types.LEVEL
    ) -> None:
        """Log message in all the Fuchsia devices.

        Args:
            message: Message that need to logged.
            level: Log message level.
        """
        for fx_device in self.fuchsia_devices:
            try:
                await fx_device.log_message_to_device(message, level)
            except Exception as err:
                _LOGGER.exception(
                    "Unable to log message '%s' on '%s'. Failed with error: %s",
                    message,
                    fx_device.device_name,
                    err,
                )

    def _process_metric_user_params(self) -> None:
        _LOGGER.info(
            "user_params associated with the test: %s", self.user_params
        )

        try:
            if "snapshot_on" in self.user_params:
                self.snapshot_on = SnapshotOn(
                    self.user_params["snapshot_on"].lower()
                )
            if "tracing_on" in self.user_params:
                self.tracing_on = TracingOn(
                    self.user_params["tracing_on"].lower()
                )
            if "trace_categories" in self.user_params:
                self.trace_categories = self.user_params["trace_categories"]
                if not isinstance(self.trace_categories, list):
                    raise ValueError("trace_categories must be a list")
        except ValueError as e:
            raise signals.TestAbortClass("invalid metric user_param") from e
