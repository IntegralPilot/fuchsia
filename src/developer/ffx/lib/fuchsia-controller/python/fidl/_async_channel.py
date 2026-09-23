# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import fuchsia_controller_py as fc

from ._ipc import GlobalHandleWaker, HandleWaker


class AsyncChannel:
    """An async channel wrapper.

    This is primarily used to wrap async functionality around channel reads.

    Args:
        channel: The channel which will be asynchronously readable.
        waker: (Optional) the HandleWaker implementation (defaults to GlobalHandleWaker).
    """

    waker: HandleWaker

    def __init__(self, channel: fc.Channel, waker: HandleWaker | None = None):
        self.channel = channel
        if waker is None:
            self.waker = GlobalHandleWaker()
        else:
            self.waker = waker

    async def read(self) -> tuple[bytes, list[fc.Handle]]:
        """Attempts to read off of the channel.

        Returns:
            bytes read from the channel.

        Raises:
            FcTransportStatus exception outlining the specific failure of the underlying handle.
        """
        with self.waker.registration(
            self.channel, name=f"AsyncChannel {self.channel}"
        ):
            while True:
                try:
                    return self.channel.read()
                except fc.FcTransportStatus as e:
                    if e.args[0] != fc.FcTransportStatus.FC_ERR_SHOULD_WAIT:
                        raise e
                await self.waker.wait_ready(self.channel)

    def write(
        self,
        bytes_and_handles: tuple[bytes, list[tuple[int, int, int, int, int]]],
    ) -> None:
        """Does a blocking write on the channel.

        This is identical to calling the write function on the channel itself.

        Args:
            bytes_and_handles: The array of bytes (read-only) to write to the channel
                               and the handle dispositions to be sent.

        Raises:
            FcTransportStatus exception on failure of the underlying handle.
        """
        self.channel.write(bytes_and_handles)

    def __del__(self) -> None:
        self.channel.close()
