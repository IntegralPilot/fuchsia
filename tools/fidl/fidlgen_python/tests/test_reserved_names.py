# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import unittest

import fidl_test_python_reserved as test_python_reserved
from fuchsia_controller_py import Context


class ReservedNamesProtocolServer(
    test_python_reserved.ReservedNamesProtocolServer
):
    def next_(self) -> None:
        return

    def struct_arg(
        self,
        request: test_python_reserved.ReservedNamesProtocolStructArgRequest,
    ) -> None:
        assert request.int_ == 0

    def table_arg(
        self, request: test_python_reserved.ReservedNamesProtocolTableArgRequest
    ) -> None:
        assert request.int_ == 1

    def union_arg(
        self, request: test_python_reserved.ReservedNamesProtocolUnionArgRequest
    ) -> None:
        assert request.int_ == 2


class ReservedNamesTestsuite(unittest.IsolatedAsyncioTestCase):
    async def test_reserved_method_name(self) -> None:
        ctx = Context()
        (tx, rx) = ctx.channel_create()
        server = ReservedNamesProtocolServer(rx)
        client = test_python_reserved.ReservedNamesProtocolClient(tx)
        server_task = asyncio.get_running_loop().create_task(server.serve())
        await client.next_()
        server_task.cancel()

    async def test_reserved_struct_arg(self) -> None:
        ctx = Context()
        (tx, rx) = ctx.channel_create()
        server = ReservedNamesProtocolServer(rx)
        client = test_python_reserved.ReservedNamesProtocolClient(tx)
        server_task = asyncio.get_running_loop().create_task(server.serve())
        await client.struct_arg(int_=0)
        server_task.cancel()

    async def test_reserved_table_arg(self) -> None:
        ctx = Context()
        (tx, rx) = ctx.channel_create()
        server = ReservedNamesProtocolServer(rx)
        client = test_python_reserved.ReservedNamesProtocolClient(tx)
        server_task = asyncio.get_running_loop().create_task(server.serve())
        await client.table_arg(int_=1)
        server_task.cancel()

    async def test_reserved_union_arg(self) -> None:
        ctx = Context()
        (tx, rx) = ctx.channel_create()
        server = ReservedNamesProtocolServer(rx)
        client = test_python_reserved.ReservedNamesProtocolClient(tx)
        server_task = asyncio.get_running_loop().create_task(server.serve())
        await client.union_arg(int_=2)
        server_task.cancel()
