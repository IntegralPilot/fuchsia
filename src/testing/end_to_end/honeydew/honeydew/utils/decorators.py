# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility module that contains some useful decorators."""

from __future__ import annotations

import asyncio
import functools
import logging
import multiprocessing
import os
import time
from collections.abc import Callable, Coroutine
from timeit import default_timer as timer
from typing import Any, ParamSpec, TypeVar

P = ParamSpec("P")
R = TypeVar("R")


_LIVENESS_CHECK_SLEEP_TIMER: float = 10.0

_LOGGER: logging.Logger = logging.getLogger(__name__)


def _truncate(s: str, limit: int = 40) -> str:
    """Truncates string s to limit characters with ellipsis."""
    return f"{s[:limit - 3]}..." if len(s) > limit else s


def _safe_repr(obj: Any, limit: int | None = None) -> str:
    """Safely gets repr() of obj, handling exceptions and truncating."""
    try:
        s = repr(obj)
    except Exception:
        s = f"<{type(obj).__name__}>"
    if limit is not None:
        return _truncate(s, limit=limit)
    return _truncate(s)


def _format_operation(
    func: Callable[..., Any],
    args: tuple[Any, ...],
    kwargs: dict[str, Any],
) -> str:
    """Formats a concise human-readable description of an operation.

    For command-line executions (e.g. host shell or FFX commands passed via
    ``cmd`` in kwargs or positional args), this extracts the binary name,
    filters out noisy internal options (such as ``--config``, ``-c``, ``-o``,
    JSON payloads), normalizes file paths to basenames, and truncates long
    commands.

    For callable and method executions, this formats the callable/class name
    and arguments with safe representation and truncation.

    Examples:
        Command execution:
            >>> # Positional command execution
            >>> _format_operation(func, args=(["/bin/sleep", "5"],), kwargs={})
            "'sleep 5'"

            >>> # FFX command with noisy config and target
            >>> _format_operation(
            ...     func,
            ...     args=(),
            ...     kwargs={
            ...         "cmd": [
            ...             "/path/to/ffx",
            ...             "--config", '{"log": {"level": "debug"}}',
            ...             "-t", "fuchsia-54b7-e1d8-4a53",
            ...             "target", "wait",
            ...         ],
            ...     },
            ... )
            "'ffx -t fuchsia-54b7-e1d8-4a53 target wait'"

        Callable execution:
            >>> # Function call with arguments
            >>> _format_operation(my_func, args=("foo", [1, 2]), kwargs={"timeout": 10})
            "'my_func('foo', [1, 2], timeout=10)'"

            >>> # Class instance method
            >>> _format_operation(device.reboot, args=(device, "bootloader"), kwargs={})
            "'FuchsiaDevice.reboot('bootloader')'"
    """
    cmd = (
        kwargs.get("cmd")
        if "cmd" in kwargs
        else (args[0] if args and isinstance(args[0], (list, tuple)) else None)
    )
    if isinstance(cmd, (list, tuple)) and cmd:
        binary: str = os.path.basename(str(cmd[0])) if str(cmd[0]) else ""
        clean_args: list[str] = []
        skip_next: bool = False
        is_ffx: bool = binary == "ffx"

        for arg in cmd[1:]:
            arg_str: str = str(arg)
            if skip_next:
                skip_next = False
            elif is_ffx and arg_str in ("-c", "-o", "--config"):
                skip_next = True
            elif arg_str in ("--strict", "--direct") or arg_str.startswith(
                ("{", "[{")
            ):
                continue
            else:
                if (
                    "/" in arg_str
                    and not arg_str.startswith("-")
                    and not arg_str.startswith(("http://", "https://"))
                ):
                    arg_str = os.path.basename(os.path.normpath(arg_str))
                clean_args.append(arg_str)

        args_str: str = " ".join(clean_args)
        cmd_summary: str = (
            f"{binary} {args_str}".strip() if args_str else binary
        )
        return f"'{_truncate(cmd_summary, 100)}'"

    func_name: str = getattr(
        func,
        "__name__",
        getattr(getattr(func, "func", None), "__name__", repr(func)),
    )
    display_name: str = func_name
    remaining_args = args
    if args and hasattr(type(args[0]), func_name):
        display_name = f"{args[0].__class__.__name__}.{func_name}"
        remaining_args = args[1:]

    items: list[str] = [_safe_repr(a) for a in remaining_args]
    items.extend(f"{k}={_safe_repr(v)}" for k, v in kwargs.items())
    return f"'{display_name}({', '.join(items)})'"


