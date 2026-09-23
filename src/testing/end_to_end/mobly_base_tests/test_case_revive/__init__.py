# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import dataclasses
import enum
import inspect
import logging
from collections.abc import Callable
from typing import Awaitable, TypeVar

from fuchsia_base_test import FuchsiaBaseTest

_LOGGER: logging.Logger = logging.getLogger(__name__)


F = TypeVar("F", bound=Callable[..., object])


class FuchsiaDeviceOperation(enum.StrEnum):
    """Operation that need to be performed on Fuchsia Device."""

    NONE = "none"

    SOFT_REBOOT = "soft-reboot"

    HARD_REBOOT = "hard-reboot"

    POWER_CYCLE = "power-cycle"

    IDLE_SUSPEND_TIMER_RESUME = "idle-suspend-timer-resume"

    IDLE_SUSPEND_BUTTON_PRESS_RESUME = "idle-suspend-button-press-resume"

    USB_POWER_SUSPEND_RESUME = "usb-power-suspend-resume"


class TestMethodExecutionFrequency(enum.StrEnum):
    """How often the test case method need to be executed in the revived test case."""

    # This will result in: Run_Test -> Run_Device_Operation -> Run_Test
    PRE_AND_POST = "Pre-and-Post"

    # This will result in: Run_Test -> Run_Device_Operation
    PRE_ONLY = "Pre-Only"

    # This will result in: Run_Device_Operation -> Run_Test
    POST_ONLY = "Post-Only"


@dataclasses.dataclass(frozen=True)
class _TestArgTuple:
    test_case_method: str
    fuchsia_device_operation: FuchsiaDeviceOperation
    test_method_execution_frequency: TestMethodExecutionFrequency
    pre_test_execution_fn: Callable[..., None] | Callable[
        ..., Awaitable[None]
    ] | None = None
    pre_test_execution_fn_kwargs: dict[str, object] | None = None
    post_test_execution_fn: Callable[..., None] | Callable[
        ..., Awaitable[None]
    ] | None = None
    post_test_execution_fn_kwargs: dict[str, object] | None = None

    def __str__(self) -> str:
        return (
            f"TestCaseMethod:{self.test_case_method}, "
            f"FuchsiaDeviceOperation:{self.fuchsia_device_operation}, "
            f"TestMethodExecutionFrequency:{self.test_method_execution_frequency}, "
            f"PreTestExecutionFunction:{self.pre_test_execution_fn}, "
            f"PreTestExecutionFunctionKeywordArgs:{self.pre_test_execution_fn_kwargs}, "
            f"PostTestExecutionFunction:{self.post_test_execution_fn}, "
            f"PostTestExecutionFunctionKeywordArgs:{self.post_test_execution_fn_kwargs}, "
        )


def opt_out() -> Callable[[F], F]:
    """Decorator that will opt out a test case from "revive" """

    def opt_out_decorator(func: F) -> F:
        setattr(func, "_opt_out", True)
        return func

    return opt_out_decorator


def tag_test(
    fuchsia_device_operation: FuchsiaDeviceOperation | None = None,
    test_method_execution_frequency: TestMethodExecutionFrequency | None = None,
    pre_test_execution_fn: Callable[..., None]
    | Callable[..., Awaitable[None]]
    | None = None,
    pre_test_execution_fn_kwargs: dict[str, object] | None = None,
    post_test_execution_fn: Callable[..., None]
    | Callable[..., Awaitable[None]]
    | None = None,
    post_test_execution_fn_kwargs: dict[str, object] | None = None,
) -> Callable[[F], F]:
    """Decorator that can be used to tag a test with a label"""

    def tags_decorator(func: F) -> F:
        setattr(func, "_revive", True)
        if fuchsia_device_operation is not None:
            setattr(
                func,
                "_fuchsia_device_operation",
                FuchsiaDeviceOperation(fuchsia_device_operation),
            )
        if test_method_execution_frequency is not None:
            setattr(
                func,
                "_test_method_execution_frequency",
                TestMethodExecutionFrequency(test_method_execution_frequency),
            )
        if pre_test_execution_fn is not None:
            setattr(func, "_pre_test_execution_fn", pre_test_execution_fn)
            setattr(
                func,
                "_pre_test_execution_fn_kwargs",
                pre_test_execution_fn_kwargs,
            )
        if post_test_execution_fn is not None:
            setattr(func, "_post_test_execution_fn", post_test_execution_fn)
            setattr(
                func,
                "_post_test_execution_fn_kwargs",
                post_test_execution_fn_kwargs,
            )
        return func

    return tags_decorator


