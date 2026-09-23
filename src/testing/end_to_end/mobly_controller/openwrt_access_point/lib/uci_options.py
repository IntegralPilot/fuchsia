# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


# Data Rates
class SupportedRates:
    OFDM = [6000, 9000, 12000, 18000, 24000, 36000, 48000, 54000]
    CCK = [1000, 2000, 5500, 11000]
    CCK_AND_OFDM = CCK + OFDM


class BasicRate:
    OFDM_ONLY = [6000, 12000, 24000]
    CCK = [1000, 2000]
    CCK_AND_OFDM = [1000, 2000, 5500, 11000]


class VendorElements:
    CORRECT_LENGTH = "dd0411223301"
    TOO_SHORT_LENGTH = "dd0311223301"
    TOO_LONG_LENGTH = "dd0511223301"
    ZERO_LENGTH_WITH_DATA = "dd0011223301"
    ZERO_LENGTH_WITHOUT_DATA = "dd00"
    SIMILAR_TO_WPA = "dd040050f203"
