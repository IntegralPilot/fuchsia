# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by ADB transport."""

from honeydew import errors


class AdbError(errors.HoneydewError):
    """Base exception for ADB transport."""


class InitializationError(AdbError):
    """Exception for ADB transport initialization failures."""


class AdbCommandError(AdbError):
    """Exception for errors raised by ADB commands."""


class AdbTimeoutError(AdbError, TimeoutError):
    """Exception for ADB commands timing out."""


class AdbServerError(AdbError):
    """Exception for ADB server errors."""
