# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Screenshot affordance."""

import abc

from honeydew.affordances import affordance
from honeydew.affordances.ui.screenshot import types


class Screenshot(affordance.Affordance):
    """Abstract base class for Screenshot affordance."""

    @abc.abstractmethod
    def take(self) -> types.ScreenshotImage:
        """Take a screenshot.

        Return:
            ScreenshotImage: the screenshot image.
        """
