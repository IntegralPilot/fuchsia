#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="type-arg"
import logging
from types import TracebackType


class LogLevel:
    """Sets the logging level threshold for logger within this context.

    Logging messages which are equal or less severe than level will be ignored.
    See https://docs.python.org/3/library/logging.html#levels for a list of
    levels.
    """

    def __init__(
        self, logger: logging.Logger | logging.LoggerAdapter, level: int
    ) -> None:
        self._logger = logger
        if isinstance(logger, logging.Logger):
            self._old_level = logger.level
        else:
            self._old_level = logger.logger.level
        self._new_level = level

    def __enter__(self) -> logging.Logger | logging.LoggerAdapter:
        self._logger.setLevel(self._new_level)
        return self._logger

    def __exit__(
        self,
        _exit_type: type[BaseException] | None,
        _exit_value: BaseException | None,
        _exit_traceback: TracebackType | None,
    ) -> None:
        self._logger.setLevel(self._old_level)
