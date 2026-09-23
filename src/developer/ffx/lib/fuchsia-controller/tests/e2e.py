# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import asyncio
import os
import os.path
import struct
import sys
import typing
import unittest

import fidl_fuchsia_controller_test as fc_test
import fidl_fuchsia_developer_ffx as ffx_fidl
from fuchsia_controller_py import (
    Channel,
    Context,
    FcTransportStatus,
    IsolateDir,
)

from fidl import AsyncChannel, DomainError


class TestSocketServer(fc_test.TestingServer):
    def __init__(self, ctx: Context, ch: Channel):
        client, server = ctx.socket_create()
        self.client = client
        self.server = server
        super().__init__(ch)

    def return_socket_or_error(
        self,
    ) -> fc_test.TestingServer.ReturnSocketOrErrorResponse:
        if self.server.as_int() == 0:
            return DomainError(FcTransportStatus.FC_ERR_NOT_SUPPORTED)
        return fc_test.TestingReturnSocketOrErrorResponse(
            reader_thing=self.server.take()
        )

    def return_union(self) -> fc_test.TestingServer.ReturnUnionResponse:
        res = fc_test.TestingReturnUnionResponse(x=10)
        return res

    def return_union_with_table(
        self,
    ) -> fc_test.TestingServer.ReturnUnionWithTableResponse:
        x = fc_test.NoopUnion(union_str="foobar")
        res = fc_test.TestingReturnUnionWithTableResponse(x=x)
        return res

    def return_other_composed_protocol(
        self,
    ) -> fc_test.TestingServer.ReturnOtherComposedProtocolResponse:
        return fc_test.TestingReturnOtherComposedProtocolResponse(
            client_thing=0
        )

    def return_possible_error(
        self,
    ) -> fc_test.TestingServer.ReturnPossibleErrorResponse:
        return DomainError("not implemented")

    def return_possible_error2(
        self,
    ) -> fc_test.TestingServer.ReturnPossibleError2Response:
        return DomainError("not implemented")


class EndToEnd(unittest.IsolatedAsyncioTestCase):
    def _get_default_config(self) -> typing.Dict[str, str]:
        return {}

    def _get_isolate_dir(self) -> IsolateDir:
        isolation_path = None
        tmp_path = os.getenv("TEST_UNDECLARED_OUTPUTS_DIR")
        if tmp_path:
            isolation_path = os.path.join(tmp_path, "isolate")
        return IsolateDir(dir=isolation_path)

    def test_config_get_nonexistent(self) -> None:
        ctx = Context()
        self.assertEqual(ctx.config_get_string("foobarzzzzzzo==?"), None)

    def test_config_get_exists(self) -> None:
        config = self._get_default_config()
        key = "foobar"
        expect = "bazmumble"
        config[key] = expect
        ctx = Context(config=config, isolate_dir=self._get_isolate_dir())
        self.assertEqual(ctx.config_get_string(key), expect)

    def test_config_get_too_long(self) -> None:
        config = self._get_default_config()
        key = "foobarzington"
        expect = "b" * 50000
        config[key] = expect
        ctx = Context(config=config, isolate_dir=self._get_isolate_dir())
        with self.assertRaises(BufferError):
            ctx.config_get_string(key)

    async def test_client_sends_message_before_coro_await(self) -> None:
        ctx = Context()
        (ch0, ch1) = ctx.channel_create()
        async_ch1 = AsyncChannel(ch1)
        echo_proxy = ffx_fidl.EchoClient(ch0)
        coro = echo_proxy.echo_string(value="foo")
        buf, _ = await async_ch1.read()
        txid = int.from_bytes(buf[0:4], sys.byteorder)

        ordinal = next(
            ord
            for ord, info in ffx_fidl.EchoServer.method_map.items()
            if info.name == "echo_string"
        )

        def encode_message(obj: typing.Any) -> tuple[bytes, list[typing.Any]]:
            if obj is not None:
                payload, handles = obj.encode()
            else:
                payload, handles = b"", []
            header = struct.pack("<IHBBQ", txid, 0x02, 0x00, 0x01, ordinal)
            return header + payload, handles

        encoded_bytes, _ = encode_message(
            ffx_fidl.EchoEchoStringRequest(value="foo")
        )
        self.assertEqual(buf, encoded_bytes)
        msg = encode_message(
            ffx_fidl.EchoEchoStringResponse(response="otherthing")
        )
        async_ch1.write(msg)
        result = await coro
        self.assertEqual(result.response, "otherthing")

    def test_context_creation_duplicate_target_raises_exception(self) -> None:
        with self.assertRaises(RuntimeError):
            _ctx = Context(target="foo", config={"target.default": "bar"})

    def test_context_creation_no_args(self) -> None:
        Context()

    async def test_sending_fidl_protocol(self) -> None:
        ctx = Context()
        tc_server, tc_client = ctx.channel_create()
        list_server, list_client = ctx.channel_create()
        tc_proxy = ffx_fidl.TargetCollectionClient(tc_client)
        query = ffx_fidl.TargetQuery(string_matcher="foobar")
        tc_proxy.list_targets(query=query, reader=list_client.take())
        async_tc_server = AsyncChannel(tc_server)
        buf, hdls = await async_tc_server.read()
        self.assertEqual(len(hdls), 1)
        new_list_client = Channel(hdls[0])
        new_list_client.write((bytearray([5, 6, 7]), []))
        async_list_server = AsyncChannel(list_server)
        list_buf, list_hdls = await async_list_server.read()
        self.assertEqual(len(list_hdls), 0)
        self.assertEqual(list_buf, bytearray([5, 6, 7]))

    async def test_sending_socket_as_result(self) -> None:
        ctx = Context()
        t_server, t_client = ctx.channel_create()
        server = TestSocketServer(ctx, t_server)
        client = fc_test.TestingClient(t_client)
        server_task = asyncio.get_running_loop().create_task(server.serve())
        res1 = await client.return_union()
        self.assertEqual(res1.x, 10)
        res2 = await client.return_union_with_table()
        # This is to placate mypy
        assert res2.x is not None
        self.assertEqual(res2.x.union_str, "foobar")
        res3 = await client.return_socket_or_error()
        # TODO(b/415379365): Given this is intended to be a socket, it should return
        # the appropriate type here.
        assert res3.response is not None
        self.assertNotEqual(res3.response.reader_thing, 0)
        res4 = await client.return_socket_or_error()
        self.assertEqual(res4.err, FcTransportStatus.FC_ERR_NOT_SUPPORTED)
        server_task.cancel()
