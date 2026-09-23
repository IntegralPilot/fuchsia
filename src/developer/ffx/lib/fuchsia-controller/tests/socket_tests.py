# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import unittest

from fuchsia_controller_py import Context, FcTransportStatus, Socket

from fidl import AlreadyReadingAll, AsyncChannel, AsyncSocket


class SocketTests(unittest.IsolatedAsyncioTestCase):
    """Socket tests."""

    async def test_socket_write_then_read(self) -> None:
        """Tests a simple write followed by an immediate read."""
        ctx = Context()
        (sock_out, sock_in) = ctx.socket_create()
        bytes_in = bytearray([1, 2, 3])
        sock_out.write(bytes_in)
        async_sock_in = AsyncSocket(sock_in)
        bytes_out = await async_sock_in.read()
        self.assertEqual(bytes_out, bytes_in)

    def test_socket_write_fails_when_closed(self) -> None:
        """Verifies FC_ERR_SOCKET_WRITE is surfaced when opposing socket is closed."""
        ctx = Context()
        (sock_out, sock_in) = ctx.socket_create()
        del sock_in
        with self.assertRaises(FcTransportStatus):
            try:
                sock_out.write(bytearray([1, 2, 3]))
            except FcTransportStatus as e:
                self.assertEqual(
                    e.code(), FcTransportStatus.FC_ERR_SOCKET_WRITE
                )
                raise e

    async def test_socket_passing(self) -> None:
        """Verifies a socket remains connected when passed through a channel."""
        ctx = Context()
        (chan_out, chan_in) = ctx.channel_create()
        (sock_out, sock_in) = ctx.socket_create()
        # This is using 'take' rather than 'as_int' as using 'as_int' would cause a double-close
        # error on a channel that has already been closed.
        chan_out.write((bytearray(), [(0, sock_out.take(), 0, 0, 0)]))
        async_chan_in = AsyncChannel(chan_in)
        _, hdls = await async_chan_in.read()
        self.assertEqual(len(hdls), 1)
        bytes_in = bytearray([1, 2, 3])
        new_sock_out = Socket(hdls[0])
        new_sock_out.write(bytes_in)
        async_sock_in = AsyncSocket(sock_in)
        bytes_out = await async_sock_in.read()
        self.assertEqual(bytes_out, bytes_in)

    async def test_async_socket(self) -> None:
        """Verifies an async socket is able to wait for the other end to complete writing."""
        ctx = Context()
        (sock_out, sock_in) = ctx.socket_create()
        async_sock_in = AsyncSocket(sock_in)
        bytes_in = bytearray([1, 2, 3])

        async def slow_write(sock: Socket, b: bytes) -> None:
            await asyncio.sleep(1)
            sock.write(b)

        loop = asyncio.get_running_loop()
        write_task = loop.create_task(slow_write(sock_out, bytes_in))
        read_task = loop.create_task(async_sock_in.read())
        done, _ = await asyncio.wait([write_task, read_task])
        self.assertEqual(len(done), 2)
        read_bytes = None
        for item in done:
            if item.result() is not None:
                read_bytes = item.result()
        self.assertEqual(read_bytes, bytes_in)

    async def test_async_socket_write_fails_when_closed(self) -> None:
        """Verifies an async socket write fails in the expected way when the other end is closed."""
        ctx = Context()
        (sock_out, sock_in) = ctx.socket_create()
        del sock_in
        with self.assertRaises(FcTransportStatus):
            async_sock_out = AsyncSocket(sock_out)
            try:
                async_sock_out.write(bytearray([1, 2, 3]))
            except FcTransportStatus as e:
                self.assertEqual(
                    e.code(), FcTransportStatus.FC_ERR_SOCKET_WRITE
                )
                raise e

    async def test_async_socket_read_fails_when_closed(self) -> None:
        """Verifies an async socket read fails in the expected way when the other end is closed."""
        ctx = Context()
        (sock_out, sock_in) = ctx.socket_create()
        del sock_out
        with self.assertRaises(FcTransportStatus):
            async_sock_in = AsyncSocket(sock_in)
            try:
                await async_sock_in.read()
            except FcTransportStatus as e:
                self.assertEqual(e.code(), FcTransportStatus.FC_ERR_FDOMAIN)
                raise e

    async def test_async_socket_read_all_twice_fails(self) -> None:
        """Verifies running `read_all()` twice fails."""
        ctx = Context()
        (sock_out, sock_in) = ctx.socket_create()
        with self.assertRaises(AlreadyReadingAll):
            async_sock_in = AsyncSocket(sock_in)
            task = asyncio.get_running_loop().create_task(
                async_sock_in.read_all()
            )
            await asyncio.gather(async_sock_in.read_all(), task)

    async def test_async_socket_read_all_then_read_fails(self) -> None:
        """Verifies running `read_all()` then `read()` fails."""
        ctx = Context()
        (sock_out, sock_in) = ctx.socket_create()
        with self.assertRaises(AlreadyReadingAll):
            async_sock_in = AsyncSocket(sock_in)
            task = asyncio.get_running_loop().create_task(
                async_sock_in.read_all()
            )
            await asyncio.gather(async_sock_in.read(), task)
