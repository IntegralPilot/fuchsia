# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
import string
from enum import Enum, StrEnum, auto, unique

import fidl_fuchsia_wlan_policy as f_wlan_policy
from antlion.controllers.ap_lib import hostapd_constants


class SecurityModeInt(int, Enum):
    """Possible values for hostapd's "wpa" config option.

    The int value is a bit field that can enable WPA and/or WPA2.

    bit0 = enable WPA defined by IEEE 802.11i/D3.0
    bit1 = enable RNA (WPA2) defined by IEEE 802.11i/RSN
    bit2 = enable WAPI (rejected/withdrawn)
    bit3 = enable OSEN (ENT)
    """

    WEP = 0
    WPA1 = 1
    WPA2 = 2
    WPA3 = 2  # same as wpa2 and wpa2/wpa3; distinguished by wpa_key_mgmt
    MIXED = 3  # applies to wpa/wpa2 and wpa/wpa2/wpa3; distinguished by wpa_key_mgmt
    ENT = 8

    def __str__(self) -> str:
        return str(self.value)


@unique
class KeyManagement(StrEnum):
    SAE = "SAE"
    WPA_PSK = "WPA-PSK"
    WPA_PSK_SAE = "WPA-PSK SAE"
    ENT = "WPA-EAP"


# TODO(http://b/286584981): This is currently only being used for OpenWRT.
# Investigate whether we can replace KeyManagement with OpenWRTEncryptionMode.
@unique
class OpenWRTEncryptionMode(StrEnum):
    """Combination of Wi-Fi encryption mode and ciphers.

    Only used by OpenWRT.

    Besides the encryption mode, the encryption option also specifies the group and peer
    ciphers to use. To override the cipher, the value of encryption must be given in the
    form "mode+cipher". This enum contains all possible combinations.

    See https://openwrt.org/docs/guide-user/network/wifi/basic#encryption_modes.
    """

    NONE = "none"
    """No authentication, no ciphers"""
    SAE = "sae"
    """WPA3 Personal (SAE) using CCMP cipher"""
    SAE_MIXED = "sae-mixed"
    """WPA2/WPA3 Personal (PSK/SAE) mixed mode using CCMP cipher"""
    PSK2_TKIP_CCMP = "psk2+tkip+ccmp"
    """WPA2 Personal (PSK) using TKIP and CCMP ciphers"""
    PSK2_TKIP_AES = "psk2+tkip+aes"
    """WPA2 Personal (PSK) using TKIP and AES ciphers"""
    PSK2_TKIP = "psk2+tkip"
    """WPA2 Personal (PSK) using TKIP cipher"""
    PSK2_CCMP = "psk2+ccmp"
    """WPA2 Personal (PSK) using CCMP cipher"""
    PSK2_AES = "psk2+aes"
    """WPA2 Personal (PSK) using AES cipher"""
    PSK2 = "psk2"
    """WPA2 Personal (PSK) using CCMP cipher"""
    PSK_TKIP_CCMP = "psk+tkip+ccmp"
    """WPA Personal (PSK) using TKIP and CCMP ciphers"""
    PSK_TKIP_AES = "psk+tkip+aes"
    """WPA Personal (PSK) using TKIP and AES ciphers"""
    PSK_TKIP = "psk+tkip"
    """WPA Personal (PSK) using TKIP cipher"""
    PSK_CCMP = "psk+ccmp"
    """WPA Personal (PSK) using CCMP cipher"""
    PSK_AES = "psk+aes"
    """WPA Personal (PSK) using AES cipher"""
    PSK = "psk"
    """WPA Personal (PSK) using CCMP cipher"""
    PSK_MIXED_TKIP_CCMP = "psk-mixed+tkip+ccmp"
    """WPA/WPA2 Personal (PSK) mixed mode using TKIP and CCMP ciphers"""
    PSK_MIXED_TKIP_AES = "psk-mixed+tkip+aes"
    """WPA/WPA2 Personal (PSK) mixed mode using TKIP and AES ciphers"""
    PSK_MIXED_TKIP = "psk-mixed+tkip"
    """WPA/WPA2 Personal (PSK) mixed mode using TKIP cipher"""
    PSK_MIXED_CCMP = "psk-mixed+ccmp"
    """WPA/WPA2 Personal (PSK) mixed mode using CCMP cipher"""
    PSK_MIXED_AES = "psk-mixed+aes"
    """WPA/WPA2 Personal (PSK) mixed mode using AES cipher"""
    PSK_MIXED = "psk-mixed"
    """WPA/WPA2 Personal (PSK) mixed mode using CCMP cipher"""
    WEP = "wep"
    """defaults to “open system” authentication aka wep+open using RC4 cipher"""
    WEP_OPEN = "wep+open"
    """“open system” authentication using RC4 cipher"""
    WEP_SHARED = "wep+shared"
    """“shared key” authentication using RC4 cipher"""
    WPA3 = "wpa3"
    """WPA3 Enterprise using CCMP cipher"""
    WPA3_MIXED = "wpa3-mixed"
    """WPA3/WPA2 Enterprise using CCMP cipher"""
    WPA2_TKIP_CCMP = "wpa2+tkip+ccmp"
    """WPA2 Enterprise using TKIP and CCMP ciphers"""
    WPA2_TKIP_AES = "wpa2+tkip+aes"
    """WPA2 Enterprise using TKIP and AES ciphers"""
    WPA2_CCMP = "wpa2+ccmp"
    """WPA2 Enterprise using CCMP cipher"""
    WPA2_AES = "wpa2+aes'"
    """WPA2 Enterprise using AES cipher"""
    WPA2 = "wpa2"
    """WPA2 Enterprise using CCMP cipher"""
    WPA2_TKIP = "wpa2+tkip"
    """WPA2 Enterprise using TKIP cipher"""
    WPA_TKIP_CCMP = "wpa+tkip+ccmp"
    """WPA Enterprise using TKIP and CCMP ciphers"""
    WPA_TKIP_AES = "wpa+tkip+aes"
    """WPA Enterprise using TKIP and AES ciphers"""
    WPA_CCMP = "wpa+ccmp"
    """WPA Enterprise using CCMP cipher"""
    WPA_AES = "wpa+aes"
    """WPA Enterprise using AES cipher"""
    WPA_TKIP = "wpa+tkip"
    """WPA Enterprise using TKIP cipher"""
    WPA = "wpa"
    """WPA Enterprise using CCMP cipher"""
    WPA_MIXED_TKIP_CCMP = "wpa-mixed+tkip+ccmp"
    """WPA/WPA2 Enterprise mixed mode using TKIP and CCMP ciphers"""
    WPA_MIXED_TKIP_AES = "wpa-mixed+tkip+aes"
    """WPA/WPA2 Enterprise mixed mode using TKIP and AES ciphers"""
    WPA_MIXED_TKIP = "wpa-mixed+tkip"
    """WPA/WPA2 Enterprise mixed mode using TKIP cipher"""
    WPA_MIXED_CCMP = "wpa-mixed+ccmp"
    """WPA/WPA2 Enterprise mixed mode using CCMP cipher"""
    WPA_MIXED_AES = "wpa-mixed+aes"
    """WPA/WPA2 Enterprise mixed mode using AES cipher"""
    WPA_MIXED = "wpa-mixed"
    """WPA/WPA2 Enterprise mixed mode using CCMP cipher"""
    OWE = "owe"
    """Opportunistic Wireless Encryption (OWE) using CCMP cipher"""


