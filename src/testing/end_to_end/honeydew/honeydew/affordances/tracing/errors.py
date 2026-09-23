# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by tracing affordance."""

from honeydew import errors


class TracingError(errors.AffordanceError):
    """Raised by tracing affordance."""


class TracingStateError(TracingError):
    """Raised by tracing affordance when in an unexpected state."""
