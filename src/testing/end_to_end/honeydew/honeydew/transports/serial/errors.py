# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by Serial transport."""

from honeydew import errors


class SerialError(errors.TransportError):
    """Exception for errors raised by host-target communication over serial."""
