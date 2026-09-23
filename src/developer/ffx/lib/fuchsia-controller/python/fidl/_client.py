# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import inspect
import logging
import struct
from collections.abc import Coroutine
from inspect import getframeinfo, stack
from typing import Any, Dict, Set, Type, TypeVar, overload

import fuchsia_controller_py as fc

from ._fidl_common import (
    FIDL_EPITAPH_ORDINAL,
    Decodable,
    Encodable,
    EpitaphError,
    FidlMessage,
    FidlMeta,
    StopEventHandler,
    TXID_Type,
    parse_epitaph_value,
    parse_ordinal,
    parse_txid,
)
from ._ipc import GlobalHandleWaker, HandleWaker
from ._registry import get_registered_method

# The active TXID (mutable).
TXID: TXID_Type = 0

T = TypeVar("T", bound=Decodable)


# The TXID of a FIDL event.
EVENT_TXID: TXID_Type = 0
# Simple client ID. Monotonically increasing for each client.
_CLIENT_ID = 0
_LOGGER = logging.getLogger("fidl.client")

# FIDL transactional message header size:
# txid (4B) + at-rest flags (2B) + dynamic flags (2B) + magic (1B) + reserved (1B) + ordinal (8B)
_FIDL_MESSAGE_HEADER_SIZE = 16


