# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import asyncio
import copy
import functools
import inspect
import logging
import typing
from collections.abc import Coroutine
from functools import wraps
from typing import Any, Callable, ParamSpec, Sequence, TypeVar

from mobly import base_test, records, signals

_ASYNC_EVENT_LOOP: asyncio.AbstractEventLoop = asyncio.new_event_loop()


def get_loop() -> asyncio.AbstractEventLoop:
    return _ASYNC_EVENT_LOOP


P = ParamSpec("P")
T = TypeVar("T")


def _make_sync_wrapper(
    func: Callable[P, Coroutine[Any, Any, T]]
) -> Callable[P, Coroutine[Any, Any, T] | T]:
    @wraps(func)
    def wrapper(
        *args: P.args, **kwargs: P.kwargs
    ) -> Coroutine[Any, Any, T] | T:
        loop: asyncio.AbstractEventLoop | None = None
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            pass

        # If there was no event loop, then run func on the global event loop.
        # This is a Mobly synchronous entry point.
        if loop is None:
            return get_loop().run_until_complete(func(*args, **kwargs))
        # If an event loop is running, return the coroutine directly.
        # The caller will await the coroutine because the calling test
        # test code must already be running in an async context.
        else:
            return func(*args, **kwargs)

    return wrapper


if typing.TYPE_CHECKING:
    from _typeshed import Incomplete
    from mobly import base_test, controller_manager, records, runtime_test_info

    # LINT.IfChange
    class _MoblyStub(base_test.BaseTestClass):
        TAG: str
        tests: list[str]
        root_output_path: str
        log_path: str
        user_params: dict[str, Any]
        controller_configs: dict[str, Any]
        current_test_info: Any
        _controller_manager: controller_manager.ControllerManager

    # LINT.ThenChange(//src/testing/end_to_end/stubs/mobly/base_test.pyi)
    _BaseTestClass = _MoblyStub
else:
    _BaseTestClass = base_test.BaseTestClass


class _AsyncBaseTestClassMeta(_BaseTestClass):
    _MOBLY_INHERITED_METHOD_NAMES = [
        "pre_run",
        "setup_generated_tests",
        "setup_class",
        "teardown_class",
        "setup_test",
        "teardown_test",
        "on_fail",
        "on_pass",
        "on_skip",
    ]

    def __init_subclass__(cls) -> None:
        super().__init_subclass__()

        dict_items = list(cls.__dict__.items())
        for attr_name, attr_value in dict_items:
            # Handle Mobly lifecycle methods
            if (
                attr_name in cls._MOBLY_INHERITED_METHOD_NAMES
                and inspect.iscoroutinefunction(attr_value)
            ):
                async_attr_name = f"_async_{attr_name}"
                setattr(cls, async_attr_name, attr_value)
                setattr(cls, attr_name, _make_sync_wrapper(attr_value))

            # Handle async test methods
            elif attr_name.startswith("test_") and inspect.iscoroutinefunction(
                attr_value
            ):
                async_attr_name = f"_async_{attr_name}"
                setattr(cls, async_attr_name, attr_value)
                setattr(cls, attr_name, _make_sync_wrapper(attr_value))


class TestCases:
    """Base class for modular test cases."""

    def __init__(self) -> None:
        self._mobly_test: AsyncBaseTestClass | None = None

    @property
    def test(self) -> AsyncBaseTestClass:
        assert (
            self._mobly_test is not None
        ), "mobly_test was not injected properly."
        return self._mobly_test

    async def setup_test(
        self,
    ) -> None:
        """Called before each test case."""

    async def teardown_test(self) -> None:
        """Called after each test case."""

    @classmethod
    def inject_test_cases(cls, mobly_test: "AsyncBaseTestClass") -> None:
        tc_instance = cls()
        tc_instance._mobly_test = mobly_test
        for attr_name, method in inspect.getmembers(cls, callable):
            if attr_name.startswith("test_"):

                @functools.wraps(method)
                async def wrapper(
                    *args: Any, method: Any = method, **kwargs: Any
                ) -> None:
                    try:
                        await tc_instance.setup_test()

                        if inspect.iscoroutinefunction(method):
                            await method(tc_instance, *args, **kwargs)
                        else:
                            method(tc_instance, *args, **kwargs)
                    finally:
                        await tc_instance.teardown_test()

                mobly_test.generate_tests(
                    test_logic=wrapper,
                    name_func=lambda *a, name=attr_name: name,
                    arg_sets=[()],
                )


