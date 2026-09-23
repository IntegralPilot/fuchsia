# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import asyncio
import logging

import fuchsia_controller_py as fc

from ._ipc import GlobalHandleWaker, HandleWaker

_LOGGER: logging.Logger = logging.getLogger(__name__)


class AlreadyReadingAll(Exception):
    def __init__(self) -> None:
        super().__init__("This socket is already doing a read-all operation.")


class AsyncSocket:
    """Represents an async socket.

    In 99% of cases it is recommended to use this instead of the standard fuchsia-controller Socket
    object. This has built-in support for handling waits.

    In the remaining 1% of cases it may be useful for the user to do a one-off attempt at reading a
    socket directly and immediately exiting in the event that FC_ERR_SHOULD_WAIT is encountered.
    Such a case would likely involve adding some custom behavior to existing async code, like
    registering custom wakers. Another case would be where the user is _only_ writing to the socket,
    as writes in AsyncSocket are also synchronous and wrap Socket.write directly.

    Args:
        socket: The socket which will be asynchronously readable.
        waker: (Optional) the HandleWaker implementation (defaults to GlobalHandleWaker).

    Raises:

    If you are running this socket in a task using `read_all()` be sure there is
    only one instance of `read_all()` running, else this will raise an exception
    in the case where one attempts to run `read()` or `read_all()` after this
    has been initiated.

    Keep in mind that if you run `read_all()` and place it on a task in the
    executor, and you do not yield to the executor, and then attempt to run
    `read_all()`, then it will be the task itself that will fail, because the
    read invocation will be reached before yielding to the async task, so you
    cannot solely rely on this to catch potential deadlocks.
    """

    waker: HandleWaker

    def __init__(self, socket: fc.Socket, waker: HandleWaker | None = None):
        self.socket = socket
        if waker is None:
            self.waker = GlobalHandleWaker()
        else:
            self.waker = waker
        self._read_all_lock = asyncio.Lock()

    def __del__(self) -> None:
        self.socket.close()

    def _invariant_check(self) -> None:
        """Checks to make sure we're not already in the middle of a read-all
        invocation."""
        if self._read_all_lock.locked():
            raise AlreadyReadingAll()

    async def read(self) -> bytes:
        """Attempts to read off of the socket.

        Returns:
            bytes read from the socket.

        Raises:
            FcTransportStatus exception outlining the specific failure of the underlying handle.

            AlreadyReadAll exception if `read_all()` is being invoked elsewhere
            asynchronously.
        """
        self._invariant_check()
        return await self._read()

    async def _read(self) -> bytes:
        _LOGGER.debug("Doing read of socket: %s", self.socket)
        with self.waker.registration(
            self.socket, name=f"AsyncSocket {self.socket}"
        ):
            while True:
                try:
                    return self.socket.read()
                except fc.FcTransportStatus as e:
                    if e.args[0] != fc.FcTransportStatus.FC_ERR_SHOULD_WAIT:
                        raise e
                    _LOGGER.debug(
                        f"Received wait signal for socket: {self.socket}"
                    )
                _LOGGER.debug(f"Awaiting socket wake for socket: {self.socket}")
                await self.waker.wait_ready(self.socket)

    async def read_all(self) -> bytearray:
        """Attempts to read all data on the socket until it is closed.

        Returns:
            All bytes read on the socket.

        Raises:
            Any FC errors encountered besides FC_ERR_SHOULD_WAIT or
            FC_ERR_FDOMAIN.

            AlreadyReadAll exception if `read_all()` is being invoked elsewhere
            asynchronously.
        """
        self._invariant_check()
        output = bytearray()
        async with self._read_all_lock:
            while True:
                try:
                    output.extend(await self._read())
                except fc.FcTransportStatus as status:
                    _LOGGER.debug(
                        f"Socket {self.socket} caught exception: {status}"
                    )
                    err_code = status.code()
                    if err_code != fc.FcTransportStatus.FC_ERR_FDOMAIN:
                        raise status
                    break
            self.socket.close()
            return output

    def write(self, buf: bytes) -> None:
        """Does a blocking write on the socket.

        This is identical to calling the write function on the socket itself.

        Args:
            buf: The array of bytes (read-only) to write to the socket.

        Raises:
            FcTransportStatus exception on failure of the underlying handle.
        """
        self.socket.write(buf)
