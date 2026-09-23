# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Session affordance implementation using ffx."""

import logging
import time

from honeydew import affordances_capable
from honeydew.affordances.session import errors as session_errors
from honeydew.affordances.session import session
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types

_LOGGER = logging.getLogger(__name__)

ELEMENT_PREFIX = "core/session-manager/session:session/elements:"


class SessionUsingFfx(session.Session):
    """Session affordance implementation using ffx.

    Args:
        device_name: Device name returned by `ffx target list`.
        ffx: ffx_transport.FFX.
    """

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_device_close: affordances_capable.FuchsiaDeviceClose,
    ) -> None:
        self._name: str = device_name
        self._ffx: ffx_transport.FFX = ffx
        self._fuchsia_device_close = fuchsia_device_close
        self._fuchsia_device_close.register_for_on_device_close(self.cleanup)

        self.verify_supported()

    def verify_supported(self) -> None:
        """Check if Session is supported on the DUT.
        Raises:
            NotSupportedError: Session affordance is not supported by Fuchsia device.
        """

    def start(self) -> None:
        """Start session.

        `ffx session start` will start a new session when there is a started session.

        Raises:
            SessionError: session failed to start.
        """

        _LOGGER.info("start session on device %s", self._name)

        try:
            self._ffx.run(
                ["session", "start"], machine=ffx_types.MachineFormat.RAW
            )
        except ffx_errors.FfxCommandError as err:
            raise session_errors.SessionError(err)

        _LOGGER.info("wait for session started on device %s", self._name)
        # wait for session started.
        while not self.is_started():
            time.sleep(1)  # second.

        _LOGGER.info("session started on device %s", self._name)

    def is_started(self) -> bool:
        """Check if session is started.

        Returns:
            True if session is started.

        Raises:
            SessionError: failed to check the session state.
        """

        try:
            res = self._ffx.run(
                ["session", "show"], machine=ffx_types.MachineFormat.RAW
            )
            lines = res.splitlines()
            for line in lines:
                if "Execution State:  Running" in line:
                    return True
        except ffx_errors.FfxCommandError as err:
            if (
                'No matching component instance found for query "core/session-manager/session:session"'
                in str(err)
            ) or (
                # ffx may return an internal command error during the initial
                # connection, in the case, we treat the session as not started
                # and retry the check later.
                # LINT.IfChange
                "BUG: An internal command error occurred."
                # LINT.ThenChange(//src/developer/ffx/command/error/src/error.rs)
                in str(err)
            ):
                # session is not running.
                return False

            raise session_errors.SessionError(err)

        return False

    def ensure_started(self) -> None:
        """Ensure session started, if not start a session. Wait indefinitely until session
        started.

        Raises:
            honeydew.errors.SessionError: session failed to check or start.
        """

        if not self.is_started():
            _LOGGER.info("session not started on device %s", self._name)
            self.start()

    def add_component(self, url: str) -> None:
        """Instantiates a component by its URL and adds to the session.

        Args:
            url: url of the component

        Raises:
            SessionError: Session failed to launch component with given url. Session is not started.
        """
        if not self.is_started():
            raise session_errors.SessionError("session is not started.")

        try:
            self._ffx.run(
                ["session", "add", url], machine=ffx_types.MachineFormat.RAW
            )
        except ffx_errors.FfxCommandError as err:
            raise session_errors.SessionError(err)

    def restart(self) -> None:
        """Restart session.

        `ffx session restart` is only allow to call when session is started.

        Raises:
            honeydew.errors.SessionError: session failed to restart.
        """

        if not self.is_started():
            raise session_errors.SessionError("session is not started.")

        _LOGGER.info("restart session on device %s", self._name)

        try:
            self._ffx.run(
                ["session", "restart"], machine=ffx_types.MachineFormat.RAW
            )
        except ffx_errors.FfxCommandError as err:
            raise session_errors.SessionError(err)

        _LOGGER.info("wait for session started on device %s", self._name)
        # wait for session started.
        while not self.is_started():
            time.sleep(1)  # second.

        _LOGGER.info("session started on device %s", self._name)

    def stop(self) -> None:
        """Stop the session.

        It is ok to call `ffx session stop` regardless of whether a session
        has started or not.

        Raises:
            SessionError: Session failed stop to the session.
        """

        _LOGGER.info("stop session on device %s", self._name)

        try:
            self._ffx.run(
                ["session", "stop"], machine=ffx_types.MachineFormat.RAW
            )
        except ffx_errors.FfxCommandError as err:
            raise session_errors.SessionError(err)

        _LOGGER.info("wait for session stopped on device %s", self._name)
        # wait for session stopped.
        while self.is_started():
            time.sleep(1)  # second.

        _LOGGER.info("session stopped on device %s", self._name)

    def cleanup(self) -> None:
        """Cleanup the session using `ffx component list` and `ffx session remove`.

        Raises:
            SessionError: Session failed to list running components or remove components.
        """

        # when session is stopped, no cleanup.
        if not self.is_started():
            _LOGGER.info(
                "no cleanup needed on device %s, session is stopped.",
                self._name,
            )
            return

        _LOGGER.info("cleanup session on device %s", self._name)

        try:
            # "ffx component list" supports "--machine json", but the "raw" output
            # is simply the list of components, which is all that is necessary here.
            # On the other hand, the json form has a schema which means there is
            # a migration path if the output changes, which is not guaranteed for
            # raw output. So it should really be using JSON. TODO(b/484355868)
            res = self._ffx.run(
                ["component", "list"], machine=ffx_types.MachineFormat.RAW
            )
            components = res.splitlines()
            for component in components:
                if component.startswith(ELEMENT_PREFIX):
                    name = component[len(ELEMENT_PREFIX) :]
                    # Starnix's element naming starts with main. Not sure how to remove them yet,
                    # `ffx session remove` seems not work.
                    if not name.startswith("main"):
                        _LOGGER.info(
                            "remove component on device %s: %s",
                            self._name,
                            name,
                        )
                        self._ffx.run(
                            ["session", "remove", name],
                            machine=ffx_types.MachineFormat.RAW,
                        )
        except ffx_errors.FfxCommandError as err:
            # TODO(b/406501041): Handle potential "NotFound" error. If multiple components share the
            # same URL, removing one will implicitly remove all others with that URL. Subsequent
            # attempts to remove components with the same URL will result in a "NotFound" error.
            if "Error: NotFound" not in str(
                err
            ) and "Element not found" not in str(err):
                raise session_errors.SessionError(err)
