#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
SYSTEM_INFO_CMD = "ubus call system board"


class OpenWrtWifiSecurity:
    # Used by OpenWrt AP
    WPA_PSK_DEFAULT = "psk"
    WPA_PSK_CCMP = "psk+ccmp"
    WPA_PSK_TKIP = "psk+tkip"
    WPA_PSK_TKIP_AND_CCMP = "psk+tkip+ccmp"
    WPA2_PSK_DEFAULT = "psk2"
    WPA2_PSK_CCMP = "psk2+ccmp"
    WPA2_PSK_TKIP = "psk2+tkip"
    WPA2_PSK_TKIP_AND_CCMP = "psk2+tkip+ccmp"


class OpenWrtWifiSetting:
    IFACE_2G = 2
    IFACE_5G = 3


class OpenWrtModelMap:
    NETGEAR_R8000 = ("radio2", "radio1")
