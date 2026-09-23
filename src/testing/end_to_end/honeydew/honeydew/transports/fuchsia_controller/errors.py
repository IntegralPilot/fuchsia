# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by Fuchsia-Controller transport."""

from honeydew import errors


class FuchsiaControllerError(errors.TransportError):
    """Exception for errors raised by Fuchsia Controller requests."""


class FuchsiaControllerConnectionError(errors.TransportConnectionError):
    """Raised when Fuchsia-Controller transport's check_connection fails."""
