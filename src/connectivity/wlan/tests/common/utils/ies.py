# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Helpers for readings IEs for antlion tests of WLAN core.
"""

from dataclasses import dataclass
from enum import IntEnum


class ElementId(IntEnum):
    SSID = 0
    ELEMENT_ID_EXTENTION_PRESENT = 255


def read_ssid(ies_bytes: bytes) -> str | None:
    i = 0
    while True:
        id = ies_bytes[i]
        length = ies_bytes[i + 1]
        if id == ElementId.SSID:
            return ies_bytes[i + 2 : i + 2 + length].decode("utf-8")
        if i + 2 + length >= len(ies_bytes):
            break
        i += 2 + length
    return None


@dataclass
class Ie:
    element_id: int
    length: int
    element_id_extension: int | None
    information: bytes

    def __init__(
        self,
        element_id: int,
        length: int,
        information: bytes,
        element_id_extension: int | None = None,
    ) -> None:
        self.element_id = element_id
        self.length = length
        self.information = information
        self.element_id_extension = element_id_extension

    @staticmethod
    def read_ies(ies_bytes: bytes) -> list["Ie"]:
        ies = []
        i = 0
        while True:
            element_id = ies_bytes[i]
            element_id_extension = None
            length = ies_bytes[i + 1]
            information_start = i + 2
            information_end = i + 2 + length
            if element_id == ElementId.ELEMENT_ID_EXTENTION_PRESENT:
                element_id_extension = ies_bytes[i + 2]
                information_start = i + 3

            ies.append(
                Ie(
                    element_id=element_id,
                    element_id_extension=element_id_extension,
                    length=length,
                    information=ies_bytes[information_start:information_end],
                )
            )
            if information_end >= len(ies_bytes):
                break
            i = information_end
        return ies
