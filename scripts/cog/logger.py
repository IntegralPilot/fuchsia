# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import contextlib
import functools
import logging
import os
import shlex
import subprocess
import sys
import tempfile
from logging.handlers import MemoryHandler
from pathlib import Path
from typing import Any, Iterator, Optional

_logger: Optional[logging.Logger] = None
_stream_handler: Optional[logging.StreamHandler[Any]] = None
_file_handler: Optional[logging.FileHandler] = None
_memory_handler: Optional[MemoryHandler] = None
_enable_status_updates: bool = False
_console_log_level: int = logging.WARNING


_EXCEPTION = logging.CRITICAL + 1


class ColoredFormatter(logging.Formatter):
    """A logging formatter that adds colors to the output."""

    COLORS = {
        "DEBUG": "\033[94m",  # Blue
        "INFO": "\033[92m",  # Green
        "WARNING": "\033[93m",  # Yellow
        "ERROR": "\033[91m",  # Red
        "CRITICAL": "\033[1;91m",  # Bold Red
    }
    RESET = "\033[0m"

    def __init__(self, fmt: str) -> None:
        super().__init__(fmt)

    def format(self, record: logging.LogRecord) -> str:
        log_message = super().format(record)
        # Don't colorize exception messages.
        if record.exc_info:
            return log_message
        color = self.COLORS.get(record.levelname)
        if color:
            return f"{color}{log_message}{self.RESET}"
        return log_message


def add_args(
    parser: argparse.ArgumentParser,
    default_log_level: int = logging.WARNING,
) -> None:
    """
    Adds arguments to the parser to control logging behavior.

    Args:
        parser: The parser to add arguments to.
        default_log_level: The default log level to use.
    """
    parser.set_defaults(log_level=default_log_level)

    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "-v",
        "--verbose",
        action="store_const",
        const=logging.DEBUG,
        dest="log_level",
        help="Increase verbosity level to DEBUG.",
    )
    group.add_argument(
        "-i",
        "--info",
        action="store_const",
        const=logging.INFO,
        dest="log_level",
        help="Set verbosity level to INFO.",
    )
    group.add_argument(
        "-q",
        "--quiet",
        action="store_const",
        const=logging.WARNING,
        dest="log_level",
        help="Suppress non-critical output (INFO and below).",
    )
    parser.add_argument(
        "--color",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Enable or disable color output.",
    )
    parser.add_argument(
        "--enable-status-updates",
        action="store_true",
        help="Enable status updates.",
    )


def _create_formatter(colors: bool = False) -> logging.Formatter:
    """Creates a formatter for logging."""
    fmt = "%(asctime)s - %(levelname)s: [%(filename)s:%(lineno)d] %(message)s"

    if colors:
        return ColoredFormatter(fmt)
    else:
        return logging.Formatter(fmt)


def init_logger(
    log_level: int = logging.WARNING,
    colors: bool = False,
    enable_status_updates: bool = False,
) -> None:
    """
    Initializes the global logger.

    Args:
        log_level: The minimum log level to display.
        colors: Whether to use colored output.
        enable_status_updates: Whether to enable status updates.
    """
    global _logger
    global _stream_handler
    global _enable_status_updates
    global _console_log_level
    global _memory_handler

    _enable_status_updates = enable_status_updates
    _console_log_level = log_level
    logger = logging.getLogger("cog")

    # Clear any existing StreamHandler. This can happen if log() is called before
    # init_logger(), which may cause duplicate output if we do not clear
    # the old logger handler.
    if _stream_handler is not None:
        logger.removeHandler(_stream_handler)

    logger.setLevel(logging.DEBUG)
    handler = logging.StreamHandler(sys.stderr)
    handler.setLevel(log_level)

    formatter = _create_formatter(colors=colors)

    handler.setFormatter(formatter)
    logger.addHandler(handler)

    # `_memory_handler` lets us retroactively dump logs that were recorded before
    # `logger.setup_file_logging` is called.
    if _file_handler is None and _memory_handler is None:
        _memory_handler = MemoryHandler(
            capacity=10000, flushLevel=logging.CRITICAL + 1
        )
        _memory_handler.setLevel(logging.DEBUG)
        logger.addHandler(_memory_handler)

    _stream_handler = handler
    _logger = logger


