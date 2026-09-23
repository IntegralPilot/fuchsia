# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import unittest

from fuchsia_controller_py import (
    Channel,
    Context,
    FcTransportStatus,
    Handle,
    Socket,
)

from fidl import AsyncChannel


class ChannelTests(unittest.IsolatedAsyncioTestCase):
    """Channel tests."""

    def test_event_access_denied(self) -> None:
        ctx = Context()
        e = ctx.event_create()
        with self.assertRaises(FcTransportStatus):
            try:
                # Attempt USER_0 signal.
                e.signal_peer(0, 1 << 24)
            except FcTransportStatus as e:
                self.assertEqual(e.code(), FcTransportStatus.FC_ERR_FDOMAIN)
                raise e

    def test_eventpair_peer_closed(self) -> None:
        ctx = Context()
        e1, e2 = ctx.event_create_pair()
        del e2
        with self.assertRaises(FcTransportStatus):
            try:
                # Attempt USER_0 signal.
                e1.signal_peer(0, 1 << 24)
            except FcTransportStatus as e:
                self.assertEqual(e.code(), FcTransportStatus.FC_ERR_FDOMAIN)
                raise e

    def test_as_int(self) -> None:
        self.assertEqual(Handle(1).as_int(), 1)
        self.assertEqual(Channel(2).as_int(), 2)
        self.assertEqual(Socket(3).as_int(), 3)

    async def test_channel_write_then_read(self) -> None:
        ctx = Context()
        (a, b) = ctx.channel_create()
        a.write((bytearray([1, 2, 3]), []))
        async_b = AsyncChannel(b)
        buf, hdls = await async_b.read()
        self.assertEqual(buf, bytearray([1, 2, 3]))

    def test_channel_write_fails_when_closed(self) -> None:
        ctx = Context()
        (a, b) = ctx.channel_create()
        del b
        with self.assertRaises(FcTransportStatus):
            try:
                a.write((bytearray([1, 2, 3]), []))
            except FcTransportStatus as e:
                self.assertEqual(
                    e.args[0], FcTransportStatus.FC_ERR_CHANNEL_WRITE
                )
                raise e

    async def test_channel_passing(self) -> None:
        ctx = Context()
        (a, b) = ctx.channel_create()
        (c, d) = ctx.channel_create()
        # This is using 'take' rather than 'as_int' as using 'as_int' would cause a double-close
        # error on a channel that has already been closed.
        a.write((bytearray(), [(0, c.take(), 0, 0, 0)]))
        async_b = AsyncChannel(b)
        _, hdls = await async_b.read()
        self.assertEqual(len(hdls), 1)
        new_c = Channel(hdls[0])
        new_c.write((bytearray([1, 2, 3]), []))
        async_d = AsyncChannel(d)
        buf, d_hdls = await async_d.read()
        self.assertEqual(buf, bytearray([1, 2, 3]))
