# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import atexit
from concurrent.futures import ThreadPoolExecutor

import fuchsia_controller_internal
from fuchsia_controller_internal import (
    FcTransportStatus,
    ZxStatus,
    _block_forever,
)

__all__ = [
    "FcTransportStatus",
    "ZxStatus",
    "Handle",
    "Channel",
    "Event",
    "Socket",
    "Context",
    "IsolateDir",
    "BaseHandle",
    "_block_forever",
]

from abc import ABC, abstractmethod
from typing import Any, Callable, Self

# This is a placeholder to unblock the main thread from receiving signals in
# lieu of a more thoroughly implemented async story for the fuchsia-controller
# ABI.
EXECUTOR = ThreadPoolExecutor(max_workers=1)


def _shutdown_executor() -> None:
    EXECUTOR.shutdown(wait=True, cancel_futures=True)


atexit.register(_shutdown_executor)


def _run_in_executor(
    func: Callable[..., Any], *args: Any, **kwargs: Any
) -> Any:
    future = EXECUTOR.submit(func, *args, **kwargs)
    try:
        return future.result()
    except BaseException:
        future.cancel()
        raise


def connect_handle_notifier() -> int:
    return fuchsia_controller_internal.connect_handle_notifier()


class BaseHandle(ABC):
    @abstractmethod
    def as_int(self) -> int:
        """Returns the underlying handle as an integer."""

    @abstractmethod
    def koid(self) -> int:
        """Returns the underlying kernel object ID."""

    @abstractmethod
    def take(self) -> int:
        """Takes the underlying fidl handle, setting it internally to zero.

        This invalidates the underlying channel. Used for sending a handle
        through FIDL function calls.
        """

    @abstractmethod
    def close(self) -> None:
        """Releases the underlying handle."""


class Handle(BaseHandle):
    """Fuchsia controller FIDL handle.

    This is used to bootstrap processes for FIDL interactions.
    """

    _handle: fuchsia_controller_internal.InternalHandle | None

    def __init__(
        self, handle: int | Self | fuchsia_controller_internal.InternalHandle
    ):
        if isinstance(handle, int):
            self._handle = fuchsia_controller_internal.handle_from_int(handle)
        elif isinstance(handle, Handle):
            self._handle = fuchsia_controller_internal.handle_from_int(
                handle.take()
            )
        else:
            self._handle = handle

    def as_int(self) -> int:
        if self._handle is None:
            raise ValueError("Handle is already closed")
        return fuchsia_controller_internal.handle_as_int(self._handle)

    def koid(self) -> int:
        if self._handle is None:
            raise ValueError("Handle is already closed")
        return _run_in_executor(
            fuchsia_controller_internal.handle_koid, self._handle
        )

    def take(self) -> int:
        if self._handle is None:
            raise ValueError("Handle is already closed")
        return fuchsia_controller_internal.handle_take(self._handle)

    def close(self) -> None:
        self._handle = None


class Socket(BaseHandle):
    """Fuchsia controller Zircon socket. This can be read from and written to.

    Can be constructed from a Handle object, but keep in mind that this will mark
    the caller's handle invalid, leaving this socket to be the only owner of the underlying
    handle.
    """

    _socket: fuchsia_controller_internal.InternalHandle | None

    def __init__(
        self, handle: int | Handle | fuchsia_controller_internal.InternalHandle
    ):
        if isinstance(handle, int):
            self._socket = fuchsia_controller_internal.socket_from_int(handle)
        elif isinstance(handle, Handle):
            self._socket = fuchsia_controller_internal.socket_from_int(
                handle.take()
            )
        else:
            self._socket = handle

    def write(self, buffer: bytes) -> None:
        """Writes data to the socket.

        Args:
            data: The buffer to write to the socket.
                  Will write all data to the socket at once.

        Returns: None

        Raises:
            TypeError: If data is not the correct type.
        """
        if self._socket is None:
            raise ValueError("Socket is already closed")
        _run_in_executor(
            fuchsia_controller_internal.socket_write, self._socket, buffer
        )

    def read(self) -> bytes:
        """Reads data from the socket."""
        if self._socket is None:
            raise ValueError("Socket is already closed")
        return fuchsia_controller_internal.socket_read(self._socket)

    def as_int(self) -> int:
        if self._socket is None:
            raise ValueError("Socket is already closed")
        return fuchsia_controller_internal.socket_as_int(self._socket)

    def take(self) -> int:
        if self._socket is None:
            raise ValueError("Socket is already closed")
        return fuchsia_controller_internal.socket_take(self._socket)

    def koid(self) -> int:
        if self._socket is None:
            raise ValueError("Socket is already closed")
        return _run_in_executor(
            fuchsia_controller_internal.socket_koid, self._socket
        )

    def close(self) -> None:
        self._socket = None


class IsolateDir:
    """Fuchsia controller Isolate Directory.

    Represents an Isolate Directory path to be used by the fuchsia controller
    Context object. This object cleans up the Isolate Directory (if it exists)
    once it goes out of scope.
    """

    def __init__(self, dir: str | None = None) -> None:
        self._handle = fuchsia_controller_internal.isolate_dir_create(dir)

    def directory(self) -> str:
        """Returns a string representing this object's directory.

        The IsolateDir will create it upon initialization.
        """
        return fuchsia_controller_internal.isolate_dir_get_path(self._handle)