class FidlClient(metaclass=FidlMeta):
    def __init__(
        self,
        channel: int | fc.Channel,
        channel_waker: HandleWaker | None = None,
    ) -> None:
        global _CLIENT_ID
        self.id = _CLIENT_ID
        _CLIENT_ID += 1
        if isinstance(channel, int):
            self._channel: fc.Channel | None = fc.Channel(channel)
        else:
            self._channel = channel
        if channel_waker is None:
            self._channel_waker: HandleWaker = GlobalHandleWaker()
        else:
            self._channel_waker = channel_waker
        self.pending_txids: Set[TXID_Type] = set({})
        self.staged_messages: Dict[TXID_Type, asyncio.Queue[FidlMessage]] = {}
        self.epitaph_received: EpitaphError | None = None
        self.epitaph_event = asyncio.Event()
        caller = getframeinfo(stack()[1][0])
        _LOGGER.debug(
            f"{self} instantiated from {caller.filename}:{caller.lineno}"
        )

    def close_cleanly(self) -> None:
        """Closes the underlying channel.

        This is so-named to avoid name conflicts with existing FIDL methods.
        """
        if self._channel is not None:
            self._channel.close()
            # self._channel = None is NOT done here, as it may be used in pending tasks.
            # The handle being closed will result in errors in those tasks.

    def __str__(self) -> str:
        return f"client:{type(self).__name__}:{self.id}"

    async def _get_staged_message(self, txid: TXID_Type) -> FidlMessage:
        res = await self.staged_messages[txid].get()
        self.staged_messages[txid].task_done()
        return res

    def _stage_message(self, txid: TXID_Type, msg: FidlMessage) -> None:
        # This should only ever happen if we're a channel reading another channel's response before
        # it has ever made a request.
        if txid not in self.staged_messages:
            self.staged_messages[txid] = asyncio.Queue(1)
        self.staged_messages[txid].put_nowait(msg)

    def _clean_staging(self, txid: TXID_Type) -> None:
        self.staged_messages.pop(txid)
        # Events are never added to this set, since they're always pending.
        if txid != EVENT_TXID:
            self.pending_txids.remove(txid)

    def _decode(
        self,
        txid: TXID_Type,
        msg: FidlMessage,
        response_cls: Type[T] | None,
        expected_ordinal: int | None = None,
    ) -> T | None:
        self._clean_staging(txid)
        if expected_ordinal is not None:
            actual_ordinal = parse_ordinal(msg)
            if actual_ordinal != expected_ordinal:
                type_name = (
                    response_cls.__name__
                    if response_cls is not None
                    else "None"
                )
                raise RuntimeError(
                    f"Ordinal mismatch when decoding {type_name}: expected {expected_ordinal}, got {actual_ordinal}"
                )
        if response_cls is None:
            return None

        handles = msg[1]
        verified_handles: list[int] = [0] * len(handles)
        for i in range(len(handles)):
            hdl = handles[i]
            if isinstance(hdl, tuple):
                verified_handles[i] = hdl[1]
            else:
                verified_handles[i] = hdl.take()
        return response_cls.decode(
            msg[0][_FIDL_MESSAGE_HEADER_SIZE:], verified_handles
        )

    async def next_event(self) -> FidlMessage | None:
        """Attempts to read the next FIDL event from this client.

        Returns:
            The next FIDL event. If ZX_ERR_PEER_CLOSED is received on the channel, will return None.
            Note: this does not check to see if the protocol supports any events, so if not this
            function could wait forever.

        Raises:
            Any exceptions other than ZX_ERR_PEER_CLOSED (fuchsia_controller_py.FcTransportStatus)
        """
        # TODO(awdavies): Raise an exception if there are no events supported for this client.
        try:
            return await self._read_and_decode(0)
        except fc.FcTransportStatus as e:
            err_code = e.code()
            if err_code != fc.FcTransportStatus.FC_ERR_FDOMAIN:
                _LOGGER.warning(
                    f"{self} received error waiting for next event: {e}"
                )
                raise e
        return None

    def _epitaph_check(self, msg: FidlMessage) -> None:
        # If the epitaph is already set, no need to continue with the remaining
        # work.
        if self.epitaph_received is not None:
            raise self.epitaph_received

        ordinal = parse_ordinal(msg)
        if ordinal == FIDL_EPITAPH_ORDINAL:
            if self.epitaph_received is None:
                self.epitaph_received = EpitaphError(parse_epitaph_value(msg))
                self.epitaph_event.set()
            if self.epitaph_received is not None:
                raise self.epitaph_received

    async def _epitaph_event_wait(self) -> None:
        await self.epitaph_event.wait()
        if self.epitaph_received is not None:
            raise self.epitaph_received

    async def _read_and_decode(
        self,
        txid: int,
        response_cls: Type[T] | None = None,
        expected_ordinal: int | None = None,
    ) -> T | FidlMessage | None:
        if txid not in self.staged_messages:
            self.staged_messages[txid] = asyncio.Queue(1)
        if self._channel is None:
            raise ValueError("Channel is already closed")
        msg: FidlMessage
        with self._channel_waker.registration(self._channel, name=str(self)):
            while True:
                if self.epitaph_received is not None:
                    raise self.epitaph_received

                # 1. Try to read from the channel.
                try:
                    msg = self._channel.read()
                    self._epitaph_check(msg)
                    recvd_txid = parse_txid(msg)
                    if recvd_txid == txid:
                        if txid != EVENT_TXID:
                            return self._decode(
                                txid, msg, response_cls, expected_ordinal
                            )
                        else:
                            self._clean_staging(txid)
                            return msg

                    if (
                        recvd_txid != EVENT_TXID
                        and recvd_txid not in self.pending_txids
                    ):
                        _LOGGER.warning(
                            f"{self} received unexpected TXID: {recvd_txid}"
                        )
                        # Unexpected TXID is often a sign of a bad server or a serious bug.
                        # We don't close the channel here, but we raise.
                        raise RuntimeError(
                            f"{self} received unexpected TXID: {recvd_txid}"
                        )

                    self._stage_message(recvd_txid, msg)
                except fc.FcTransportStatus as e:
                    if e.code() != fc.FcTransportStatus.FC_ERR_SHOULD_WAIT:
                        raise e

                # 2. Wait for either:
                #    - The channel being readable again.
                #    - A staged message for our TXID becoming available.
                #    - An epitaph being received.
                #
                #    This is handled with a task group in the event that
                #    there are task cancellations they apply to the other
                #    tasks in the task group, making cleanup predictable.
                try:
                    async with asyncio.TaskGroup() as tg:
                        read_ready_task = tg.create_task(
                            self._channel_waker.wait_ready(self._channel)
                        )
                        staged_msg_task = tg.create_task(
                            self._get_staged_message(txid)
                        )
                        epitaph_task = tg.create_task(self.epitaph_event.wait())

                        done, pending = await asyncio.wait(
                            [read_ready_task, staged_msg_task, epitaph_task],
                            return_when=asyncio.FIRST_COMPLETED,
                        )

                        for p in pending:
                            p.cancel()
                except ExceptionGroup as eg:
                    # If there is only a single exception, raise this as the
                    # main exception. We care particularly if there is an
                    # epitaph exception raised.
                    if len(eg.exceptions) == 1:
                        raise eg.exceptions[0] from None
                    raise

                if staged_msg_task in done:
                    # If we got a staged message, we're done.
                    msg = staged_msg_task.result()
                    # If read_ready_task was also done, we should "re-notify" because we didn't read.
                    if read_ready_task in done:
                        if self._channel is None:
                            raise ValueError("Channel is already closed")
                        self._channel_waker.post_ready(self._channel)

                    if txid != EVENT_TXID:
                        return self._decode(
                            txid, msg, response_cls, expected_ordinal
                        )
                    else:
                        self._clean_staging(txid)
                        return msg

                if epitaph_task in done:
                    # This will raise the epitaph error.
                    if self.epitaph_received is not None:
                        raise self.epitaph_received
                    return None

                # If only read_ready_task reached here, we just loop again and try to read().

    # Overload for two-way FIDL requests with a response payload.
    # Enables static type checking and IDE autocomplete to infer the exact response
    # object type (e.g. `Coroutine[Any, Any, EchoResponse]`) without requiring
    # `typing.cast` or leaving the return type as `Any` or `T | None`.
    # See: https://mypy.readthedocs.io/en/stable/more_types.html#type-checking-calls-to-overloads
    @overload
    def _send_two_way_fidl_request(
        self,
        ordinal: int,
        library: str,
        msg_obj: Encodable | None,
        response_cls: Type[T],
    ) -> Coroutine[Any, Any, T]:
        ...

    # Overload for two-way FIDL requests with an empty response payload (-> ()).
    # Returns `Coroutine[..., None]` statically rather than `Coroutine[..., T | None]`,
    # allowing IDEs and type checkers to know the call resolves to `None`.
    @overload
    def _send_two_way_fidl_request(
        self,
        ordinal: int,
        library: str,
        msg_obj: Encodable | None,
        response_cls: None = None,
    ) -> Coroutine[Any, Any, None]:
        ...

    def _send_two_way_fidl_request(
        self,
        ordinal: int,
        library: str,
        msg_obj: Encodable | None,
        response_cls: Type[T] | None = None,
    ) -> Coroutine[Any, Any, T | None]:
        """Sends a two-way asynchronous FIDL request.

        Args:
            ordinal: The method ordinal (for encoding).
            library: The FIDL library from which this method ordinal exists.
            msg_obj: The object being sent.
            response_cls: The class of the response object.

        Returns:
            The object from the two-way function.
        """
        global TXID
        TXID += 1
        self.pending_txids.add(TXID)
        self._send_one_way_fidl_request(TXID, ordinal, library, msg_obj)

        async def result(txid: int) -> Any:
            return await self._read_and_decode(
                txid, response_cls, expected_ordinal=ordinal
            )

        return result(TXID)

    def _send_one_way_fidl_request(
        self,
        txid: int,
        ordinal: int,
        library: str,
        msg_obj: Encodable | None,
    ) -> None:
        """Sends a synchronous one-way FIDL request.

        Args:
            ordinal: The method ordinal (for encoding).
            library: The FIDL library from which this method ordinal exists.
            msg_obj: The object being sent.
        """
        if msg_obj is not None:
            payload_bytes, handles = msg_obj.encode()
        else:
            payload_bytes, handles = b"", []

        header = struct.pack("<IHBBQ", txid, 0x02, 0x00, 0x01, ordinal)
        encoded_msg = header + payload_bytes

        if self._channel is None:
            raise ValueError("Channel is already closed")
        self._channel.write((encoded_msg, handles))


