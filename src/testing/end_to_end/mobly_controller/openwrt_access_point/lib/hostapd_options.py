# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


from typing import TypedDict


class HostapdOptions(TypedDict, total=False):
    """A TypedDict for common hostapd options passed via UCI list hostapd_options.

    'total=False' means all keys are optional. Add more options as needed.

    Attributes:
        bss_load_update_period: BSS load update period in seconds.
        chan_util_avg_period: Channel utilization average period.
        wmm_ac_*: WMM parameters for different access categories (BK, BE, VI, VO).
        assocresp_elements: Vendor-specific information elements for Association Response.
        country3: 3rd byte of country code (e.g., 'O' for outdoor).
    """

    bss_load_update_period: int
    chan_util_avg_period: int

    # WMM parameters
    wmm_ac_bk_cwmin: int
    wmm_ac_bk_cwmax: int
    wmm_ac_bk_aifs: int
    wmm_ac_bk_txop_limit: int
    wmm_ac_bk_acm: bool

    wmm_ac_be_cwmin: int
    wmm_ac_be_cwmax: int
    wmm_ac_be_aifs: int
    wmm_ac_be_txop_limit: int
    wmm_ac_be_acm: bool

    wmm_ac_vi_cwmin: int
    wmm_ac_vi_cwmax: int
    wmm_ac_vi_aifs: int
    wmm_ac_vi_txop_limit: int
    wmm_ac_vi_acm: bool

    wmm_ac_vo_cwmin: int
    wmm_ac_vo_cwmax: int
    wmm_ac_vo_aifs: int
    wmm_ac_vo_txop_limit: int
    wmm_ac_vo_acm: bool

    # Vendor IEs
    assocresp_elements: str

    # Regulatory
    country3: str

    obss_interval: int

    """
    If set non-zero, require stations to perform scans of overlapping
    channels to test for stations which would be affected by 40 MHz traffic.
    This parameter sets the interval in seconds between these scans. Setting this
    to non-zero allows 2.4 GHz band AP to move dynamically to a 40 MHz channel if
    no co-existence issues with neighboring devices are found.
    """


