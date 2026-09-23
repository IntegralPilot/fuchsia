# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by FFX transport."""

from honeydew import errors


class FfxTimeoutError(errors.HoneydewTimeoutError):
    """Exception for timeout based errors raised by ffx commands running on host machine."""


class FfxConnectionError(errors.TransportConnectionError):
    """Raised when FFX transport's check_connection fails."""


class FfxCommandError(errors.TransportError):
    """Exception for errors raised by ffx commands running on host machine."""


class FfxMonitorNotSupportedError(errors.HoneydewError):
    """If this method is called when monitor is not in use."""


class FfxMonitorRequiresNameError(errors.HoneydewError):
    """Raised when ffx device name is not provided."""


class FfxTargetStatusError(errors.HostCmdError):
    """Exception to be raised for ffx target status command failures."""
