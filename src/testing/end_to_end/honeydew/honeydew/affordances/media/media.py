# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Media affordance."""

import abc
import enum

from honeydew.affordances import affordance


class PlayerState(enum.StrEnum):
    """The possible player state values."""

    IDLE = "Idle"
    PLAYING = "Playing"
    PAUSED = "Paused"
    BUFFERING = "Buffering"
    ERROR = "Error"


class Media(affordance.Affordance):
    """Abstract base class for Media affordance."""

    @abc.abstractmethod
    async def get_active_session_status(self) -> PlayerState | None:
        """Returns the status of the active media session.

        Returns:
            The player state of the active media session if one exists,
            None otherwise.

        Raises:
            MediaError: On FIDL communication failure.
        """