# WMM
class WmmParams:
    DEFAULT_11B: HostapdOptions = {
        "wmm_ac_bk_cwmin": 5,
        "wmm_ac_bk_cwmax": 10,
        "wmm_ac_bk_aifs": 7,
        "wmm_ac_bk_txop_limit": 0,
        "wmm_ac_be_aifs": 3,
        "wmm_ac_be_cwmin": 5,
        "wmm_ac_be_cwmax": 7,
        "wmm_ac_be_txop_limit": 0,
        "wmm_ac_vi_aifs": 2,
        "wmm_ac_vi_cwmin": 4,
        "wmm_ac_vi_cwmax": 5,
        "wmm_ac_vi_txop_limit": 188,
        "wmm_ac_vo_aifs": 2,
        "wmm_ac_vo_cwmin": 3,
        "wmm_ac_vo_cwmax": 4,
        "wmm_ac_vo_txop_limit": 102,
    }

    DEFAULT_PHYS_11A_11G_11N_11AC_DEFAULT_PARAMS: HostapdOptions = {
        "wmm_ac_bk_cwmin": 4,
        "wmm_ac_bk_cwmax": 10,
        "wmm_ac_bk_aifs": 7,
        "wmm_ac_bk_txop_limit": 0,
        "wmm_ac_be_aifs": 3,
        "wmm_ac_be_cwmin": 4,
        "wmm_ac_be_cwmax": 10,
        "wmm_ac_be_txop_limit": 0,
        "wmm_ac_vi_aifs": 2,
        "wmm_ac_vi_cwmin": 3,
        "wmm_ac_vi_cwmax": 4,
        "wmm_ac_vi_txop_limit": 94,
        "wmm_ac_vo_aifs": 2,
        "wmm_ac_vo_cwmin": 2,
        "wmm_ac_vo_cwmax": 3,
        "wmm_ac_vo_txop_limit": 47,
    }

    NON_DEFAULT: HostapdOptions = {
        "wmm_ac_bk_cwmin": 5,
        "wmm_ac_bk_cwmax": 9,
        "wmm_ac_bk_aifs": 3,
        "wmm_ac_bk_txop_limit": 94,
        "wmm_ac_be_aifs": 2,
        "wmm_ac_be_cwmin": 2,
        "wmm_ac_be_cwmax": 8,
        "wmm_ac_be_txop_limit": 0,
        "wmm_ac_vi_aifs": 1,
        "wmm_ac_vi_cwmin": 7,
        "wmm_ac_vi_cwmax": 10,
        "wmm_ac_vi_txop_limit": 47,
        "wmm_ac_vo_aifs": 1,
        "wmm_ac_vo_cwmin": 6,
        "wmm_ac_vo_cwmax": 10,
        "wmm_ac_vo_txop_limit": 94,
    }

    DEGRADED_VO: HostapdOptions = {
        "wmm_ac_bk_cwmin": 7,
        "wmm_ac_bk_cwmax": 15,
        "wmm_ac_bk_aifs": 2,
        "wmm_ac_bk_txop_limit": 0,
        "wmm_ac_be_aifs": 2,
        "wmm_ac_be_cwmin": 7,
        "wmm_ac_be_cwmax": 15,
        "wmm_ac_be_txop_limit": 0,
        "wmm_ac_vi_aifs": 2,
        "wmm_ac_vi_cwmin": 7,
        "wmm_ac_vi_cwmax": 15,
        "wmm_ac_vi_txop_limit": 94,
        "wmm_ac_vo_aifs": 10,
        "wmm_ac_vo_cwmin": 7,
        "wmm_ac_vo_cwmax": 15,
        "wmm_ac_vo_txop_limit": 47,
    }

    DEGRADED_VI: HostapdOptions = {
        "wmm_ac_bk_cwmin": 7,
        "wmm_ac_bk_cwmax": 15,
        "wmm_ac_bk_aifs": 2,
        "wmm_ac_bk_txop_limit": 0,
        "wmm_ac_be_aifs": 2,
        "wmm_ac_be_cwmin": 7,
        "wmm_ac_be_cwmax": 15,
        "wmm_ac_be_txop_limit": 0,
        "wmm_ac_vi_aifs": 10,
        "wmm_ac_vi_cwmin": 7,
        "wmm_ac_vi_cwmax": 15,
        "wmm_ac_vi_txop_limit": 94,
        "wmm_ac_vo_aifs": 2,
        "wmm_ac_vo_cwmin": 7,
        "wmm_ac_vo_cwmax": 15,
        "wmm_ac_vo_txop_limit": 47,
    }

    IMPROVE_BE: HostapdOptions = {
        "wmm_ac_bk_cwmin": 7,
        "wmm_ac_bk_cwmax": 15,
        "wmm_ac_bk_aifs": 10,
        "wmm_ac_bk_txop_limit": 0,
        "wmm_ac_be_aifs": 2,
        "wmm_ac_be_cwmin": 7,
        "wmm_ac_be_cwmax": 15,
        "wmm_ac_be_txop_limit": 0,
        "wmm_ac_vi_aifs": 10,
        "wmm_ac_vi_cwmin": 7,
        "wmm_ac_vi_cwmax": 15,
        "wmm_ac_vi_txop_limit": 94,
        "wmm_ac_vo_aifs": 10,
        "wmm_ac_vo_cwmin": 7,
        "wmm_ac_vo_cwmax": 15,
        "wmm_ac_vo_txop_limit": 47,
    }

    IMPROVE_BK: HostapdOptions = {
        "wmm_ac_bk_cwmin": 7,
        "wmm_ac_bk_cwmax": 15,
        "wmm_ac_bk_aifs": 2,
        "wmm_ac_bk_txop_limit": 0,
        "wmm_ac_be_aifs": 10,
        "wmm_ac_be_cwmin": 7,
        "wmm_ac_be_cwmax": 15,
        "wmm_ac_be_txop_limit": 0,
        "wmm_ac_vi_aifs": 10,
        "wmm_ac_vi_cwmin": 7,
        "wmm_ac_vi_cwmax": 15,
        "wmm_ac_vi_txop_limit": 94,
        "wmm_ac_vo_aifs": 10,
        "wmm_ac_vo_cwmin": 7,
        "wmm_ac_vo_cwmax": 15,
        "wmm_ac_vo_txop_limit": 47,
    }


class WmmAcm:
    BK: HostapdOptions = {"wmm_ac_bk_acm": True}
    BE: HostapdOptions = {"wmm_ac_be_acm": True}
    VI: HostapdOptions = {"wmm_ac_vi_acm": True}
    VO: HostapdOptions = {"wmm_ac_vo_acm": True}


class AssocRespIe:
    CORRECT_LENGTH: HostapdOptions = {"assocresp_elements": "dd0411223301"}
    ZERO_LENGTH_WITHOUT_DATA: HostapdOptions = {"assocresp_elements": "dd00"}


class Country3:
    ALL: HostapdOptions = {"country3": "0x20"}
    OUTDOOR: HostapdOptions = {"country3": "0x4f"}
    INDOOR: HostapdOptions = {"country3": "0x49"}
    NONCOUNTRY: HostapdOptions = {"country3": "0x58"}
    GLOBAL: HostapdOptions = {"country3": "0x04"}
