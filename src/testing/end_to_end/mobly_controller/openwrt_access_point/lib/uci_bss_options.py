# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# THIS FILE IS GENERATED. DO NOT EDIT MANUALLY.
# To update, edit uci_allow_list.yaml and run:
#   python3 src/testing/end_to_end/mobly_controller/openwrt_access_point/lib/generator/generate_uci_options.py
# Schema: wireless.wifi-iface.json
# Tag: v25.12.4

from typing import Literal, TypedDict


class UciBssOptions(TypedDict, total=False):
    """Generated from OpenWrt JSON schema.

    Only includes attributes specified in the allow-list.
    """

    preamble: bool
    """Short Preamble"""

    dtim_period: int
    """Set the DTIM (delivery traffic information message) period. There will be one DTIM per this many beacon frames. This may be set between 1 and 255. This option only has an effect on ap wifi-ifaces."""

    vendor_elements: list[str]
    """Additional vendor specific elements for Beacon and Probe Response frames"""

    uapsd_advertisement_enabled: bool
    """WMM-PS Unscheduled Automatic Power Save Delivery [U-APSD]"""

    bss_load_update_period: int
    """BSS Load update period (in BUs)"""

    chan_util_avg_period: int
    """Channel utilization averaging period (in BUs)"""

    rrm_beacon_report: bool
    """Enable beacon report via radio measurements"""

    rrm_neighbor_report: bool
    """Enable neighbor report via radio measurements"""

    bss_transition: bool
    """BSS Transition Management"""

    wnm_sleep_mode: bool
    """WNM-Sleep Mode (extended sleep mode for stations)"""

    ieee80211w: Literal[0, 1, 2]
    """Enables MFP (802.11w) support (0 = disabled, 1 = optional, 2 = required). Requires the 'full' version of wpad/hostapd and support from the Wi-Fi driver"""

    max_num_sta: int
    """Maximum number of stations allowed in station table"""