# LINT.IfChange(liveness_check)
# Unit test for this is covered as part of unit_tests/utils_tests/decorators_test.py
def liveness_check(func: Callable[P, R]) -> Callable[P, R]:
    """Decorator that prints liveness check messages until the function
    call is completed."""

    @functools.wraps(func)
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
        full_operation: str = f"{func.__name__}(args={args}, kwargs={kwargs})"
        short_operation: str = _format_operation(func, args, kwargs)

        proc = multiprocessing.Process(
            target=_liveness_check_logger,
            kwargs={
                "operation": short_operation,
            },
            daemon=True,
        )

        _LOGGER.debug(
            "[Liveness Check]: Starting a new process to track the liveness "
            "of '%s'",
            full_operation,
        )
        proc.start()

        _LOGGER.info(
            "[Liveness Check]: Running %s...",
            short_operation,
        )
        _LOGGER.debug(
            "[Liveness Check]: Running '%s'...",
            full_operation,
        )
        start: float = timer()
        try:
            result: R = func(*args, **kwargs)
            return result
        finally:
            _LOGGER.debug(
                "[Liveness Check]: Stopping the process that was created to "
                "track the liveness of '%s'",
                full_operation,
            )
            proc.kill()
            proc.join()

            duration: float = timer() - start

            _LOGGER.info(
                "[Liveness Check]: %s has been completed in %.1fs.",
                short_operation,
                duration,
            )
            _LOGGER.debug(
                "[Liveness Check]: '%s' has been completed in %.1fs.",
                full_operation,
                duration,
            )

    return wrapper


def _liveness_check_logger(
    operation: str,
) -> None:
    """Helper method used by `@liveness_check()` decorator to log liveness
    messages onto console.

    Args:
        operation: Human-readable name of the operation.
    """
    start: float = timer()
    while True:
        time.sleep(_LIVENESS_CHECK_SLEEP_TIMER)
        elapsed: int = int(timer() - start)
        _LOGGER.info(
            "[Liveness Check]: Still waiting on %s operation to finish (elapsed: %ss)",
            operation,
            elapsed,
        )


# LINT.ThenChange(:async_liveness_check)


# LINT.IfChange(async_liveness_check)
def async_liveness_check(
    func: Callable[P, Coroutine[Any, Any, R]]
) -> Callable[P, Coroutine[Any, Any, R]]:
    """Decorator that prints liveness check messages until the async function
    call is completed."""

    @functools.wraps(func)
    async def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
        full_operation: str = f"{func.__name__}(args={args}, kwargs={kwargs})"
        short_operation: str = _format_operation(func, args, kwargs)

        # Create a task to run the logger asynchronously.
        logger_task = asyncio.create_task(
            _async_liveness_check_logger(short_operation)
        )

        _LOGGER.debug(
            "[Async Liveness Check]: Starting a task to track the liveness "
            "of '%s'",
            full_operation,
        )

        _LOGGER.info(
            "[Async Liveness Check]: Running %s...",
            short_operation,
        )
        _LOGGER.debug(
            "[Async Liveness Check]: Running '%s'...",
            full_operation,
        )
        start: float = timer()
        try:
            result: R = await func(*args, **kwargs)
            return result
        finally:
            _LOGGER.debug(
                "[Async Liveness Check]: Stopping the task that was created to "
                "track the liveness of '%s'",
                full_operation,
            )
            logger_task.cancel()
            try:
                await logger_task
            except asyncio.CancelledError:
                pass

            duration: float = timer() - start

            _LOGGER.info(
                "[Async Liveness Check]: %s has been completed in %.1fs.",
                short_operation,
                duration,
            )
            _LOGGER.debug(
                "[Async Liveness Check]: '%s' has been completed in %.1fs.",
                full_operation,
                duration,
            )

    return wrapper


async def _async_liveness_check_logger(
    operation: str,
) -> None:
    """Helper method used by `@async_liveness_check()` decorator to log liveness
    messages onto console.

    Args:
        operation: Human-readable name of the operation.
    """
    start: float = timer()
    while True:
        await asyncio.sleep(_LIVENESS_CHECK_SLEEP_TIMER)
        elapsed: int = int(timer() - start)
        _LOGGER.info(
            "[Async Liveness Check]: Still waiting on %s operation to finish (elapsed: %ss)",
            operation,
            elapsed,
        )


# LINT.ThenChange(:liveness_check)
