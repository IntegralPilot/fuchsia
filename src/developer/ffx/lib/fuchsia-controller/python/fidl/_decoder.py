# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import struct
import typing


def _align_to_8(offset: int) -> int:
    """Rounds an offset up to the next 8-byte boundary.

    According to the FIDL wire format specification (RFC-0027/RFC-0120), all
    out-of-line secondary objects (such as strings, vectors, out-of-line envelopes,
    and out-of-line structs) must be aligned to 8-byte boundaries.
    """
    pad_mod = offset % 8
    return offset + (8 - pad_mod) if pad_mod != 0 else offset


class Decoder:
    def __init__(self, buf: bytes, handles: list[typing.Any]) -> None:
        self.buf = buf
        self.handles = handles
        self.ool_offset = 0
        self.handle_index = 0

    def _align_ool_offset(self) -> None:
        """Aligns the out-of-line offset to the next 8-byte boundary."""
        self.ool_offset = _align_to_8(self.ool_offset)

    def read_handle(self, offset: int, nullable: bool = False) -> typing.Any:
        presence = self._read_primitive("<I", offset)
        if presence == 0:
            if nullable:
                return None
            else:
                raise ValueError("Absent non-nullable handle")
        if self.handle_index >= len(self.handles):
            raise ValueError("Not enough handles in decoder")
        hdl = self.handles[self.handle_index]
        self.handle_index += 1
        # Handles in `self.handles` can either be handle wrappers (e.g.,
        # `fuchsia_controller_py.Handle` or FDomain handles providing `.as_int()`)
        # or raw integer handle values (e.g., in unit tests or GIDL runners).
        if hasattr(hdl, "as_int"):
            return hdl.as_int()
        return int(hdl)

    def read_bool(self, offset: int) -> bool:
        return self.buf[offset] != 0

    def _read_primitive(self, fmt: str, offset: int) -> typing.Any:
        return struct.unpack_from(fmt, self.buf, offset)[0]

    def read_int8(self, offset: int) -> int:
        return self._read_primitive("<b", offset)

    def read_uint8(self, offset: int) -> int:
        return self._read_primitive("<B", offset)

    def read_int16(self, offset: int) -> int:
        return self._read_primitive("<h", offset)

    def read_uint16(self, offset: int) -> int:
        return self._read_primitive("<H", offset)

    def read_int32(self, offset: int) -> int:
        return self._read_primitive("<i", offset)

    def read_uint32(self, offset: int) -> int:
        return self._read_primitive("<I", offset)

    def read_int64(self, offset: int) -> int:
        return self._read_primitive("<q", offset)

    def read_uint64(self, offset: int) -> int:
        return self._read_primitive("<Q", offset)

    def read_float32(self, offset: int) -> float:
        return self._read_primitive("<f", offset)

    def read_float64(self, offset: int) -> float:
        return self._read_primitive("<d", offset)

    def read_string(self, offset: int, nullable: bool = False) -> typing.Any:
        length = struct.unpack_from("<Q", self.buf, offset)[0]
        presence = struct.unpack_from("<Q", self.buf, offset + 8)[0]
        if presence == 0:
            if nullable:
                return None
            else:
                raise ValueError("Absent non-nullable string")
        self._align_ool_offset()
        res = self.buf[self.ool_offset : self.ool_offset + length].decode(
            "utf-8"
        )
        self.ool_offset += length
        return res

    def read_vector(
        self,
        offset: int,
        element_decoder_fn: typing.Callable[["Decoder", int], typing.Any],
        element_size: int,
        nullable: bool = False,
    ) -> typing.Any:
        count = struct.unpack_from("<Q", self.buf, offset)[0]
        presence = struct.unpack_from("<Q", self.buf, offset + 8)[0]
        if presence == 0:
            if nullable:
                return None
            else:
                raise ValueError("Absent non-nullable vector")
        self._align_ool_offset()
        start_offset = self.ool_offset
        self.ool_offset += element_size * count
        res = []
        for i in range(count):
            res.append(
                element_decoder_fn(self, start_offset + i * element_size)
            )
        return res

    def read_array(
        self,
        offset: int,
        element_decoder_fn: typing.Callable[["Decoder", int], typing.Any],
        element_size: int,
        count: int,
    ) -> list[typing.Any]:
        res = []
        for i in range(count):
            res.append(element_decoder_fn(self, offset + i * element_size))
        return res

    def read_optional_struct(
        self,
        offset: int,
        decode_fn: typing.Callable[["Decoder", int], typing.Any],
        inline_size: int,
    ) -> typing.Any:
        presence = struct.unpack_from("<Q", self.buf, offset)[0]
        if presence == 0:
            return None

        self._align_ool_offset()
        start_offset = self.ool_offset
        self.ool_offset += inline_size
        return decode_fn(self, start_offset)

    def read_envelope(
        self,
        env_offset: int,
        decode_fn: typing.Callable[["Decoder", int], typing.Any],
        inline_size: int,
    ) -> typing.Any:
        is_empty = self.buf[env_offset : env_offset + 8] == b"\x00" * 8
        if is_empty:
            return None

        size_or_value = self.buf[env_offset : env_offset + 4]
        struct.unpack_from("<H", self.buf, env_offset + 4)[0]
        flags = struct.unpack_from("<H", self.buf, env_offset + 6)[0]
        inlined = (flags & 1) != 0

        if inlined:
            temp_decoder = Decoder(
                size_or_value, self.handles[self.handle_index :]
            )
            val = decode_fn(temp_decoder, 0)
            self.handle_index += temp_decoder.handle_index
            return val
        else:
            self._align_ool_offset()
            num_bytes = struct.unpack_from("<I", self.buf, env_offset)[0]
            member_offset = self.ool_offset

            inline_size_aligned = _align_to_8(inline_size)

            self.ool_offset = member_offset + inline_size_aligned
            val = decode_fn(self, member_offset)

            self.ool_offset = _align_to_8(member_offset + num_bytes)
            return val

    def skip_envelope(self, env_offset: int) -> None:
        is_empty = self.buf[env_offset : env_offset + 8] == b"\x00" * 8
        if is_empty:
            return

        num_handles = struct.unpack_from("<H", self.buf, env_offset + 4)[0]
        flags = struct.unpack_from("<H", self.buf, env_offset + 6)[0]
        inlined = (flags & 1) != 0

        self.handle_index += num_handles

        if not inlined:
            num_bytes = struct.unpack_from("<I", self.buf, env_offset)[0]
            self._align_ool_offset()
            self.ool_offset = _align_to_8(self.ool_offset + num_bytes)


def decode_struct(
    bytes: bytes,
    handles: list[typing.Any],
    decode_fn: typing.Callable[[Decoder, int], typing.Any],
    inline_size: int,
) -> typing.Any:
    decoder = Decoder(bytes, handles)
    decoder.ool_offset = _align_to_8(inline_size)
    return decode_fn(decoder, 0)