@unique
class SecurityMode(StrEnum):
    OPEN = auto()
    WEP = auto()
    WPA = auto()
    WPA2 = auto()
    WPA_WPA2 = auto()
    WPA3 = auto()
    WPA2_WPA3 = auto()
    WPA_WPA2_WPA3 = auto()
    ENT = auto()

    def security_mode_int(self) -> SecurityModeInt:
        match self:
            case SecurityMode.OPEN:
                raise TypeError("Open security doesn't have a SecurityModeInt")
            case SecurityMode.WEP:
                return SecurityModeInt.WEP
            case SecurityMode.WPA:
                return SecurityModeInt.WPA1
            case SecurityMode.WPA2:
                return SecurityModeInt.WPA2
            case SecurityMode.WPA_WPA2:
                return SecurityModeInt.MIXED
            case SecurityMode.WPA3:
                return SecurityModeInt.WPA3
            case SecurityMode.WPA2_WPA3:
                return SecurityModeInt.WPA3
            case SecurityMode.WPA_WPA2_WPA3:
                return SecurityModeInt.MIXED
            case SecurityMode.ENT:
                return SecurityModeInt.ENT

    def key_management(self) -> KeyManagement | None:
        match self:
            case SecurityMode.OPEN:
                return None
            case SecurityMode.WEP:
                return None
            case SecurityMode.WPA:
                return KeyManagement.WPA_PSK
            case SecurityMode.WPA2:
                return KeyManagement.WPA_PSK
            case SecurityMode.WPA_WPA2:
                return KeyManagement.WPA_PSK
            case SecurityMode.WPA3:
                return KeyManagement.SAE
            case SecurityMode.WPA2_WPA3:
                return KeyManagement.WPA_PSK_SAE
            case SecurityMode.WPA_WPA2_WPA3:
                return KeyManagement.WPA_PSK_SAE
            case SecurityMode.ENT:
                return KeyManagement.ENT

    def fuchsia_security_type(self) -> f_wlan_policy.SecurityType:
        match self:
            case SecurityMode.OPEN:
                return f_wlan_policy.SecurityType.NONE
            case SecurityMode.WEP:
                return f_wlan_policy.SecurityType.WEP
            case SecurityMode.WPA:
                return f_wlan_policy.SecurityType.WPA
            case SecurityMode.WPA2 | SecurityMode.WPA_WPA2:
                return f_wlan_policy.SecurityType.WPA2
            case (
                SecurityMode.WPA3
                | SecurityMode.WPA2_WPA3
                | SecurityMode.WPA_WPA2_WPA3
            ):
                return f_wlan_policy.SecurityType.WPA3
            case SecurityMode.ENT:
                raise NotImplementedError(
                    f'Fuchsia has not implemented support for security mode "{self}"'
                )

    def is_wpa3(self) -> bool:
        match self:
            case SecurityMode.OPEN:
                return False
            case SecurityMode.WEP:
                return False
            case SecurityMode.WPA:
                return False
            case SecurityMode.WPA2:
                return False
            case SecurityMode.WPA_WPA2:
                return False
            case SecurityMode.WPA3:
                return True
            case SecurityMode.WPA2_WPA3:
                return True
            case SecurityMode.WPA_WPA2_WPA3:
                return True
            case SecurityMode.ENT:
                return False
        raise TypeError("Unknown security mode")


