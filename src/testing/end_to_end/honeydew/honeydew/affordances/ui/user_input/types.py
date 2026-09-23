# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""user-interface testing types"""

from dataclasses import dataclass


@dataclass(frozen=True)
class Coordinate:
    x: int
    y: int


@dataclass(frozen=True)
class Size:
    width: int
    height: int


@dataclass(frozen=True)
class Pixel:
    """Pixel object with r,g,b,a values in the 0-255 range"""

    red: int
    green: int
    blue: int
    alpha: int = 255