class Context:
    """Fuchsia controller context.

    This is the necessary object for interacting with a Fuchsia device.
    """

    _handle: fuchsia_controller_internal.InternalHandle | None
    _directory: IsolateDir

    def __init__(
        self,
        config: dict[str, str] | None = None,
        isolate_dir: IsolateDir | None = None,
        target: str | None = None,
    ) -> None:
        if isolate_dir is None:
            isolate_dir = IsolateDir()
        self._handle = fuchsia_controller_internal.context_create(
            config, isolate_dir.directory(), target
        )
        self._directory = isolate_dir

    def target_wait(self, timeout: int, offline: bool = False) -> None:
        """Waits for the target to be ready.

        Args:
            timeout: The timeout in seconds. Zero is interpreted as an infinite
                     timeout.

        Raises:
            RuntimeError if target is not ready within the timeout.
        """
        if self._handle is None:
            raise ValueError("Context is already closed")
        _run_in_executor(
            fuchsia_controller_internal.context_target_wait,
            self._handle,
            timeout,
            offline,
        )

    def config_get_string(self, key: str) -> str:
        """Looks up a string from the context's config environment.

        This is the same config that ffx uses. If there is an IsolateDir in use,
        then the config will be derived from there, else it will be
        autodetected using the same mechanism as ffx.

        Returns:
            The string value iff the key points to an existing value in the
            config, and said value can be converted into a string.
            Otherwise None is returned.
        """
        if self._handle is None:
            raise ValueError("Context is already closed")
        return fuchsia_controller_internal.context_config_get_string(
            self._handle, key
        )

    def connect_device_proxy(
        self, moniker: str, capability_name: str
    ) -> "Channel":
        """Connects to a device proxy.

        Args:
            moniker: The component moniker to connect to
            capability_name: The capability to connect to

        Returns:
            A FIDL client for the device proxy.
        """
        if self._handle is None:
            raise ValueError("Context is already closed")
        return Channel(
            _run_in_executor(
                fuchsia_controller_internal.context_connect_device_proxy,
                self._handle,
                moniker,
                capability_name,
            )
        )

    def connect_remote_control_proxy(self) -> "Channel":
        """Connects to the remote control proxy.

        Returns:
            A FIDL client for the remote control proxy.
        """
        if self._handle is None:
            raise ValueError("Context is already closed")
        return Channel(
            _run_in_executor(
                fuchsia_controller_internal.context_connect_remote_control_proxy,
                self._handle,
            )
        )

    def channel_create(self) -> tuple["Channel", "Channel"]:
        """Method for creating a pair of channels.

        The returned channels are connected bidirectionally.

        Returns:
            A tuple of two Channel objects.
        """
        if self._handle is None:
            raise ValueError("Context is already closed")
        left, right = _run_in_executor(
            fuchsia_controller_internal.channel_create,
            self._handle,
        )
        return (Channel(left), Channel(right))

    def event_create_pair(self) -> tuple["Event", "Event"]:
        """Method for creating a pair of events.

        The returned event objects are connected bidirectionally.

        Returns:
            A tuple of two event objects.
        """
        if self._handle is None:
            raise ValueError("Context is already closed")
        left, right = _run_in_executor(
            fuchsia_controller_internal.event_create_pair, self._handle
        )
        return (Event(left), Event(right))

    def event_create(self) -> "Event":
        """Method for creating a single event.

        Returns:
            A single event object.
        """
        if self._handle is None:
            raise ValueError("Context is already closed")
        return Event(
            _run_in_executor(
                fuchsia_controller_internal.event_create, self._handle
            )
        )

    def socket_create(
        self, options: int | None = None
    ) -> tuple["Socket", "Socket"]:
        """Method for creating a pair of sockets.

        The returned sockets are connected bidirectionally.

        Returns:
            A tuple of two Socket objects.
        """
        if options is None:
            options = 0
        if self._handle is None:
            raise ValueError("Context is already closed")
        left, right = _run_in_executor(
            fuchsia_controller_internal.socket_create, self._handle, options
        )
        return (Socket(left), Socket(right))

    def close(self) -> None:
        """Releases the underlying handle."""
        self._handle = None


