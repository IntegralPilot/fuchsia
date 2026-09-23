# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by Fastboot transport."""

from honeydew import errors


class FastbootCommandError(errors.HoneydewError):
    """Exception for errors raised by Fastboot commands."""


class FastbootConnectionError(errors.TransportConnectionError):
    """Raised when Fastboot transport's check_connection fails."""