class EventHandlerBase(
    metaclass=FidlMeta,
    required_class_variables=[
        ("library", str),
        ("method_map", dict),
    ],
):
    """Base object for doing FIDL client event handling."""

    client: FidlClient
    method_map: Dict[int, Any]

    def __init__(self, client: FidlClient) -> None:
        self.client = client

    def __str__(self) -> str:
        return f"event:{type(self.client).__name__}:{self.client.id}"

    async def serve(self) -> None:
        while True:
            msg = await self.client.next_event()
            # msg is None if the channel has been closed.
            if msg is None:
                break
            if not await self._handle_request(msg):
                break

    async def _handle_request(self, msg: FidlMessage) -> bool:
        try:
            await self._handle_request_helper(msg)
            return True
        except StopEventHandler:
            return False

    async def _handle_request_helper(self, msg: FidlMessage) -> None:
        ordinal = parse_ordinal(msg)
        method = get_registered_method(ordinal)
        if method is None:
            raise RuntimeError(f"Unknown ordinal {ordinal}")
        _, response_cls = method  # Events are registered as response_cls

        handles = msg[1]
        verified_handles: list[int] = [0] * len(handles)
        for i in range(len(handles)):
            hdl = handles[i]
            if isinstance(hdl, tuple):
                verified_handles[i] = hdl[1]
            else:
                verified_handles[i] = hdl.take()

        if response_cls is not None:
            request_obj = response_cls.decode(msg[0][16:], verified_handles)
        else:
            request_obj = None

        method_meta = self.method_map[ordinal]
        method_lambda = getattr(self, method_meta.name)
        if request_obj is not None:
            res = method_lambda(request_obj)
        else:
            res = method_lambda()
        if inspect.isawaitable(res):
            await res