class AsyncBaseTestClass(_AsyncBaseTestClassMeta):
    # These methods intentionally mask their Mobly synchronous counterparts.
    # This ensures each subclass of this one will define these methods as async,
    # because mypy checks will enforce that. Then the __init_subclass__ in
    # _AsyncBaseTestClassMeta will wrap them with _make_sync_wrapper.

    TEST_CASES: Sequence[type[TestCases]] | None = None

    async def pre_run(self) -> None:
        if self.TEST_CASES is None:
            return

        for tc in self.TEST_CASES:
            tc.inject_test_cases(self)

    async def setup_generated_tests(self) -> None:
        pass

    async def setup_class(self) -> None:
        pass

    async def teardown_class(self) -> None:
        pass

    async def setup_test(self) -> None:
        pass

    async def teardown_test(self) -> None:
        pass

    async def on_fail(self, record: records.TestResultRecord) -> None:
        pass

    async def on_pass(self, record: records.TestResultRecord) -> None:
        pass

    async def on_skip(self, record: records.TestResultRecord) -> None:
        pass

    def generate_tests(
        self,
        test_logic: Callable[P, None | Coroutine[Any, Any, None]],
        name_func: Callable[P, str],
        arg_sets: Sequence[Any],
        uid_func: Callable[P, str] | None = None,
    ) -> None:
        if asyncio.iscoroutinefunction(test_logic):

            @wraps(test_logic)
            def wrapper(*t_args: P.args, **t_kwargs: P.kwargs) -> None:
                loop: asyncio.AbstractEventLoop | None = None
                try:
                    loop = asyncio.get_running_loop()
                except RuntimeError:
                    pass

                # This should be the typical case of Mobly calling a test method
                # from a synchronous context. Run the future on the global
                # event loop.
                if loop is None:
                    return get_loop().run_until_complete(
                        test_logic(*t_args, **t_kwargs)
                    )
                # This case indicates the test method was called from an async context,
                # but only Mobly calls test methods, and Mobly always calls them
                # from a synchronous context. Therefore, reaching this case indicates
                # something has gone wrong.
                else:
                    raise RuntimeError(
                        "Test logic was called from an active coroutine context, implying it wasn't called by Mobly."
                    )

            return super().generate_tests(
                wrapper, name_func, arg_sets, uid_func
            )
        return super().generate_tests(test_logic, name_func, arg_sets, uid_func)

    async def register_controller(
        self, module: Any, required: bool = True, min_number: int = 1
    ) -> list[Any]:
        module_ref_name = module.__name__.split(".")[-1]
        module_config_name = module.MOBLY_CONTROLLER_CONFIG_NAME

        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = None

        # Check if the controller has an async create method
        if hasattr(module, "create") and inspect.iscoroutinefunction(
            module.create
        ):
            if loop is not None:
                # We are in an active event loop. We cannot use run_until_complete.
                # We must bypass Mobly's register_controller and do it ourselves.
                #
                # FRAGILITY NOTE: This approach is fragile because it accesses private
                # attributes of Mobly's ControllerManager (`_controller_objects` and
                # `_controller_modules`). If Mobly changes its internal implementation,
                # this code may break.
                #
                # TESTING APPROACH: To mitigate this fragility, we have unit tests in
                # `fuchsia_async_extension_test.py` that test both Mobly's
                # `register_controller` and this overridden version against the same
                # expected behaviors (e.g. config checks, min_number enforcement).
                # If Mobly changes, those tests should fail and alert us.

                if module_config_name not in self.controller_configs:
                    if required:
                        raise signals.ControllerError(
                            f"No corresponding config found for {module_config_name}"
                        )
                    logging.warning(
                        f"No corresponding config found for optional controller {module_config_name}"
                    )
                    return []

                original_config = self.controller_configs[module_config_name]
                controller_config = copy.deepcopy(original_config)

                # Await the async create directly
                objects = await module.create(controller_config)

                if not isinstance(objects, list):
                    raise signals.ControllerError(
                        f"Controller module {module_ref_name} did not return a list of objects, abort."
                    )

                actual_number = len(objects)
                if actual_number < min_number:
                    if hasattr(
                        module, "destroy"
                    ) and inspect.iscoroutinefunction(module.destroy):
                        await module.destroy(objects)
                    elif hasattr(module, "destroy"):
                        module.destroy(objects)
                    raise signals.ControllerError(
                        f"Expected to get at least {min_number} controller objects, got {actual_number}."
                    )

                # Manually register objects in Mobly's controller manager
                self._controller_manager._controller_objects[
                    module_ref_name
                ] = copy.copy(objects)
                self._controller_manager._controller_modules[
                    module_ref_name
                ] = module
                res = objects
            else:
                # No event loop running. Safe to use run_until_complete via a sync wrapper.
                original_create = module.create

                def sync_create(*args: Any, **kwargs: Any) -> Any:
                    return get_loop().run_until_complete(
                        original_create(*args, **kwargs)
                    )

                setattr(module, "create", sync_create)
                res = super().register_controller(module, required, min_number)
        else:
            # Synchronous create or no create, use default Mobly behavior
            res = super().register_controller(module, required, min_number)

        # Patch module.destroy and module.get_info to run synchronously for Mobly teardown.
        if hasattr(module, "destroy"):
            original_destroy = getattr(module, "destroy")

            if inspect.iscoroutinefunction(original_destroy):

                def sync_destroy(*args: Any, **kwargs: Any) -> None:
                    get_loop().run_until_complete(
                        original_destroy(*args, **kwargs)
                    )

                setattr(module, "destroy", sync_destroy)

        if hasattr(module, "get_info"):
            original_get_info = getattr(module, "get_info")

            def sync_get_info(*args: Any, **kwargs: Any) -> list[Any]:
                if inspect.iscoroutinefunction(original_get_info):
                    return get_loop().run_until_complete(
                        original_get_info(*args, **kwargs)
                    )
                return original_get_info(*args, **kwargs)

            setattr(module, "get_info", sync_get_info)

        return res


