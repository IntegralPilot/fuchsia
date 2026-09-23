# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import struct
import typing


class Encoder:
    def __init__(self) -> None:
        self.buf = bytearray()
        self.handles: list[typing.Any] = []
        self.ool_tasks: list[typing.Callable[[], None]] = []

    def write_inline(self, offset: int, data: bytes) -> None:
        needed = offset + len(data)
        if len(self.buf) < needed:
            self.buf.extend(b"\x00" * (needed - len(self.buf)))
        self.buf[offset : offset + len(data)] = data

    def write_bool(self, val: bool, offset: int) -> None:
        if not isinstance(val, bool):
            raise TypeError(f"Expected bool, got {type(val)}")
        self.write_inline(offset, b"\x01" if val else b"\x00")

    def _write_primitive(
        self, fmt: str, val: typing.Any, offset: int, subtype: str
    ) -> None:
        if val is None:
            raise TypeError(
                f"None value not allowed for primitive subtype {subtype}"
            )
        if "int" in subtype and not isinstance(val, int):
            raise TypeError(f"Expected int, got {type(val)}")
        if "float" in subtype and not isinstance(val, (int, float)):
            raise TypeError(f"Expected float, got {type(val)}")
        try:
            self.write_inline(offset, struct.pack(fmt, val))
        except (struct.error, OverflowError) as e:
            raise OverflowError(
                f"Value {val} out of range for {subtype}"
            ) from e

    def write_int8(self, val: int, offset: int) -> None:
        self._write_primitive("<b", val, offset, "int8")

    def write_uint8(self, val: int, offset: int) -> None:
        self._write_primitive("<B", val, offset, "uint8")

    def write_int16(self, val: int, offset: int) -> None:
        self._write_primitive("<h", val, offset, "int16")

    def write_uint16(self, val: int, offset: int) -> None:
        self._write_primitive("<H", val, offset, "uint16")

    def write_int32(self, val: int, offset: int) -> None:
        self._write_primitive("<i", val, offset, "int32")

    def write_uint32(self, val: int, offset: int) -> None:
        self._write_primitive("<I", val, offset, "uint32")

    def write_int64(self, val: int, offset: int) -> None:
        self._write_primitive("<q", val, offset, "int64")

    def write_uint64(self, val: int, offset: int) -> None:
        self._write_primitive("<Q", val, offset, "uint64")

    def write_float32(self, val: float, offset: int) -> None:
        self._write_primitive("<f", val, offset, "float32")

    def write_float64(self, val: float, offset: int) -> None:
        self._write_primitive("<d", val, offset, "float64")

    def write_handle(
        self,
        val: int | None,
        offset: int,
        obj_type: int = 0,
        rights: int = 0x80000000,
    ) -> None:
        if val is None or val == 0:
            self.write_inline(offset, struct.pack("<I", 0))
        else:
            hdl_val = int(val)
            if hdl_val < 0 or hdl_val > 0xFFFFFFFF:
                raise OverflowError(f"Handle value {hdl_val} out of range")
            self.write_inline(offset, struct.pack("<I", 0xFFFFFFFF))
            self.handles.append((0, hdl_val, obj_type, rights, 0))

    def write_string(self, val: str | None, offset: int) -> None:
        if val is None:
            self.write_inline(offset, struct.pack("<Q", 0))
            self.write_inline(offset + 8, struct.pack("<Q", 0))
        else:
            encoded = val.encode("utf-8")
            self.write_inline(offset, struct.pack("<Q", len(encoded)))
            self.write_inline(offset + 8, struct.pack("<Q", 0xFFFFFFFFFFFFFFFF))
            self.ool_tasks.append(lambda: self.write_ool_bytes(encoded))

    def write_ool_bytes(self, data: bytes) -> None:
        pad_mod = len(data) % 8
        if pad_mod != 0:
            data = data + b"\x00" * (8 - pad_mod)
        self.buf.extend(data)

    def write_vector(
        self,
        val: typing.Sequence[typing.Any] | None,
        offset: int,
        element_encoder_fn: typing.Callable[[typing.Any, "Encoder", int], None],
        element_size: int,
    ) -> None:
        if val is None:
            self.write_inline(offset, struct.pack("<Q", 0))
            self.write_inline(offset + 8, struct.pack("<Q", 0))
        else:
            self.write_inline(offset, struct.pack("<Q", len(val)))
            self.write_inline(offset + 8, struct.pack("<Q", 0xFFFFFFFFFFFFFFFF))
            self.ool_tasks.append(
                lambda: self.write_ool_vector(
                    val, element_encoder_fn, element_size
                )
            )

    def write_ool_vector(
        self,
        val: typing.Sequence[typing.Any],
        element_encoder_fn: typing.Callable[[typing.Any, "Encoder", int], None],
        element_size: int,
    ) -> None:
        parent_tasks = self.ool_tasks
        self.ool_tasks = []

        pad_mod = len(self.buf) % 8
        if pad_mod != 0:
            self.buf.extend(b"\x00" * (8 - pad_mod))
        start_offset = len(self.buf)
        self.buf.extend(b"\x00" * (element_size * len(val)))
        for i, item in enumerate(val):
            element_encoder_fn(item, self, start_offset + i * element_size)

        pad_mod = len(self.buf) % 8
        if pad_mod != 0:
            self.buf.extend(b"\x00" * (8 - pad_mod))

        i = 0
        while i < len(self.ool_tasks):
            self.ool_tasks[i]()
            i += 1
        self.ool_tasks = parent_tasks

    def write_array(
        self,
        val: typing.Sequence[typing.Any],
        offset: int,
        element_encoder_fn: typing.Callable[[typing.Any, "Encoder", int], None],
        element_size: int,
        count: int,
    ) -> None:
        if len(val) != count:
            raise ValueError(
                f"Array length {len(val)} does not match expected count {count}"
            )
        for i in range(count):
            element_encoder_fn(val[i], self, offset + i * element_size)

    def write_optional_struct(
        self,
        val: typing.Any,
        offset: int,
        encode_fn: typing.Callable[[typing.Any, "Encoder", int], None],
        inline_size: int,
    ) -> None:
        if val is None:
            self.write_inline(offset, struct.pack("<Q", 0))
        else:
            self.write_inline(offset, struct.pack("<Q", 0xFFFFFFFFFFFFFFFF))
            self.ool_tasks.append(
                lambda: self.write_ool_struct(val, encode_fn, inline_size)
            )

    def write_ool_struct(
        self,
        val: typing.Any,
        encode_fn: typing.Callable[[typing.Any, "Encoder", int], None],
        inline_size: int,
    ) -> None:
        parent_tasks = self.ool_tasks
        self.ool_tasks = []

        pad_mod = len(self.buf) % 8
        if pad_mod != 0:
            self.buf.extend(b"\x00" * (8 - pad_mod))
        start_offset = len(self.buf)
        self.buf.extend(b"\x00" * inline_size)
        encode_fn(val, self, start_offset)

        pad_mod = len(self.buf) % 8
        if pad_mod != 0:
            self.buf.extend(b"\x00" * (8 - pad_mod))

        i = 0
        while i < len(self.ool_tasks):
            self.ool_tasks[i]()
            i += 1
        self.ool_tasks = parent_tasks

    def write_envelope(
        self,
        val: typing.Any,
        env_offset: int,
        encode_fn: typing.Callable[[typing.Any, "Encoder", int], None],
        inline_size: int,
        is_inline: bool,
    ) -> None:
        if val is None:
            self.write_inline(env_offset, b"\x00" * 8)
            return

        if is_inline:
            temp_encoder = Encoder()
            encode_fn(val, temp_encoder, 0)
            temp_bytes = bytes(temp_encoder.buf)
            if len(temp_bytes) < 4:
                temp_bytes = temp_bytes + b"\x00" * (4 - len(temp_bytes))
            elif len(temp_bytes) > 4:
                raise RuntimeError("Inlined value exceeds 4 bytes")
            self.write_inline(env_offset, temp_bytes)
            self.handles.extend(temp_encoder.handles)
            self.write_inline(
                env_offset + 4, struct.pack("<H", len(temp_encoder.handles))
            )
            self.write_inline(env_offset + 6, struct.pack("<H", 1))
        else:
            self.write_inline(env_offset, struct.pack("<I", 0))
            self.write_inline(env_offset + 4, struct.pack("<H", 0))
            self.write_inline(env_offset + 6, struct.pack("<H", 0))
            self.ool_tasks.append(
                lambda: self.write_ool_envelope_member(
                    val, encode_fn, inline_size, env_offset
                )
            )

    def write_ool_envelope_member(
        self,
        val: typing.Any,
        encode_fn: typing.Callable[[typing.Any, "Encoder", int], None],
        inline_size: int,
        env_offset: int,
    ) -> None:
        parent_tasks = self.ool_tasks
        self.ool_tasks = []

        pad_mod = len(self.buf) % 8
        if pad_mod != 0:
            self.buf.extend(b"\x00" * (8 - pad_mod))
        start_offset = len(self.buf)
        self.buf.extend(b"\x00" * inline_size)

        start_handles = len(self.handles)
        encode_fn(val, self, start_offset)

        pad_mod = len(self.buf) % 8
        if pad_mod != 0:
            self.buf.extend(b"\x00" * (8 - pad_mod))

        i = 0
        while i < len(self.ool_tasks):
            self.ool_tasks[i]()
            i += 1

        total_size = len(self.buf) - start_offset
        total_handles = len(self.handles) - start_handles

        self.write_inline(env_offset, struct.pack("<I", total_size))
        self.write_inline(env_offset + 4, struct.pack("<H", total_handles))

        self.ool_tasks = parent_tasks


def encode_struct(
    obj: typing.Any,
    encode_fn: typing.Callable[[typing.Any, Encoder, int], None],
    inline_size: int,
) -> tuple[bytes, list[typing.Any]]:
    encoder = Encoder()
    encoder.buf.extend(b"\x00" * inline_size)
    encode_fn(obj, encoder, 0)
    i = 0
    while i < len(encoder.ool_tasks):
        encoder.ool_tasks[i]()
        i += 1
    # Pad final buffer to 8 bytes alignment
    pad_mod = len(encoder.buf) % 8
    if pad_mod != 0:
        encoder.buf.extend(b"\x00" * (8 - pad_mod))
    return bytes(encoder.buf), encoder.handles
