# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.media.media_using_fc.py."""

import asyncio
import json
import unittest
from unittest import mock

import fidl_fuchsia_media_sessions2 as media_session
import fuchsia_controller_py as fc
from honeydew import affordances_capable, errors
from honeydew.affordances.media import media, media_using_fc
from honeydew.affordances.media.errors import MediaError
from honeydew.transports.ffx import ffx
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)


class FakeSessionControlServer(media_session.SessionControlServer):
    """Fake SessionControlServer for testing."""

    def __init__(
        self,
        channel: fc.Channel,
        status_response: media_session.SessionControlWatchStatusResponse
        | None = None,
        exception: Exception | None = None,
    ):
        super().__init__(channel)
        self.status_response: media_session.SessionControlWatchStatusResponse | None = (
            status_response
        )
        self.exception: Exception | None = exception

    async def watch_status(
        self,
    ) -> media_session.SessionControlWatchStatusResponse:
        if self.exception:
            raise self.exception
        if self.status_response:
            return self.status_response
        return media_session.SessionControlWatchStatusResponse(
            session_info_delta=media_session.SessionInfoDelta(
                player_status=media_session.PlayerStatus(
                    player_state=media_session.PlayerState.ERROR
                )
            )
        )


FakeSessionControlServer.__abstractmethods__ = frozenset()


class FakeActiveSessionServer(media_session.ActiveSessionServer):
    """Fake ActiveSessionServer for testing."""

    def __init__(
        self,
        channel: fc.Channel,
        session_channel: fc.Channel | None = None,
        response: media_session.ActiveSessionWatchActiveSessionResponse
        | None = None,
        exception: Exception | None = None,
    ):
        super().__init__(channel)
        self.session_channel: fc.Channel | None = session_channel
        self.response: media_session.ActiveSessionWatchActiveSessionResponse | None = (
            response
        )
        self.exception: Exception | None = exception

    async def watch_active_session(
        self,
    ) -> media_session.ActiveSessionWatchActiveSessionResponse:
        if self.exception:
            raise self.exception
        if self.response:
            return self.response
        if self.session_channel:
            return media_session.ActiveSessionWatchActiveSessionResponse(
                session=self.session_channel.take()
            )
        return media_session.ActiveSessionWatchActiveSessionResponse(
            session=None
        )


FakeActiveSessionServer.__abstractmethods__ = frozenset()


class MediaFcTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for the media_using_fc.MediaUsingFc class."""

    reboot_affordance_obj: mock.MagicMock = mock.MagicMock()
    fc_transport_obj: mock.MagicMock = mock.MagicMock()
    ffx_transport_obj: mock.MagicMock = mock.MagicMock()
    media_obj: media_using_fc.MediaUsingFc = mock.MagicMock()

    def setUp(self) -> None:
        super().setUp()
        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice
        )
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController
        )

        self.ffx_transport_obj = mock.MagicMock(spec=ffx.FFX)
        self.ffx_transport_obj.run.return_value = json.dumps(
            {"instances": [{"moniker": "core/mediasession"}]}
        )

        self.media_obj = media_using_fc.MediaUsingFc(
            device_name="fuchsia-emulator",
            fuchsia_controller=self.fc_transport_obj,
            ffx_transport=self.ffx_transport_obj,
        )

    def test_verify_supported_success(self) -> None:
        """Test verify_supported success."""
        self.media_obj.verify_supported()
        self.ffx_transport_obj.run.assert_called()

    def test_verify_supported_failure(self) -> None:
        """Test verify_supported failure."""
        self.ffx_transport_obj.run.return_value = json.dumps({"instances": []})
        with self.assertRaises(errors.NotSupportedError):
            self.media_obj.verify_supported()

    async def test_get_active_session_status_playing(self) -> None:
        """Test get_active_session_status returns PLAYING using fake servers."""
        ctx = fc.Context()
        as_client_ch, as_server_ch = ctx.channel_create()
        sc_client_ch, sc_server_ch = ctx.channel_create()

        as_server = FakeActiveSessionServer(
            as_server_ch, session_channel=sc_client_ch
        )
        expected_response = media_session.SessionControlWatchStatusResponse(
            session_info_delta=media_session.SessionInfoDelta(
                player_status=media_session.PlayerStatus(
                    player_state=media_session.PlayerState.PLAYING
                )
            )
        )
        sc_server = FakeSessionControlServer(
            sc_server_ch, status_response=expected_response
        )  # type: ignore[abstract]

        loop = asyncio.get_running_loop()
        as_task = loop.create_task(as_server.serve())
        sc_task = loop.create_task(sc_server.serve())

        self.fc_transport_obj.connect_device_proxy.return_value = as_client_ch

        status = await self.media_obj.get_active_session_status()

        self.assertEqual(status, media.PlayerState.PLAYING)

        as_task.cancel()
        sc_task.cancel()

    async def test_get_active_session_status_no_session(self) -> None:
        """Test get_active_session_status returns None when no session exists."""
        ctx = fc.Context()
        as_client_ch, as_server_ch = ctx.channel_create()

        as_server = FakeActiveSessionServer(as_server_ch)
        loop = asyncio.get_running_loop()
        as_task = loop.create_task(as_server.serve())

        self.fc_transport_obj.connect_device_proxy.return_value = as_client_ch

        status = await self.media_obj.get_active_session_status()

        self.assertIsNone(status)

        as_task.cancel()

    async def test_get_active_session_status_error(self) -> None:
        """Test get_active_session_status raises MediaError on FIDL failure."""
        ctx = fc.Context()
        as_client_ch, as_server_ch = ctx.channel_create()

        as_server = FakeActiveSessionServer(
            as_server_ch, exception=fc.ZxStatus(fc.ZxStatus.ZX_ERR_PEER_CLOSED)
        )
        loop = asyncio.get_running_loop()
        as_task = loop.create_task(as_server.serve())

        self.fc_transport_obj.connect_device_proxy.return_value = as_client_ch

        with self.assertRaises(MediaError):
            await self.media_obj.get_active_session_status()

        as_task.cancel()


if __name__ == "__main__":
    unittest.main()