class Security(object):
    """The Security class for hostapd representing some of the security
    settings that are allowed in hostapd.  If needed more can be added.
    """

    def __init__(
        self,
        security_mode: SecurityMode = SecurityMode.OPEN,
        password: str | None = None,
        wpa_cipher: str | None = hostapd_constants.WPA_DEFAULT_CIPHER,
        wpa2_cipher: str | None = hostapd_constants.WPA2_DEFAULT_CIPER,
        wpa_group_rekey: int = hostapd_constants.WPA_GROUP_KEY_ROTATION_TIME,
        wpa_strict_rekey: bool = hostapd_constants.WPA_STRICT_REKEY_DEFAULT,
        wep_default_key: int = hostapd_constants.WEP_DEFAULT_KEY,
        radius_server_ip: str | None = None,
        radius_server_port: int | None = None,
        radius_server_secret: str | None = None,
    ) -> None:
        """Gather all of the security settings for WPA-PSK.  This could be
           expanded later.

        Args:
            security_mode: Type of security mode.
            password: The PSK or passphrase for the security mode.
            wpa_cipher: The cipher to be used for wpa.
                        Options: TKIP, CCMP, TKIP CCMP
                        Default: TKIP
            wpa2_cipher: The cipher to be used for wpa2.
                         Options: TKIP, CCMP, TKIP CCMP
                         Default: CCMP
            wpa_group_rekey: How often to refresh the GTK regardless of network
                             changes.
                             Options: An integer in seconds, None
                             Default: 600 seconds
            wpa_strict_rekey: Whether to do a group key update when client
                              leaves the network or not.
                              Options: True, False
                              Default: True
            wep_default_key: The wep key number to use when transmitting.
            radius_server_ip: Radius server IP for Enterprise auth.
            radius_server_port: Radius server port for Enterprise auth.
            radius_server_secret: Radius server secret for Enterprise auth.
        """
        self.security_mode = security_mode
        self.wpa_cipher = wpa_cipher
        self.wpa2_cipher = wpa2_cipher
        self.wpa_group_rekey = wpa_group_rekey
        self.wpa_strict_rekey = wpa_strict_rekey
        self.wep_default_key = wep_default_key
        self.radius_server_ip = radius_server_ip
        self.radius_server_port = radius_server_port
        self.radius_server_secret = radius_server_secret
        if password:
            if self.security_mode is SecurityMode.WEP:
                if len(password) in hostapd_constants.WEP_STR_LENGTH:
                    self.password: str | None = f'"{password}"'
                elif len(password) in hostapd_constants.WEP_HEX_LENGTH and all(
                    c in string.hexdigits for c in password
                ):
                    self.password = password
                else:
                    raise ValueError(
                        "WEP key must be a hex string of %s characters"
                        % hostapd_constants.WEP_HEX_LENGTH
                    )
            else:
                if (
                    len(password) < hostapd_constants.MIN_WPA_PSK_LENGTH
                    or len(password) > hostapd_constants.MAX_WPA_PSK_LENGTH
                ):
                    raise ValueError(
                        "Password must be a minumum of %s characters and a maximum of %s"
                        % (
                            hostapd_constants.MIN_WPA_PSK_LENGTH,
                            hostapd_constants.MAX_WPA_PSK_LENGTH,
                        )
                    )
                else:
                    self.password = password
        else:
            self.password = None

    def __str__(self) -> str:
        return self.security_mode

    def generate_dict(self) -> dict[str, str | int]:
        """Returns: an ordered dictionary of settings"""
        if self.security_mode is SecurityMode.OPEN:
            return {}

        settings: dict[str, str | int] = collections.OrderedDict()

        if self.security_mode is SecurityMode.WEP:
            settings["wep_default_key"] = self.wep_default_key
            if self.password is not None:
                settings[f"wep_key{self.wep_default_key}"] = self.password
        elif self.security_mode == SecurityMode.ENT:
            if self.radius_server_ip is not None:
                settings["auth_server_addr"] = self.radius_server_ip
            if self.radius_server_port is not None:
                settings["auth_server_port"] = self.radius_server_port
            if self.radius_server_secret is not None:
                settings[
                    "auth_server_shared_secret"
                ] = self.radius_server_secret
            settings["wpa_key_mgmt"] = hostapd_constants.ENT_KEY_MGMT
            settings["ieee8021x"] = hostapd_constants.IEEE8021X
            settings["wpa"] = hostapd_constants.WPA2
        else:
            settings["wpa"] = self.security_mode.security_mode_int().value
            if self.password:
                if len(self.password) == hostapd_constants.MAX_WPA_PSK_LENGTH:
                    settings["wpa_psk"] = self.password
                else:
                    settings["wpa_passphrase"] = self.password
            # For wpa, wpa/wpa2, and wpa/wpa2/wpa3, add wpa_pairwise
            if self.wpa_cipher and (
                self.security_mode is SecurityMode.WPA
                or self.security_mode is SecurityMode.WPA_WPA2
                or self.security_mode is SecurityMode.WPA_WPA2_WPA3
            ):
                settings["wpa_pairwise"] = self.wpa_cipher
            # For wpa/wpa2, wpa2, wpa3, and wpa2/wpa3, and wpa/wpa2, wpa3, add rsn_pairwise
            if self.wpa2_cipher and (
                self.security_mode is SecurityMode.WPA_WPA2
                or self.security_mode is SecurityMode.WPA2
                or self.security_mode is SecurityMode.WPA2_WPA3
                or self.security_mode is SecurityMode.WPA3
            ):
                settings["rsn_pairwise"] = self.wpa2_cipher
            # Add wpa_key_mgmt based on security mode string
            wpa_key_mgmt = self.security_mode.key_management()
            if wpa_key_mgmt is not None:
                settings["wpa_key_mgmt"] = str(wpa_key_mgmt)
            if self.wpa_group_rekey:
                settings["wpa_group_rekey"] = self.wpa_group_rekey
            if self.wpa_strict_rekey:
                settings[
                    "wpa_strict_rekey"
                ] = hostapd_constants.WPA_STRICT_REKEY

        return settings
