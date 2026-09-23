#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from antlion import error


class AndroidDeviceConfigError(Exception):
    """Raised when AndroidDevice configs are malformatted."""


class AndroidDeviceError(error.ActsError):
    """Raised when there is an error in AndroidDevice."""
