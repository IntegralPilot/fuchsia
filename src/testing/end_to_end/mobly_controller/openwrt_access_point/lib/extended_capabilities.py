# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from enum import IntEnum, unique


@unique
class ExtendedCapability(IntEnum):
    """Minimal version of ExtendedCapability for OpenWrt."""

    WNM_SLEEP_MODE = 17
    BSS_TRANSITION = 19


def _offsets(ext_cap_offset: ExtendedCapability) -> tuple[int, int]:
    """For given capability, return the byte and bit offsets within the field."""
    byte_offset = ext_cap_offset // 8
    bit_offset = ext_cap_offset % 8
    return byte_offset, bit_offset


class ExtendedCapabilities:
    """Minimal version of ExtendedCapabilities for OpenWrt."""

    def __init__(self, ext_cap: bytearray = bytearray()):
        self._ext_cap = ext_cap

    def _capability_advertised(self, ext_cap: ExtendedCapability) -> bool:
        byte_offset, bit_offset = _offsets(ext_cap)
        if len(self._ext_cap) > byte_offset:
            if self._ext_cap[byte_offset] & 2**bit_offset > 0:
                return True
        return False

    @property
    def bss_transition(self) -> bool:
        return self._capability_advertised(ExtendedCapability.BSS_TRANSITION)

    @property
    def wnm_sleep_mode(self) -> bool:
        return self._capability_advertised(ExtendedCapability.WNM_SLEEP_MODE)
