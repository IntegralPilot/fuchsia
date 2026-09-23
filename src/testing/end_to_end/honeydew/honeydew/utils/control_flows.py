# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Control flow utilities: retries, timeouts, complex try-catch, etc"""

import asyncio
import logging
from collections.abc import Awaitable, Callable
from datetime import timedelta
from typing import TypeVar

from honeydew import errors
from honeydew.utils.deadline import Deadline
from mobly import signals

T = TypeVar("T")

_LOGGER: logging.Logger = logging.getLogger(__name__)

# TODO(b/402203873): Remove once in-tree is stabilized. This value is a global
# timeout enforced on all retry commands that have no deadline and is set to be
# large enough that no individual retry logic will timeout, but will definitely
# timeout before the entire test. This ensures that the test code will exit and
# attempt to do clean up, which gives us more logging in hang cases.
_GLOBAL_TASK_TIMEOUT = timedelta(seconds=900)

# TODO(b/421207005): Task Swarming will kill a whole test shard if there hasn't
# been any logging output for a configurable amount of time. As of Jun 11, 2025
# this timeout is normally set for 6 minutes. Periodically we'll emit logs even
# when suspended. Make sure to keep this well below any I/O timeouts.
_IDLE_LOGGING_PERIOD = timedelta(seconds=60)


class RetryAbortingError(Exception):
    def __init__(self, message: str) -> None:
        super().__init__(message)


class RetriableError(Exception):
    def __init__(self, message: str) -> None:
        super().__init__(message)


async def retry_until_deadline(
    task: Callable[[], Awaitable[T]],
    deadline: Deadline,
    retry_delay: timedelta = timedelta(seconds=1),
    backoff: bool = False,
) -> T:
    """Retries the given |task| until the deadline is due.

    If the |task| returns without error, halts. Otherwise,
    repeats after |retry_delay|. A task may also raise a permanent
    error of type RetryAbortingError to avoid more retries.

    If |backoff| is True, |retry_delay| will be doubled between each
    attempt.

    Returns:
        The value returned by |task|.

    Raises:
        *Exception: The last exception raised by the last call to |task|.
    """
    assert isinstance(deadline, Deadline)

    while True:
        try:
            return await task()
        except (
            RetryAbortingError,
            TypeError,
            NameError,
            SyntaxError,
            signals.TestError,
            signals.TestFailure,
            signals.TestAbortSignal,
            errors.FatalDeviceError,
        ):
            raise
        except Exception as e:  # pylint: disable=broad-exception-caught
            if deadline.is_due_before(retry_delay):
                raise e
            else:
                _LOGGER.info(
                    "%s raised %r. Next attempt in %s. (%s)",
                    _pretty_func_name(task),
                    e,
                    retry_delay,
                    deadline,
                )

        await sleep_for_duration(retry_delay)

        if backoff:
            retry_delay *= 2


async def retry(
    task: Callable[[], Awaitable[T]],
    max_tries: int | None = None,
    retry_delay: timedelta = timedelta(seconds=1),
    backoff: bool = False,
) -> T:
    """Retries the given |task| for up to |max_tries| times."""
    assert max_tries is None or max_tries > 0

    # TODO(b/402203873): Remove once in-tree is stabilized. See
    # `_GLOBAL_TASK_TIMEOUT` for an explanation.
    if max_tries is None or max_tries == 0:
        return await retry_for_duration(
            task=task,
            retry_delay=retry_delay,
            backoff=backoff,
            duration=_GLOBAL_TASK_TIMEOUT,
        )

    attempts: int = 0
    task_name: str = _pretty_func_name(task)
    while True:
        try:
            return await task()
        except (
            RetryAbortingError,
            TypeError,
            NameError,
            SyntaxError,
            signals.TestError,
            signals.TestFailure,
            signals.TestAbortSignal,
            errors.FatalDeviceError,
        ):
            raise
        except Exception as e:  # pylint: disable=broad-exception-caught
            attempts += 1
            limit = str(max_tries)
            if max_tries is None:
                limit = "<unbounded>"
            elif attempts == max_tries:
                _LOGGER.info(
                    "%s did not complete after %s attempts, aborting.",
                    task_name,
                    attempts,
                )
                raise e
            _LOGGER.info(
                "%s raised %r (Attempt %s of %s). Next attempt in %s.",
                task_name,
                e,
                attempts,
                limit,
                retry_delay,
            )

        await sleep_for_duration(retry_delay)

        if backoff:
            retry_delay *= 2


async def retry_for_duration(
    task: Callable[[], Awaitable[T]],
    duration: timedelta,
    retry_delay: timedelta = timedelta(seconds=1),
    backoff: bool = False,
) -> T:
    """Calls |retry_until_deadline| with a deadline based on |duration|"""
    return await retry_until_deadline(
        task, Deadline.from_timeout(duration), retry_delay, backoff
    )


async def repeat_until_deadline(
    task: Callable[[], Awaitable[T]],
    deadline: Deadline,
    repeat_delay: timedelta = timedelta(seconds=1),
) -> None:
    """Repeats |task| with until deadline is reached.

    If task fails, returns immediately. Between each repeat,
    sleeps for |repeat_delay|.
    """
    count = 0
    while not deadline.is_due():
        count += 1
        _LOGGER.debug(
            "Repeating %s for the %s time", _pretty_func_name(task), count
        )
        await task()
        if deadline.is_due_before(repeat_delay):
            break
        await sleep_for_duration(repeat_delay)


async def repeat_for_duration(
    task: Callable[[], Awaitable[T]],
    duration: timedelta,
    repeat_delay: timedelta = timedelta(seconds=1),
) -> None:
    """Calls |repeat_until_deadline| with a deadline based on |duration|"""
    await repeat_until_deadline(
        task, Deadline.from_timeout(duration), repeat_delay
    )


async def sleep_until_deadline(deadline: Deadline) -> None:
    """Sleeps until the deadline is reached.

    This function generates logs at intervals to prevent swarming from thinking
    we're frozen and timing us out.
    """
    if deadline == Deadline.infinite():
        raise ValueError("Cannot sleep for an infinite duration.")

    _LOGGER.debug("Sleeping until %s...", deadline)

    first_iteration = True
    while not deadline.is_due():
        if not first_iteration:
            _LOGGER.info("Still sleeping... %s", deadline)

        # Sleep for no longer than _IDLE_LOGGING_PERIOD, to ensure swarming
        # doesn't time us out.
        remaining = deadline.remaining_duration()
        assert (
            remaining is not None
        ), "We checked that the deadline was not infinite"

        sleep_duration = min(remaining, _IDLE_LOGGING_PERIOD)
        await asyncio.sleep(max(0, sleep_duration.total_seconds()))
        first_iteration = False
    _LOGGER.debug("Done sleeping!")


async def sleep_for_duration(duration: timedelta) -> None:
    """Sleeps for the length of this duration.

    This function generates logs at intervals to prevent swarming from thinking
    we're frozen and timing us out.
    """
    await sleep_until_deadline(Deadline.from_timeout(duration))


def _pretty_func_name(func: Callable[..., T]) -> str:
    if hasattr(func, "__qualname__"):
        return func.__qualname__
    else:
        return str(func)