class TestCaseRevive(FuchsiaBaseTest):
    """Base class for TestCaseRevive."""

    async def pre_run(self) -> None:
        """Mobly method used to generate the test cases at run time."""
        await super().pre_run()

        self._test_case_revive: bool = self.user_params.get(
            "test_case_revive", False
        )

        if self._test_case_revive is False:
            _LOGGER.info(
                "[TestCaseRevive] - test_case_revive setting is not enabled "
                "in user_params. So not testing in revive mode...",
            )
            return

        _LOGGER.info(
            "[TestCaseRevive] - test_case_revive setting is enabled in "
            "user_params. So testing in revive mode...",
        )

        self._read_and_validate_user_params()

        revived_test_cases: list[str] = self._get_list_of_revived_test_cases()

        test_arg_tuple_list: list[
            _TestArgTuple
        ] = self._generate_test_args_tuple_list(revived_test_cases)

        self.generate_tests(
            test_logic=self._logic_for_test_case_revive,
            name_func=self._revived_test_case_name_func,
            arg_sets=[
                dataclasses.astuple(test_arg_tuple)
                for test_arg_tuple in test_arg_tuple_list
            ],
        )

    async def _perform_op(
        self, fuchsia_device_operation: FuchsiaDeviceOperation
    ) -> None:
        """Perform user specified operation"""

        for fuchsia_device in self.fuchsia_devices:
            if (
                fuchsia_device_operation
                == FuchsiaDeviceOperation.IDLE_SUSPEND_TIMER_RESUME
            ):
                await fuchsia_device.system_power_state_controller.idle_suspend_timer_based_resume(
                    duration=self.user_params["resume_timer_duration_sec"],
                    verify_duration=self.user_params.get(
                        "verify_duration", True
                    ),
                )
            elif fuchsia_device_operation == FuchsiaDeviceOperation.SOFT_REBOOT:
                await fuchsia_device.reboot()
            elif fuchsia_device_operation in [
                FuchsiaDeviceOperation.HARD_REBOOT,
                FuchsiaDeviceOperation.POWER_CYCLE,
            ]:
                await fuchsia_device.power_cycle()
            elif (
                fuchsia_device_operation
                == FuchsiaDeviceOperation.IDLE_SUSPEND_BUTTON_PRESS_RESUME
            ):
                raise ValueError(
                    f"'{fuchsia_device_operation}' operation is not supported by 'TestCaseRevive'"
                )

    async def _logic_for_test_case_revive(
        self,
        test_case: str,
        fuchsia_device_operation: FuchsiaDeviceOperation,
        test_method_execution_frequency: TestMethodExecutionFrequency,
        pre_test_execution_fn: Callable[..., None]
        | Callable[..., Awaitable[None]]
        | None,
        pre_test_execution_fn_kwargs: dict[str, object] | None,
        post_test_execution_fn: Callable[..., None]
        | Callable[..., Awaitable[None]]
        | None,
        post_test_execution_fn_kwargs: dict[str, object] | None,
    ) -> None:
        """TestCaseRevive logic"""

        sequence: str
        if (
            test_method_execution_frequency
            == TestMethodExecutionFrequency.PRE_AND_POST
        ):
            sequence = (
                f"{test_case} -> {fuchsia_device_operation} -> {test_case}"
            )
        elif (
            test_method_execution_frequency
            == TestMethodExecutionFrequency.PRE_ONLY
        ):
            sequence = f"{test_case} -> {fuchsia_device_operation}"
        else:  # TestMethodExecutionFrequency.POST_ONLY
            sequence = f"{fuchsia_device_operation} -> {test_case}"
        if pre_test_execution_fn:
            sequence = f"{pre_test_execution_fn.__qualname__} -> {sequence}"
        if post_test_execution_fn:
            sequence = f"{sequence} -> {post_test_execution_fn.__qualname__}"
        _LOGGER.info("[TestCaseRevive] - Revived test logic: %s", sequence)

        if pre_test_execution_fn:
            if pre_test_execution_fn_kwargs:
                _LOGGER.info(
                    "[TestCaseRevive] - Calling %s with args:%s which is the first step in the revived test sequence...",
                    pre_test_execution_fn.__qualname__,
                    pre_test_execution_fn_kwargs,
                )
                res = pre_test_execution_fn(
                    self, **pre_test_execution_fn_kwargs
                )
                if inspect.isawaitable(res):
                    await res
            else:
                _LOGGER.info(
                    "[TestCaseRevive] - Calling %s which is the first step in the revived test sequence...",
                    pre_test_execution_fn.__qualname__,
                )
                res = pre_test_execution_fn(self)
                if inspect.isawaitable(res):
                    await res

        if test_method_execution_frequency in [
            TestMethodExecutionFrequency.PRE_AND_POST,
            TestMethodExecutionFrequency.PRE_ONLY,
        ]:
            _LOGGER.info(
                "[TestCaseRevive] - Running the %s before performing %s operation...",
                test_case,
                fuchsia_device_operation,
            )
            res = getattr(self, test_case)()
            if inspect.isawaitable(res):
                await res

        _LOGGER.info(
            "[TestCaseRevive] - Performing %s operation on all Fuchsia devices "
            "that are part of the testbed...",
            fuchsia_device_operation,
        )
        await self._perform_op(
            fuchsia_device_operation=fuchsia_device_operation
        )

        if test_method_execution_frequency in [
            TestMethodExecutionFrequency.PRE_AND_POST,
            TestMethodExecutionFrequency.POST_ONLY,
        ]:
            _LOGGER.info(
                "[TestCaseRevive] - Running the %s after performing %s operation...",
                test_case,
                fuchsia_device_operation,
            )
            res = getattr(self, test_case)()
            if inspect.isawaitable(res):
                await res

        if post_test_execution_fn:
            if post_test_execution_fn_kwargs:
                _LOGGER.info(
                    "[TestCaseRevive] - Calling %s with args:%s which is the last step in the revived test sequence...",
                    post_test_execution_fn.__qualname__,
                    post_test_execution_fn_kwargs,
                )
                res = post_test_execution_fn(
                    self, **post_test_execution_fn_kwargs
                )
                if inspect.isawaitable(res):
                    await res
            else:
                _LOGGER.info(
                    "[TestCaseRevive] - Calling %s which is the last step in the revived test sequence...",
                    post_test_execution_fn.__qualname__,
                )
                res = post_test_execution_fn(self)
                if inspect.isawaitable(res):
                    await res

    def _revived_test_case_name_func(
        self,
        test_case: str,
        fuchsia_device_operation: FuchsiaDeviceOperation,
        test_method_execution_frequency: TestMethodExecutionFrequency,
        pre_test_execution_fn: Callable[..., None]
        | Callable[..., Awaitable[None]]
        | None,
        pre_test_execution_fn_kwargs: dict[str, object] | None,
        post_test_execution_fn: Callable[..., None]
        | Callable[..., Awaitable[None]]
        | None,
        post_test_execution_fn_kwargs: dict[str, object] | None,
    ) -> str:
        """Revived test case name function"""
        test_case = test_case.lstrip("_")
        return f"{test_case}_revived_with_{fuchsia_device_operation}"

    def _read_and_validate_user_params(self) -> None:
        """Read the user params associated with this test"""
        self._fuchsia_device_operation: str = self.user_params.get(
            "fuchsia_device_operation", FuchsiaDeviceOperation.NONE
        )
        try:
            self._fuchsia_device_operation_obj: FuchsiaDeviceOperation = (
                FuchsiaDeviceOperation(self._fuchsia_device_operation.lower())
            )
        except ValueError as err:
            raise ValueError(
                f"'{self._fuchsia_device_operation}' operation is not "
                f"supported by 'TestCaseRevive'"
            ) from err

        if (
            self._fuchsia_device_operation_obj
            == FuchsiaDeviceOperation.IDLE_SUSPEND_TIMER_RESUME
        ):
            if not isinstance(
                self.user_params.get("resume_timer_duration_sec"), int
            ):
                raise ValueError(
                    f"FuchsiaDeviceOperation: '{self._fuchsia_device_operation}' "
                    f"requires user to also pass 'resume_timer_duration_sec' key "
                    f"with int value in test prams"
                )

        self._test_method_execution_frequency: str = self.user_params.get(
            "test_method_execution_frequency",
            TestMethodExecutionFrequency.PRE_AND_POST,
        )
        try:
            self._test_method_execution_frequency_obj: (
                TestMethodExecutionFrequency
            ) = TestMethodExecutionFrequency(
                self._test_method_execution_frequency
            )
        except ValueError as err:
            raise ValueError(
                f"'{self._test_method_execution_frequency}' is not valid"
            ) from err

    def _get_list_of_revived_test_cases(self) -> list[str]:
        """Return the list of test cases that need to be revived."""
        test_cases: list[str] = [
            attribute
            for attribute in dir(self)
            if attribute.startswith(("test_", "_test_")) is True
            and callable(getattr(self, attribute))
        ]
        _LOGGER.info(
            "[TestCaseRevive] - List of all the test cases in this test "
            "class: %s",
            test_cases,
        )

        revived_test_cases = [
            test_case
            for test_case in test_cases
            if "_opt_out" not in dir(getattr(self, test_case))
            or not getattr(self, test_case)._opt_out
        ]
        _LOGGER.info(
            "[TestCaseRevive] - List of all the test cases in this test class "
            "that are configured to run with revived sequence: %s",
            revived_test_cases,
        )

        return revived_test_cases

    def _generate_test_args_tuple_list(
        self, revived_test_cases: list[str]
    ) -> list[_TestArgTuple]:
        """Generate the list of duple data structure that need to be passed to
        Mobly's generate_tests() method."""

        test_arg_tuple_list: list[_TestArgTuple] = []
        for revived_test_case in revived_test_cases:
            fuchsia_device_operation: FuchsiaDeviceOperation = (
                self._fuchsia_device_operation_obj
            )
            if "_fuchsia_device_operation" in dir(
                getattr(self, revived_test_case)
            ):
                fuchsia_device_operation = getattr(
                    self, revived_test_case
                )._fuchsia_device_operation

            test_method_execution_frequency: TestMethodExecutionFrequency = (
                self._test_method_execution_frequency_obj
            )
            if "_test_method_execution_frequency" in dir(
                getattr(self, revived_test_case)
            ):
                test_method_execution_frequency = getattr(
                    self, revived_test_case
                )._test_method_execution_frequency

            pre_test_execution_fn: Callable[..., None] | Callable[
                ..., Awaitable[None]
            ] | None = None
            if "_pre_test_execution_fn" in dir(
                getattr(self, revived_test_case)
            ):
                pre_test_execution_fn = getattr(
                    self, revived_test_case
                )._pre_test_execution_fn

            pre_test_execution_fn_kwargs: dict[str, object] | None = None
            if "_pre_test_execution_fn_kwargs" in dir(
                getattr(self, revived_test_case)
            ):
                pre_test_execution_fn_kwargs = getattr(
                    self, revived_test_case
                )._pre_test_execution_fn_kwargs

            post_test_execution_fn: Callable[..., None] | Callable[
                ..., Awaitable[None]
            ] | None = None
            if "_post_test_execution_fn" in dir(
                getattr(self, revived_test_case)
            ):
                post_test_execution_fn = getattr(
                    self, revived_test_case
                )._post_test_execution_fn

            post_test_execution_fn_kwargs: dict[str, object] | None = None
            if "_post_test_execution_fn_kwargs" in dir(
                getattr(self, revived_test_case)
            ):
                post_test_execution_fn_kwargs = getattr(
                    self, revived_test_case
                )._post_test_execution_fn_kwargs

            test_arg_tuple: _TestArgTuple = _TestArgTuple(
                test_case_method=revived_test_case,
                fuchsia_device_operation=fuchsia_device_operation,
                test_method_execution_frequency=test_method_execution_frequency,
                pre_test_execution_fn=pre_test_execution_fn,
                pre_test_execution_fn_kwargs=pre_test_execution_fn_kwargs,
                post_test_execution_fn=post_test_execution_fn,
                post_test_execution_fn_kwargs=post_test_execution_fn_kwargs,
            )

            test_arg_tuple_list.append(test_arg_tuple)

        _LOGGER.debug(
            "[TestCaseRevive] - Test arg list that will be passed to Mobly's "
            "generate_test method to generate the revived test cases is: %s",
            test_arg_tuple_list,
        )

        return test_arg_tuple_list
