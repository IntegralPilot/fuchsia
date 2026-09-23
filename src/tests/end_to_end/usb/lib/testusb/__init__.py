# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia USB testusb host testing library."""

from . import backend, cli, ioctl, models, runner
from .backend import USBTestBackend
from .cli import create_parser, main, parse_test_selection
from .ioctl import USBTestIoctlBackend
from .models import (
    ALL_TEST_CASES,
    EP0_GENERIC_TESTS,
    KNOWN_TEST_DEVICES,
    TestCase,
    TestParams,
    TestResult,
    TestStatus,
    UsbDescriptorType,
)
from .runner import (
    JSONReporter,
    TestRunner,
    TextReporter,
    format_text_report,
    generate_json_report,
)

__all__ = [
    "ALL_TEST_CASES",
    "EP0_GENERIC_TESTS",
    "JSONReporter",
    "KNOWN_TEST_DEVICES",
    "TestCase",
    "TestParams",
    "TestResult",
    "TestRunner",
    "TestStatus",
    "TextReporter",
    "USBTestBackend",
    "USBTestIoctlBackend",
    "UsbDescriptorType",
    "backend",
    "cli",
    "create_parser",
    "format_text_report",
    "generate_json_report",
    "ioctl",
    "main",
    "models",
    "parse_test_selection",
    "runner",
]
