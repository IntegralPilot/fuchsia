# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Module for handling encoding and decoding FIDL messages, as well as for handling async I/O."""

import asyncio
import logging
import socket
import traceback
from abc import ABC, abstractmethod
from types import TracebackType
from typing import Dict, Self

import fuchsia_controller_py as fc

logger = logging.getLogger(__name__)


HANDLE_READY_QUEUES: Dict[int, asyncio.Queue[int]] = {}
HANDLE_REFCOUNTS: Dict[int, int] = {}


def enqueue_ready_zx_handle_from_fd(
    fd: int, handle_ready_queues: Dict[int, asyncio.Queue[int]]
) -> None:
    """Reads zx_handle that is ready for reading, and enqueues it in the appropriate ready queue."""
    s = socket.fromfd(fd, socket.AF_UNIX, socket.SOCK_STREAM)
    handle_no = int.from_bytes(s.recv(4), "little")

    queue = handle_ready_queues.get(handle_no)
    if not queue:
        logger.debug(f"Dropping notification for: {handle_no}")
        return

    queue.put_nowait(handle_no)


class HandleRegistration:
    """A scoped object for handles.

    This is intended to be used with the `with` keyword to ensure that handle registration is
    properly cleaned up.
    """

    def __init__(
        self, waker: "HandleWaker", h: fc.BaseHandle, *, name: str
    ) -> None:
        self.waker = waker
        self.handle = h
        self.name = name

    def __enter__(self) -> Self:
        self.waker._register(self.handle, name=self.name)
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> None:
        self.waker._unregister(self.handle)


class HandleWaker(ABC):
    """Base class for a waker used with potentially blocking handles."""

    @abstractmethod
    def _register(self, channel: fc.BaseHandle, *, name: str) -> None:
        """Registers a handle to receive wake notifications."""

    @abstractmethod
    def _unregister(self, channel: fc.BaseHandle) -> None:
        """Unregisters a handle, meaning it is not possible to wait for it to be ready."""

    def registration(
        self, channel: fc.BaseHandle, *, name: str
    ) -> HandleRegistration:
        """Returns a scoped registration object for a handle."""
        return HandleRegistration(self, channel, name=name)

    @abstractmethod
    def post_ready(self, channel: fc.BaseHandle) -> None:
        """Notifies the waker that a channel is ready."""

    @abstractmethod
    async def wait_ready(self, channel: fc.BaseHandle) -> int:
        """Waits for a channel to be ready asynchronously."""


class GlobalHandleWaker(HandleWaker):
    """A class for handling notifications on a readable handle.

    As this is a global channel waker, this hooks into global state. This is the default waker used
    with all client and server code, as well as async wrapper code around readable handles.
    """

    def __init__(self) -> None:
        self._handle_ready_queues = HANDLE_READY_QUEUES
        self._handle_refcounts = HANDLE_REFCOUNTS

    def _register(self, h: fc.BaseHandle, *, name: str) -> None:
        h_id = h.as_int()
        if h_id not in self._handle_ready_queues:
            self._handle_ready_queues[h_id] = asyncio.Queue()
            self._handle_refcounts[h_id] = 0
        self._handle_refcounts[h_id] += 1

        notification_fd = fc.connect_handle_notifier()
        # This try call is simply here in case registration occurs in outside
        # an async event loop.
        try:
            # Calling this multiple times only overwrites the reader.
            # In the event that the loop is destroyed this will be removed automatically.
            asyncio.get_running_loop().add_reader(
                notification_fd,
                enqueue_ready_zx_handle_from_fd,
                notification_fd,
                self._handle_ready_queues,
            )
        except RuntimeError as e:
            logger.debug(
                "Registered reader in non-async context. Printing exception for debugging."
            )
            logger.debug("[[ TRACE BEGIN ]]")
            for line in map(lambda x: x.strip(), traceback.format_exception(e)):
                logger.debug(f"-- {line}")
            logger.debug("[[ TRACE END ]]")

    def _unregister(self, h: fc.BaseHandle) -> None:
        h_id = h.as_int()
        if h_id in self._handle_ready_queues:
            self._handle_refcounts[h_id] -= 1
            if self._handle_refcounts[h_id] <= 0:
                self._handle_ready_queues.pop(h_id)
                self._handle_refcounts.pop(h_id)

    def post_ready(self, h: fc.BaseHandle) -> None:
        logger.debug(f"Re-notifying for channel: {h.as_int()}")
        self._handle_ready_queues[h.as_int()].put_nowait(h.as_int())

    async def wait_ready(self, h: fc.BaseHandle) -> int:
        res = await self._handle_ready_queues[h.as_int()].get()
        self._handle_ready_queues[h.as_int()].task_done()
        return res

    def _reset_for_testing(self) -> None:
        """This is a method intended only to be used for testing.

        This implementation of handle notifications doesn't consistently get cleaned up
        when channels are garbage collected. There is likely a clearer root-cause
        for this that can be prevented, but for the time being this is a hacked-together
        fix for using `unittest.IsolatedAsyncioTestCase` with various FIDL server
        implementations and background tasks. Each test uses its own new/isolated
        loop to prevent pollution from other tasks, so resetting state will at least
        ensure that any asyncio.Queue objects in our state are removed and don't cross from
        one loop into another.
        """
        self._handle_ready_queues.clear()
        self._handle_refcounts.clear()
