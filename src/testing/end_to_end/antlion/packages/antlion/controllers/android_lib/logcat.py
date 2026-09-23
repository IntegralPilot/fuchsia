#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import logging
import re

from antlion.libs.logging import log_stream
from antlion.libs.logging.log_stream import LogStyles
from antlion.libs.proc.process import Process

TIMESTAMP_REGEX = r"((?:\d+-)?\d+-\d+ \d+:\d+:\d+.\d+)"


class TimestampTracker(object):
    """Stores the last timestamp outputted by the Logcat process."""

    def __init__(self):
        self._last_timestamp = None

    @property
    def last_timestamp(self):
        return self._last_timestamp

    def read_output(self, message):
        """Reads the message and parses all timestamps from it."""
        all_timestamps = re.findall(TIMESTAMP_REGEX, message)
        if len(all_timestamps) > 0:
            self._last_timestamp = all_timestamps[0]


def _get_log_level(message):
    """Returns the log level for the given message."""
    if message.startswith("-") or len(message) < 37:
        return logging.ERROR
    else:
        log_level = message[36]
        if log_level in ("V", "D"):
            return logging.DEBUG
        elif log_level == "I":
            return logging.INFO
        elif log_level == "W":
            return logging.WARNING
        elif log_level == "E":
            return logging.ERROR
    return logging.NOTSET


def _log_line_func(log, timestamp_tracker):
    """Returns a lambda that logs a message to the given logger."""

    def log_line(message):
        timestamp_tracker.read_output(message)
        log.log(_get_log_level(message), message)

    return log_line


def _on_retry(serial, extra_params, timestamp_tracker):
    def on_retry(_):
        begin_at = '"%s"' % (timestamp_tracker.last_timestamp or 1)
        additional_params = extra_params or ""

        return (
            f"adb -s {serial} logcat -T {begin_at} -v year {additional_params}"
        )

    return on_retry


def create_logcat_keepalive_process(serial, logcat_dir, extra_params=""):
    """Creates a Logcat Process that automatically attempts to reconnect.

    Args:
        serial: The serial of the device to read the logcat of.
        logcat_dir: The directory used for logcat file output.
        extra_params: Any additional params to be added to the logcat cmdline.

    Returns:
        A acts.libs.proc.process.Process object.
    """
    logger = log_stream.create_logger(
        f"adblog_{serial}",
        log_name=serial,
        subcontext=logcat_dir,
        log_styles=(LogStyles.LOG_DEBUG | LogStyles.TESTCASE_LOG),
    )
    process = Process(f"adb -s {serial} logcat -T 1 -v year {extra_params}")
    timestamp_tracker = TimestampTracker()
    process.set_on_output_callback(_log_line_func(logger, timestamp_tracker))
    process.set_on_terminate_callback(
        _on_retry(serial, extra_params, timestamp_tracker)
    )
    return process
