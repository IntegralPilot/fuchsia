# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# THIS FILE IS GENERATED. DO NOT EDIT MANUALLY.
# To update, edit uci_allow_list.yaml and run:
#   python3 src/testing/end_to_end/mobly_controller/openwrt_access_point/lib/generator/generate_uci_options.py
# Schema: wireless.wifi-device.json
# Tag: v25.12.4

from typing import Literal, TypedDict


class UciRadioOptions(TypedDict, total=False):
    """Generated from OpenWrt JSON schema.

    Only includes attributes specified in the allow-list.
    """

    frag: int
    """Fragmentation threshold"""

    beacon_int: int
    """Set the beacon interval. This is the time interval between beacon frames, measured in units of 1.024 ms. hostapd permits this to be set between 15 and 65535. This option only has an effect on ap and adhoc wifi-ifaces"""

    rts: int
    """Override the RTS/CTS threshold"""

    require_mode: Literal["n", "ac", "ax"]
    """Sets the minimum client capability level mode that connecting clients must support to be allowed to connect"""

    supported_rates: list[int]
    """Set the supported data rates. Each supported rate is measured in kb/s. This option only has an effect on ap and adhoc wifi-ifaces. This must be a superset of the rates set in basic_rate. The minimum basic rate should also be the minimum supported rate. It is recommended to use the cell_density option instead"""

    basic_rates: list[int]
    """Set the supported basic rates. Each basic_rate is measured in kb/s. This option only has an effect on ap and adhoc wifi-ifaces. """

    ieee80211h: bool
    """This enables radar detection and DFS support"""

    ieee80211d: bool
    """Enables IEEE 802.11d country IE (information element) advertisement in beacon and probe response frames. This IE contains the country code and channel/power map. Requires country"""

    spectrum_mgmt_required: bool
    """Set Spectrum Management subfield in the Capability Information field"""

    local_pwr_constraint: int
    """Add Power Constraint element to Beacon and Probe Response frame"""
