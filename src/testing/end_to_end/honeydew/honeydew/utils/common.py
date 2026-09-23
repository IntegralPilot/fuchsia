# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Common utils used across Honeydew."""
import asyncio
import inspect
import logging
import signal
import time
import types
from collections.abc import Awaitable, Callable, Generator
from contextlib import contextmanager
from typing import Any

from honeydew import errors
from honeydew.utils import decorators

_LOGGER: logging.Logger = logging.getLogger(__name__)


def _retry_condition(end_time: float | None = None) -> bool:
    if end_time is None:
        return True

    return time.time() < end_time


@decorators.liveness_check
async def wait_for_state(
    state_fn: (Callable[[], bool] | Callable[[], Awaitable[bool]]),
    expected_state: bool,
    timeout: float | None = None,
    wait_time: float = 1,
) -> None:
    """Wait for specified time for state_fn to return expected_state.

    Args:
        state_fn: function to call for getting current state.
        expected_state: expected state to wait for.
        timeout: How long in sec to wait. By default, no timeout is set.
        wait_time: How long in sec to wait between the retries.

    Raises:
        errors.HoneydewTimeoutError: If state_fn does not return the
            expected_state with in specified timeout.
    """
    message: str = (
        f"Waiting for {state_fn.__qualname__} to return {expected_state}..."
    )
    end_time: float | None = None

    if timeout:
        start_time: float = time.time()
        end_time = start_time + timeout

        message = (
            f"Waiting for {timeout} sec for {state_fn.__qualname__} "
            f"to return {expected_state}..."
        )

    _LOGGER.info(message)

    while _retry_condition(end_time):
        _LOGGER.debug("calling %s", state_fn.__qualname__)
        try:
            current_state = state_fn()
            if inspect.isawaitable(current_state):
                current_state = await current_state

            _LOGGER.debug(
                "%s returned %s", state_fn.__qualname__, current_state
            )
            if current_state == expected_state:
                return
        except Exception as err:  # pylint: disable=broad-except
            # `state_fn()` raised an exception. Retry again
            _LOGGER.debug(err)
        await asyncio.sleep(wait_time)
    else:
        message = f"{state_fn.__qualname__} didn't return {expected_state}"
        if timeout:
            message += f" in {timeout} sec"
        raise errors.HoneydewTimeoutError(message)


@decorators.liveness_check
async def retry(
    fn: (Callable[[], object] | Callable[[], Awaitable[object]]),
    timeout: float | None = None,
    wait_time: int = 1,
) -> object:
    """Wait for specified time for fn to succeed.

    Args:
        fn: function to call.
        timeout: How long in sec to retry in case of failure. By default, no
            timeout is set.
        wait_time: How long in sec to wait between the retries.

    Raises:
        errors.HoneydewTimeoutError: If fn does not succeed with in specified
            timeout.
    """
    message: str = (
        f"Run {fn.__qualname__} until it succeeds, with wait time "
        f"of {wait_time}sec between the retries..."
    )
    end_time: float | None = None

    if timeout:
        start_time: float = time.time()
        end_time = start_time + timeout

        message = (
            f"Run {fn.__qualname__} until it succeeds or {timeout}sec timeout "
            f"hit, with wait time of {wait_time}sec between the retries..."
        )

    _LOGGER.info(message)

    while _retry_condition(end_time):
        _LOGGER.debug("calling %s", fn.__qualname__)
        try:
            ret_value = fn()
            if inspect.isawaitable(ret_value):
                ret_value = await ret_value

            _LOGGER.debug("%s returned %s", fn.__qualname__, ret_value)
            _LOGGER.info("Successfully finished %s.", fn.__qualname__)
            return ret_value
        except Exception as err:  # pylint: disable=broad-except
            # `fn()` raised an exception. Retry again
            _LOGGER.warning(
                "%s failed with error: %s. Retry again in %s sec",
                fn.__qualname__,
                err,
                wait_time,
            )
        await asyncio.sleep(wait_time)
    else:
        raise errors.HoneydewTimeoutError(
            f"{fn.__qualname__} didn't succeed in {timeout} sec"
        )


def read_from_dict(
    d: dict[str, Any],
    key_path: tuple[str, ...],
    should_exist: bool = True,
) -> Any:
    """Given a (nested) dictionary and path to the key, return value at that key.

    Args:
        d: Input nested dictionary
        key_path: path from root key to destination key
        should_exist: If set to True, exception will be raised if key does not exist. Otherwise,
            returns None.

    Returns:
        Value of the key if exist. Return None or raise exception, otherwise

    Raises:
        errors.ConfigError: Failed to traverse to the key using path
    """
    try:
        traversed_path: list[str] = []
        item_value: Any = d
        for item in key_path:
            item_value = item_value[item]
            traversed_path.append(item)
        return item_value
    except KeyError as err:
        traversed_path.append(item)
        _LOGGER.info(
            "'%s' does not exist in the dict: %s",
            traversed_path,
            d,
        )
        if should_exist:
            raise errors.ConfigError(
                f"'{traversed_path}' does not exist in the config dict passed during init."
            ) from err
        return None


@contextmanager
def time_limit(
    timeout: int,
    exception_type: type[Exception] = errors.HoneydewTimeoutError,
    exception_message: str = "",
) -> Generator[None, None, None]:
    """Context manager that can be used to time limit any function/method.

    Args:
        timeout: time limit allowed in seconds.
        exception_type: type of exception to be raised on reaching the time limit. If not passed,
            errors.HoneydewTimeoutError exception will be used.
        exception_message: Error message to be used (if any) while raising the exception on reaching
            the time limit.

    Raises:
        errors.HoneydewTimeoutError: If time limit is reached and exception_type is None.
    """

    def sigalarm_handler(signum: int, _: types.FrameType | None) -> None:
        _LOGGER.debug(
            "Received signal: %s (%s). Raising exception: %s('%s')",
            signal.Signals(signum).name,
            signum,
            exception_type.__name__,
            exception_message,
        )
        raise exception_type(exception_message)

    signal.signal(signal.SIGALRM, sigalarm_handler)
    signal.alarm(timeout)
    try:
        yield
    finally:
        signal.alarm(0)