def setup_file_logging(workspace_root: Path) -> None:
    """Enables writing logs to a file.

    Args:
        workspace_root: The root directory of the workspace.
    """
    global _logger
    global _file_handler
    global _memory_handler
    if _logger is None:
        init_logger()

    assert _logger

    # Check if file handler already exists to avoid duplicates
    if _file_handler is not None:
        return

    try:
        log_file = workspace_root / "workspace_setup.log"
        file_handler = logging.FileHandler(log_file)
        file_handler.setLevel(logging.DEBUG)
        file_formatter = _create_formatter(colors=False)
        file_handler.setFormatter(file_formatter)

        # Dump all of the logs that have been emitted so far.
        if _memory_handler:
            _memory_handler.setTarget(file_handler)
            _logger.removeHandler(_memory_handler)
            _memory_handler.close()
            _memory_handler = None

        _logger.addHandler(file_handler)
        _file_handler = file_handler
    except Exception as e:
        _logger.warning(f"Failed to initialize file logging: {e}")


def get_log_path() -> Path:
    """Returns a path to the log file.

    Returns:
        The relative path of the log file to CWD if `setup_file_logging` has been called.
        Otherwise, dumps the logs so far into a temporary file and returns that absolute path.
    """
    if _file_handler:
        abs_path = Path(_file_handler.baseFilename)
        return Path(os.path.relpath(abs_path))

    try:
        fd, temp_path = tempfile.mkstemp(suffix=".log", prefix="cog_")
        if _memory_handler:
            formatter = _create_formatter(colors=False)
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                for record in _memory_handler.buffer:
                    f.write(formatter.format(record) + "\n")
        else:
            os.close(fd)
        return Path(temp_path)
    except OSError as e:
        print(f"Failed to create temporary log file: {e}")
        return Path("<unknown>")


def get_log_level() -> int:
    """Returns the current log level."""
    return _console_log_level


@contextlib.contextmanager
def set_level(level: int) -> Iterator[None]:
    """Context manager to temporarily set the log level."""
    global _console_log_level
    global _logger
    global _stream_handler
    if _logger is None:
        init_logger(logging.WARNING)

    assert _logger
    old_level = _console_log_level
    _console_log_level = level

    old_stream_level = None
    if _stream_handler:
        old_stream_level = _stream_handler.level
        _stream_handler.setLevel(level)

    try:
        yield
    finally:
        _console_log_level = old_level
        if _stream_handler and old_stream_level is not None:
            _stream_handler.setLevel(old_stream_level)


def log(level: int, *args: Any, **kwargs: Any) -> None:
    """
    Logs a message with the given level.

    This function has an API similar to the print function.

    Args:
        level: The log level (e.g., logging.INFO, 'INFO', 'DEBUG').
        *args: The message parts to log.
        **kwargs: Supports 'sep' to specify a separator.

    Raises:
        Exception: If the logger has not been initialized.
        ValueError: If the log level is invalid.
    """
    if _logger is None:
        init_logger(logging.WARNING)

    # Needed to make mypy happy
    assert _logger

    # The stacklevel keyword argument allows wrappers to adjust which stack frame
    # is reported as the source of the log message. We default to 1, which refers
    # to the caller of this function. We add 1 to this value later to account
    # for this wrapper function.
    stacklevel = kwargs.pop("stacklevel", 1)

    sep = kwargs.get("sep", " ")
    message = sep.join(map(str, args))

    if level == _EXCEPTION:
        _logger.exception(message, stacklevel=stacklevel + 1)
        return

    _logger.log(level, message, stacklevel=stacklevel + 1)


def emit_status(message: str) -> None:
    """
    Emits a status update to stdout.

    Args:
        message: The status message to emit.
    """
    if not _enable_status_updates:
        return

    # Status updates are printed to stdout with a special prefix so that they can
    # be identified by the IDEs.
    print(f"STATUS_UPDATE:{message}")


