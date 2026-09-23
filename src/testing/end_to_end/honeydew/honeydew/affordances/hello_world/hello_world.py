# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for HelloWorld affordance."""

import abc

from honeydew.affordances import affordance


class HelloWorld(affordance.Affordance):
    """Abstract base class for HelloWorld affordance."""

    @abc.abstractmethod
    def greeting(self, name: str | None = None) -> str:
        """Returns a string that contains greeting's message.

        Returns:
            greeting's message str

        Raises:
            HelloWorldError: Failed to construct greeting's message.
        """
