# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import inspect
import logging
import struct
from inspect import getframeinfo, stack
from typing import Any

import fuchsia_controller_py as fc

from ._fidl_common import (
    DomainError,
    FidlMessage,
    FidlMeta,
    FrameworkError,
    parse_ordinal,
    parse_txid,
)
from ._ipc import GlobalHandleWaker, HandleWaker
from ._registry import get_registered_method

# Rather than make a long server UUID, this will be a monotonically increasing
# ID to differentiate servers for debugging purposes.
_SERVER_ID = 0
_LOGGER = logging.getLogger("fidl.server")


class ServerError(Exception):
    pass


class ServerBase(
    metaclass=FidlMeta,
    required_class_variables=[
        ("library", str),
        ("method_map", dict),
    ],
):
    """Base object for doing basic FIDL server tasks."""

    _channel: fc.Channel | None
    library: str
    method_map: dict[int, Any]

    def __str__(self) -> str:
        return f"server:{type(self).__name__}:{id(self)}"

    def __init__(
        self,
        channel: fc.Channel,
        channel_waker: HandleWaker | None = None,
    ) -> None:
        global _SERVER_ID
        self._channel = channel
        self.id = _SERVER_ID
        _SERVER_ID += 1
        if channel_waker is None:
            self._channel_waker: HandleWaker = GlobalHandleWaker()
        else:
            self._channel_waker = channel_waker
        caller = getframeinfo(stack()[1][0])
        _LOGGER.debug(
            f"{self} instantiated from {caller.filename}:{caller.lineno}"
        )

    async def serve(self) -> None:
        if self._channel is None:
            raise ValueError("Channel is already closed")

        try:
            with self._channel_waker.registration(
                self._channel, name=str(self)
            ):
                while await self.handle_next_request():
                    pass
        finally:
            # As long as we don't do something silly like catch a
            # `GeneratorExit` exception or a `CancelledError` in the above
            # code we should always execute this `finally` block.
            _LOGGER.debug(f"{self} completed serving. Closing channel")
            # Explicitly close the channel instead of deferring closure to the
            # garbage collector to close it. The garbage collector may never
            # close the channel since removing the last reference to an object
            # only ever makes it eligible for garbage collection. One cannot
            # treat these objects like scoped RAII objects in C++ or Rust.
            #
            # If another coroutine depends on this server functioning (like a
            # client), then it'll hang forever. So, we must close the channel in
            # order to make progress.
            if self._channel is not None:
                self._channel.close()

    async def handle_next_request(self) -> bool:
        # TODO(b/299946378): Handle case where ordinal is unknown.
        # TODO(b/303532690): When attempting to decode a method that is
        # unrecognized, there should be a message sent declaring this is
        # an unknown method.
        try:
            msg, txid, ordinal = await self._channel_read_and_parse()
        except fc.FcTransportStatus as e:
            if e.code() == fc.FcTransportStatus.FC_ERR_FDOMAIN:
                _LOGGER.debug(f"{self} shutting down. PEER_CLOSED received")
                return False
            else:
                _LOGGER.warn(f"{self} channel received error: {e}")
                raise e
        info = self.method_map[ordinal]
        info.request_ident
        method_name = info.name
        method = getattr(self, method_name)
        if msg is not None:
            res = method(msg)
        else:
            res = method()
        if inspect.isawaitable(res):
            res = await res
        if res is not None and not info.requires_response:
            raise ServerError(
                f"{self} method {info.name} received a "
                + "response but is one-way method"
            )
        if res is None and info.requires_response and not info.empty_response:
            raise ServerError(
                f"{self} method {info.name} returned "
                + "None when a response was expected"
            )
        if not info.requires_response:
            return True

        if info.has_result:
            _LOGGER.debug(f"{self} received method response {res}")
            method_reg = get_registered_method(ordinal)
            if method_reg is None:
                raise RuntimeError(f"Unknown ordinal {ordinal}")
            _, response_cls = method_reg
            assert response_cls is not None
            if type(res) is DomainError:
                res = response_cls(err=res.error)
            elif type(res) is FrameworkError:
                res = response_cls(framework_err=res)
            else:
                if res is None:
                    res = response_cls(response=None)
                else:
                    res = response_cls(response=res)

        if res is not None:
            payload_bytes, handles = res.encode()
        else:
            payload_bytes, handles = b"", []

        header = struct.pack("<IHBBQ", txid, 0x02, 0x00, 0x01, ordinal)
        encoded_msg = header + payload_bytes

        if self._channel is None:
            raise ValueError("Channel is already closed")
        self._channel.write((encoded_msg, handles))
        return True

    async def _channel_read(self) -> FidlMessage:
        while True:
            if self._channel is None:
                raise ValueError("Channel is already closed")
            try:
                return self._channel.read()
            except fc.FcTransportStatus as e:
                # Any number of spurious wakeups are possible. Stay in the loop if the error
                # is FC_ERR_SHOULD_WAIT.
                if e.code() == fc.FcTransportStatus.FC_ERR_SHOULD_WAIT:
                    _LOGGER.debug(f"{self} channel spurious wakeup")
                    await self._channel_waker.wait_ready(self._channel)
                    continue
                _LOGGER.warning(f"{self} channel received error: {e}")
                raise e

    async def _channel_read_and_parse(self) -> tuple[Any, int, int]:
        raw_msg = await self._channel_read()
        ordinal = parse_ordinal(raw_msg)
        txid = parse_txid(raw_msg)
        method = get_registered_method(ordinal)
        if method is None:
            raise RuntimeError(f"Unknown ordinal {ordinal}")
        request_cls, _ = method

        handles = raw_msg[1]
        verified_handles: list[int] = [0] * len(handles)
        for i in range(len(handles)):
            hdl = handles[i]
            if isinstance(hdl, tuple):
                verified_handles[i] = hdl[1]
            else:
                verified_handles[i] = hdl.take()

        if request_cls is not None:
            result_obj = request_cls.decode(raw_msg[0][16:], verified_handles)
        else:
            result_obj = None

        return result_obj, txid, ordinal

    def _send_event(self, ordinal: int, library: str, msg_obj: Any) -> None:
        if msg_obj is not None:
            payload_bytes, handles = msg_obj.encode()
        else:
            payload_bytes, handles = b"", []

        header = struct.pack("<IHBBQ", 0, 0x02, 0x00, 0x01, ordinal)
        encoded_msg = header + payload_bytes

        if self._channel is None:
            raise ValueError("Channel is already closed")
        self._channel.write((encoded_msg, handles))
