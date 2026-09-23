# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data types used by Tracing affordance."""

import enum


class Implementation(enum.StrEnum):
    """Different Tracing affordance implementations available."""

    # Use Tracing affordances that is implemented using Fuchsia-Controller
    FUCHSIA_CONTROLLER = "fuchsia-controller"

    # Use Tracing affordances that is implemented using FFX
    FFX = "ffx"
