# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Session affordance."""

import abc

from honeydew.affordances import affordance


class Session(affordance.Affordance):
    """Abstract base class for Session affordance."""

    @abc.abstractmethod
    def start(self) -> None:
        """Start session. Wait indefinitely until session started.

        Raises:
            honeydew.errors.SessionError: session failed to start.
        """

    @abc.abstractmethod
    def is_started(self) -> bool:
        """Check if session is started.

        Returns:
            True if session is started.

        Raises:
            honeydew.errors.SessionError: failed to check the session state.
        """

    @abc.abstractmethod
    def ensure_started(self) -> None:
        """Ensure session started, if not start a new session. Wait indefinitely until session
        started.

        Raises:
            honeydew.errors.SessionError: session failed to check or start.
        """

    @abc.abstractmethod
    def add_component(self, url: str) -> None:
        """Instantiates a component by its URL and adds to the session.

        Args:
            url: url of the component

        Raises:
            honeydew.errors.SessionError: Session failed to launch component
                with given url. Session is not started.
        """

    @abc.abstractmethod
    def restart(self) -> None:
        """Restarts the session. Wait indefinitely until session started.

        Raises:
            honeydew.errors.SessionError: Session failed to restart the session.
        """

    @abc.abstractmethod
    def stop(self) -> None:
        """Stop the session. Wait indefinitely until session stopped.

        Raises:
            honeydew.errors.SessionError: Session failed to stop the session.
        """

    @abc.abstractmethod
    def cleanup(self) -> None:
        """Cleanup the session using `ffx component list` and `ffx session remove`.

        This method has been registered in device.close, no need to call explicitly in test
        teardown.

        Raises:
            honeydew.errors.SessionError: Session failed to list running components or remove
            components.
        """
