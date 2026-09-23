# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by UserInput affordance."""

from honeydew import errors


class UserInputError(errors.AffordanceError):
    """Exception to be raised by UserInput"""
