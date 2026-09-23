#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
class EventSubscription(object):
    """A class that defines the way a function is subscribed to an event.

    Attributes:
        event_type: The type of the event.
        _func: The subscribed function.
        _event_filter: A lambda that returns True if an event should be passed
                       to the subscribed function.
        order: The order value in which this subscription should be called.
    """

    def __init__(self, event_type, func, event_filter=None, order=0):
        self._event_type = event_type
        self._func = func
        self._event_filter = event_filter
        self.order = order

    @property
    def event_type(self):
        return self._event_type

    def deliver(self, event):
        """Delivers an event to the subscriber.

        This function will not deliver the event if the event filter rejects the
        event.

        Args:
            event: The event to send to the subscriber.
        """
        if self._event_filter and not self._event_filter(event):
            return
        self._func(event)
