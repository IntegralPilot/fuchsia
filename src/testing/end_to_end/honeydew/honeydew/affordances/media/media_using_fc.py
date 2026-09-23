# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Media affordance implementation using FuchsiaController."""

import json
import logging

import fidl_fuchsia_media_sessions2 as media_session
import fuchsia_controller_py as fc
from honeydew import errors
from honeydew.affordances.media import media
from honeydew.affordances.media.errors import MediaError
from honeydew.transports.ffx import ffx
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing import custom_types

_ACTIVE_SESSION_ENDPOINT: custom_types.FidlEndpoint = custom_types.FidlEndpoint(
    "/core/mediasession", "fuchsia.media.sessions2.ActiveSession"
)

_MEDIA_SESSION_COMPONENT: str = "core/mediasession"

_LOGGER: logging.Logger = logging.getLogger(__name__)


class MediaUsingFc(media.Media):
    """Media affordance implementation using FuchsiaController."""

    def __init__(
        self,
        device_name: str,
        fuchsia_controller: fc_transport.FuchsiaController,
        ffx_transport: ffx.FFX,
    ) -> None:
        self._name: str = device_name
        self._fc_transport: fc_transport.FuchsiaController = fuchsia_controller
        self._ffx_transport: ffx.FFX = ffx_transport

        self.verify_supported()

    def verify_supported(self) -> None:
        """Verifies that affordance implementation is supported by the Fuchsia device.

        Raises:
            NotSupportedError: If affordance is not supported.
        """
        output = self._ffx_transport.run(
            ["--machine", "json", "component", "list"]
        )
        component_list = json.loads(output)
        instances = component_list.get("instances", [])

        if not any(
            instance.get("moniker") == _MEDIA_SESSION_COMPONENT
            for instance in instances
        ):
            raise errors.NotSupportedError(
                f"{_MEDIA_SESSION_COMPONENT} is not available in device {self._name}"
            )

    def _connect_active_session_proxy(
        self,
    ) -> media_session.ActiveSessionClient:
        """Returns the ActiveSession proxy."""
        return media_session.ActiveSessionClient(
            self._fc_transport.connect_device_proxy(_ACTIVE_SESSION_ENDPOINT)
        )

    async def get_active_session_status(self) -> media.PlayerState | None:
        """Returns the status of the active media session.

        Returns:
            The player state of the active media session if one exists,
            None otherwise.

        Raises:
            MediaError: On FIDL communication failure.
        """
        active_session_proxy = self._connect_active_session_proxy()
        try:
            # WatchActiveSession is a hanging get.
            # The first call returns immediately with the current active session.
            response = await active_session_proxy.watch_active_session()
            session_client_end = response.session

            if session_client_end is None:
                return None

            session_proxy = media_session.SessionControlClient(
                session_client_end
            )
            status_response = await session_proxy.watch_status()
            player_status = status_response.session_info_delta.player_status

            if player_status is None or player_status.player_state is None:
                return None

            fidl_state = player_status.player_state
            if fidl_state == media_session.PlayerState.IDLE:
                return media.PlayerState.IDLE
            if fidl_state == media_session.PlayerState.PLAYING:
                return media.PlayerState.PLAYING
            if fidl_state == media_session.PlayerState.PAUSED:
                return media.PlayerState.PAUSED
            if fidl_state == media_session.PlayerState.BUFFERING:
                return media.PlayerState.BUFFERING
            if fidl_state == media_session.PlayerState.ERROR:
                return media.PlayerState.ERROR

            return None

        except fc.ZxStatus as status:
            raise MediaError(
                f"FIDL Error while watching active session status: {status}"
            ) from status
        except Exception as e:
            raise MediaError(
                f"Unexpected error while watching active session status: {e}"
            ) from e
