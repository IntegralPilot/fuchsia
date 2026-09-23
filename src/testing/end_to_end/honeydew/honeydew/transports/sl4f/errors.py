# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by SL4F transport."""

from honeydew import errors


class Sl4fError(errors.TransportError):
    """Exception for errors raised by SL4F requests."""


class Sl4fTimeoutError(Sl4fError, TimeoutError):
    """Exception for errors raised by SL4F request timeouts."""


class Sl4fConnectionError(errors.TransportConnectionError):
    """Raised when SL4F transport's check_connection fails."""
