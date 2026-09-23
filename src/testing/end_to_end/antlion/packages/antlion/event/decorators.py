#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from antlion.event.subscription_handle import StaticSubscriptionHandle


def subscribe_static(event_type, event_filter=None, order=0):
    """A decorator that subscribes a static or module-level function.

    This function must be registered manually.
    """

    class InnerSubscriptionHandle(StaticSubscriptionHandle):
        def __init__(self, func):
            super().__init__(
                event_type, func, event_filter=event_filter, order=order
            )

    return InnerSubscriptionHandle