def _add_stacklevel(func: Any) -> Any:
    """Decorator to increase the stacklevel for log helper functions."""

    @functools.wraps(func)
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        # Increase stacklevel so that the log message is attributed to the caller
        # of the helper function, rather than the helper itself. We add 2 to
        # account for this wrapper and the decorated log helper.
        kwargs["stacklevel"] = kwargs.get("stacklevel", 1) + 2
        return func(*args, **kwargs)

    return wrapper


@_add_stacklevel
def log_info(*args: Any, **kwargs: Any) -> None:
    """
    Logs a message with level INFO.

    This function has an API similar to the print function.

    Args:
        *args: The message parts to log.
        **kwargs: Supports 'sep' to specify a separator.
    """
    log(logging.INFO, *args, **kwargs)


@_add_stacklevel
def log_debug(*args: Any, **kwargs: Any) -> None:
    """
    Logs a message with level DEBUG.

    This function has an API similar to the print function.

    Args:
        *args: The message parts to log.
        **kwargs: Supports 'sep' to specify a separator.
    """
    log(logging.DEBUG, *args, **kwargs)


@_add_stacklevel
def log_warn(*args: Any, **kwargs: Any) -> None:
    """
    Logs a message with level WARNING.

    This function has an API similar to the print function.

    Args:
        *args: The message parts to log.
        **kwargs: Supports 'sep' to specify a separator.
    """
    log(logging.WARNING, *args, **kwargs)


@_add_stacklevel
def log_error(*args: Any, **kwargs: Any) -> None:
    """
    Logs a message with level ERROR.

    This function has an API similar to the print function.

    Args:
        *args: The message parts to log.
        **kwargs: Supports 'sep' to specify a separator.
    """
    log(logging.ERROR, *args, **kwargs)


@_add_stacklevel
def log_critical(*args: Any, **kwargs: Any) -> None:
    """
    Logs a message with level CRITICAL.

    This function has an API similar to the print function.

    Args:
        *args: The message parts to log.
        **kwargs: Supports 'sep' to specify a separator.
    """
    log(logging.CRITICAL, *args, **kwargs)


@_add_stacklevel
def log_exception(*args: Any, **kwargs: Any) -> None:
    """
    Logs a message with level ERROR, including exception information.

    A stacktrace will be included if the log level is set to DEBUG, otherwise
    only the error message will be logged.

    If the current exception is a subprocess.CalledProcessError, the command,
    return code, and any captured stdout/stderr will also be logged.

    This function has an API similar to the print function and should be called
    from within an exception handler.

    Args:
        *args: The message parts to log.
        **kwargs: Supports 'sep' to specify a separator.
    """
    exc_type, exc_value, exc_traceback = sys.exc_info()

    if isinstance(exc_value, subprocess.CalledProcessError):
        e = exc_value
        if get_log_level() <= logging.DEBUG:
            log(_EXCEPTION, *args, **kwargs)
        else:
            log_error(*args, **kwargs)

        cmd_str = (
            shlex.join(map(str, e.cmd))
            if isinstance(e.cmd, (list, tuple))
            else str(e.cmd)
        )
        log(
            logging.ERROR,
            f"Command `{cmd_str}` exited with status {e.returncode}",
            stacklevel=kwargs.get("stacklevel", 1),
        )

        if e.stdout:
            stdout_str = (
                e.stdout.decode("utf-8", errors="replace")
                if isinstance(e.stdout, bytes)
                else e.stdout
            )
            log(
                logging.ERROR,
                f"stdout: {stdout_str}",
                stacklevel=kwargs.get("stacklevel", 1),
            )
        if e.stderr:
            stderr_str = (
                e.stderr.decode("utf-8", errors="replace")
                if isinstance(e.stderr, bytes)
                else e.stderr
            )
            log(
                logging.ERROR,
                f"stderr: {stderr_str}",
                stacklevel=kwargs.get("stacklevel", 1),
            )
    else:
        if get_log_level() <= logging.DEBUG:
            log(_EXCEPTION, *args, **kwargs)
        else:
            log_error(*args, **kwargs)
            if exc_value:
                log_error(str(exc_value), **kwargs)