F = typing.TypeVar("F", bound=typing.Callable[..., typing.Any])


def retry(
    count: int, max_consecutive_error: int | None = None
) -> Callable[[F], F]:
    """Decorator for retrying a test case until it passes.

    This is a copy of mobly.base_test.retry that supports both sync and
    async methods.
    """
    if count <= 1:
        raise ValueError(
            f'The `count` for `repeat` must be larger than 1, got "{count}".'
        )

    if max_consecutive_error is not None and max_consecutive_error > count:
        raise ValueError(
            f"The `max_consecutive_error` ({max_consecutive_error}) for `repeat` "
            f"must be smaller than `count` ({count})."
        )

    def _outer_decorator(
        func: F,
    ) -> F:
        setattr(func, base_test.ATTR_MAX_RETRY_CNT, count)
        setattr(func, base_test.ATTR_MAX_CONSEC_ERROR, max_consecutive_error)

        if inspect.iscoroutinefunction(func):

            @functools.wraps(func)
            async def _async_wrapper(*args: Any, **kwargs: Any) -> Any:
                return await func(*args, **kwargs)

            return typing.cast(F, _async_wrapper)

        @functools.wraps(func)
        def _wrapper(*args: Any, **kwargs: Any) -> Any:
            return func(*args, **kwargs)

        return typing.cast(F, _wrapper)

    return _outer_decorator
