# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from antlion.controllers.ap_lib.hostapd_constants import BandType
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode as HostapdSecurityMode,
)
from openwrt_access_point.lib.access_point_config import (
    Band,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWep,
    SecurityWpa,
    SecurityWpa2,
    SecurityWpa2Wpa3Mixed,
    SecurityWpa3,
    SecurityWpaWpa2Mixed,
)

# TODO(b/489927930): This file should be removed after OpenWRT migration is complete.


class AccessPointConfigMapper:
    @staticmethod
    def to_hostapd_band(band: Band) -> BandType:
        """Maps Band to BandType"""
        band_map = {
            Band.BAND_2G: BandType.BAND_2G,
            Band.BAND_5G: BandType.BAND_5G,
        }
        return band_map[band]

    @staticmethod
    def to_hostapd_security(
        security: Security,
    ) -> HostapdSecurityMode:
        """Maps Security or SecurityMode to HostapdSecurityMode."""
        if isinstance(security, SecurityOpen):
            return HostapdSecurityMode.OPEN
        if isinstance(security, SecurityWpa):
            return HostapdSecurityMode.WPA
        if isinstance(security, SecurityWpa2):
            return HostapdSecurityMode.WPA2
        if isinstance(security, SecurityWpa3):
            return HostapdSecurityMode.WPA3
        if isinstance(security, SecurityWpaWpa2Mixed):
            return HostapdSecurityMode.WPA_WPA2
        if isinstance(security, SecurityWpa2Wpa3Mixed):
            return HostapdSecurityMode.WPA2_WPA3
        if isinstance(security, SecurityWep):
            return HostapdSecurityMode.WEP

        raise ValueError(f"Unsupported security mode: {security}")

    @staticmethod
    def to_hostapd_cipher(cipher: str) -> str:
        """Maps OpenWrt-style cipher string to hostapd format."""
        mapping = {
            "tkip": "TKIP",
            "ccmp": "CCMP",
            "ccmp+tkip": "TKIP CCMP",
            "tkip+ccmp": "TKIP CCMP",
        }
        if cipher not in mapping:
            raise ValueError(f"Unsupported cipher: {cipher}")
        return mapping[cipher]

    @staticmethod
    def to_hostapd_n_cap(cap: str) -> object:
        """Maps a generic capability string to its hostapd_constants equivalent."""
        from antlion.controllers.ap_lib import hostapd_constants
        from openwrt_access_point.lib import capabilities

        mapping = {
            capabilities.N_CAPABILITY_LDPC: hostapd_constants.N_CAPABILITY_LDPC,
            capabilities.N_CAPABILITY_SHORT_GI_20: hostapd_constants.N_CAPABILITY_SGI20,
            capabilities.N_CAPABILITY_SHORT_GI_40: hostapd_constants.N_CAPABILITY_SGI40,
            capabilities.N_CAPABILITY_TX_STBC: hostapd_constants.N_CAPABILITY_TX_STBC,
            capabilities.N_CAPABILITY_RX_STBC1: hostapd_constants.N_CAPABILITY_RX_STBC1,
            capabilities.N_CAPABILITY_MAX_AMSDU_7935: hostapd_constants.N_CAPABILITY_MAX_AMSDU_7935,
            capabilities.N_CAPABILITY_HT40_PLUS: hostapd_constants.N_CAPABILITY_HT40_PLUS,
            capabilities.N_CAPABILITY_HT20: hostapd_constants.N_CAPABILITY_HT20,
            capabilities.N_CAPABILITY_40_INTOLERANT: hostapd_constants.N_CAPABILITY_40_INTOLERANT,
            capabilities.N_CAPABILITY_SMPS_STATIC: hostapd_constants.N_CAPABILITY_SMPS_STATIC,
            capabilities.N_CAPABILITY_DSSS_CCK_40: hostapd_constants.N_CAPABILITY_DSSS_CCK_40,
        }
        return mapping.get(cap, cap)

    @staticmethod
    def to_hostapd_ac_cap(cap: str) -> object:
        """Maps a generic capability string to its hostapd_constants equivalent."""
        from antlion.controllers.ap_lib import hostapd_constants
        from openwrt_access_point.lib import capabilities

        mapping = {
            capabilities.AC_CAPABILITY_MAX_MPDU_7991: hostapd_constants.AC_CAPABILITY_MAX_MPDU_7991,
            capabilities.AC_CAPABILITY_MAX_MPDU_11454: hostapd_constants.AC_CAPABILITY_MAX_MPDU_11454,
            capabilities.AC_CAPABILITY_RXLDPC: hostapd_constants.AC_CAPABILITY_RXLDPC,
            capabilities.AC_CAPABILITY_SHORT_GI_80: hostapd_constants.AC_CAPABILITY_SHORT_GI_80,
            capabilities.AC_CAPABILITY_SHORT_GI_160: hostapd_constants.AC_CAPABILITY_SHORT_GI_160,
            capabilities.AC_CAPABILITY_TX_STBC_2BY1: hostapd_constants.AC_CAPABILITY_TX_STBC_2BY1,
            capabilities.AC_CAPABILITY_RX_STBC_1: hostapd_constants.AC_CAPABILITY_RX_STBC_1,
            capabilities.AC_CAPABILITY_RX_STBC_12: hostapd_constants.AC_CAPABILITY_RX_STBC_12,
            capabilities.AC_CAPABILITY_RX_STBC_123: hostapd_constants.AC_CAPABILITY_RX_STBC_123,
            capabilities.AC_CAPABILITY_RX_STBC_1234: hostapd_constants.AC_CAPABILITY_RX_STBC_1234,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP0: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP0,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP1: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP1,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP2: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP2,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP3: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP3,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP4: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP4,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP5: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP5,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP6: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP6,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7: hostapd_constants.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7,
            capabilities.AC_CAPABILITY_VHT_LINK_ADAPT2: hostapd_constants.AC_CAPABILITY_VHT_LINK_ADAPT2,
            capabilities.AC_CAPABILITY_VHT_LINK_ADAPT3: hostapd_constants.AC_CAPABILITY_VHT_LINK_ADAPT3,
            capabilities.AC_CAPABILITY_VHT160: hostapd_constants.AC_CAPABILITY_VHT160,
            capabilities.AC_CAPABILITY_VHT160_80PLUS80: hostapd_constants.AC_CAPABILITY_VHT160_80PLUS80,
            capabilities.AC_CAPABILITY_RX_ANTENNA_PATTERN: hostapd_constants.AC_CAPABILITY_RX_ANTENNA_PATTERN,
            capabilities.AC_CAPABILITY_TX_ANTENNA_PATTERN: hostapd_constants.AC_CAPABILITY_TX_ANTENNA_PATTERN,
        }
        return mapping.get(cap, cap)

    @staticmethod
    def to_legacy_params(radio_config: RadioConfig) -> dict[str, object]:
        """Maps RadioConfig to legacy hostapd options for legacy AP support."""
        mapping = {
            "country_ie": "ieee80211d",
            "frag": "fragm_threshold",
            "rts": "rts_threshold",
        }
        hostapd_options: dict[str, object] = {}

        # 1. Map country
        hostapd_options["country_code"] = radio_config.country

        # 2. Map custom_uci_options on radio
        for k, v in radio_config.custom_uci_options.items():
            hostapd_key = mapping.get(k, k)
            if k in ("supported_rates", "basic_rates") and isinstance(v, list):
                hostapd_options[hostapd_key] = " ".join(
                    str(int(x) // 100) for x in v
                )
            elif isinstance(v, list):
                hostapd_options[hostapd_key] = " ".join(str(x) for x in v)
            else:
                hostapd_options[hostapd_key] = v

        # 3. Map custom_uci_options on first BSS (assuming single BSS for compliance tests)
        if radio_config.bss_settings:
            bss = radio_config.bss_settings[0]
            for k, v in bss.custom_uci_options.items():
                hostapd_key = mapping.get(k, k)
                if isinstance(v, list):
                    hostapd_options[hostapd_key] = " ".join(str(x) for x in v)
                else:
                    hostapd_options[hostapd_key] = v

        # 4. Merge with custom_hostapd_options
        hostapd_options.update(radio_config.custom_hostapd_options)

        return hostapd_options