class Channel(BaseHandle):
    """Fuchsia controller FIDL channel. This can be read from and written to.

    Can be constructed from a Handle object, but keep in mind that this will
    mark the caller's handle invalid, leaving this channel to be the only owner
    of the underlying handle.
    """

    _channel: fuchsia_controller_internal.InternalHandle | None

    def __init__(
        self, handle: int | Handle | fuchsia_controller_internal.InternalHandle
    ):
        if isinstance(handle, int):
            self._channel = fuchsia_controller_internal.channel_from_int(handle)
        elif isinstance(handle, Handle):
            self._channel = fuchsia_controller_internal.channel_from_int(
                handle.take()
            )
        else:
            self._channel = handle

    def write(
        self,
        encoded_fidl_message: tuple[
            bytes, list[tuple[int, int, int, int, int]]
        ],
    ) -> None:
        """Writes data to the channel.

        Args:
            encoded_fidl_message: An encoded FIDL message of the format returned
                                  by encode::encode_fidl_message from fidl_codec. This message is a
                                  tuple where the first element is a list of bytes. The second
                                  element is a list of tuples in the structure of a
                                  `zx_handle_disposition_t`, e.g. operation, handle, type, rights,
                                  and result. See `//zircon/system/public/zircon/types.h`.

        Raises:
            TypeError: If data is not the correct type.
        """
        if self._channel is None:
            raise ValueError("Channel is already closed")

        # TODO(https://fxbug.dev/346628306): Each handle disposition must be encoded because the
        # fuchsia_controller_internal C extension performs a memcpy into each actual
        # zx_handle_disposition_t. We should consider creating a HandleDisposition type or changing
        # the tuple type to tuple[c_uint32, c_uint32, c_uint32, c_uint32, c_int32] to ensure the
        # caller passes correctly sized 4 byte integers.
        encoded_handle_dispositions = b"".join(
            [
                x.to_bytes(4, byteorder="little")
                for handle_desc in encoded_fidl_message[1]
                for x in handle_desc
            ]
        )
        return _run_in_executor(
            fuchsia_controller_internal.channel_write,
            self._channel,
            encoded_fidl_message[0],
            encoded_handle_dispositions,
        )

    def read(self) -> tuple[bytes, list[Handle]]:
        """Reads data from the channel."""
        if self._channel is None:
            raise ValueError("Channel is already closed")
        retval = fuchsia_controller_internal.channel_read(self._channel)
        # Convert internal Handle objects to Python Handle objects
        return (retval[0], [Handle(x) for x in retval[1]])

    def as_int(self) -> int:
        if self._channel is None:
            raise ValueError("Channel is already closed")
        return fuchsia_controller_internal.channel_as_int(self._channel)

    def koid(self) -> int:
        if self._channel is None:
            raise ValueError("Channel is already closed")
        return _run_in_executor(
            fuchsia_controller_internal.channel_koid, self._channel
        )

    def take(self) -> int:
        if self._channel is None:
            raise ValueError("Channel is already closed")
        return fuchsia_controller_internal.channel_take(self._channel)

    def close(self) -> None:
        self._channel = None

    def close_with_epitaph(self, epitaph: int) -> None:
        """Sends an epitaph to the channel before closing it."""
        # Header txid: 0u32
        msg = bytearray(b"\x00\x00\x00\x00")
        # Header at-rest flag 1. (use FIDLv2)
        msg.extend(bytes(b"\x02"))
        # Header at-rest flag 2: 0.
        msg.extend(bytes(b"\x00"))
        # Header dynamic flags: none.
        msg.extend(bytes(b"\x00"))
        # Header magic no: 1.
        msg.extend(bytes(b"\x01"))
        # Epitaph ordinal: 64bytes of 1's.
        msg.extend(bytes(b"\xff\xff\xff\xff\xff\xff\xff\xff"))
        if epitaph < 0:
            # If negative do two's complement.
            epitaph = epitaph + (1 << 32)
        epitaph_bytes = epitaph.to_bytes(4, byteorder="little")
        # Encode u32.
        msg.extend(epitaph_bytes)
        msg.extend(bytes(b"\x00\x00\x00\x00"))
        self.write((msg, []))
        self.close()


class Event(BaseHandle):
    """
    Fuchsia controller zx Event object. This can signalled.

    Can be constructed from a Handle object, but keep in mind that this will mark
    the caller's handle invalid, leaving this channel to be the only owner of
    the underlying handle.
    """

    _event: fuchsia_controller_internal.InternalHandle | None

    def __init__(
        self, handle: int | Handle | fuchsia_controller_internal.InternalHandle
    ):
        if isinstance(handle, int):
            self._event = fuchsia_controller_internal.event_from_int(handle)
        elif isinstance(handle, Handle):
            self._event = fuchsia_controller_internal.handle_from_int(
                handle.take()
            )
        else:
            self._event = handle

    def signal_peer(self, clear_mask: int, set_mask: int) -> None:
        """Attempts to signal a peer on the other side of this event."""
        if self._event is None:
            raise ValueError("Event is already closed")
        _run_in_executor(
            fuchsia_controller_internal.event_signal_peer,
            self._event,
            clear_mask,
            set_mask,
        )

    def as_int(self) -> int:
        if self._event is None:
            raise ValueError("Event is already closed")
        return fuchsia_controller_internal.event_as_int(self._event)

    def koid(self) -> int:
        if self._event is None:
            raise ValueError("Event is already closed")
        return _run_in_executor(
            fuchsia_controller_internal.event_koid, self._event
        )

    def take(self) -> int:
        if self._event is None:
            raise ValueError("Event is already closed")
        return fuchsia_controller_internal.event_take(self._event)

    def close(self) -> None:
        self._handle = None
