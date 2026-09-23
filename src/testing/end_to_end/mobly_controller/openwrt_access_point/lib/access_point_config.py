# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data classes and enums for Wi-Fi Configuration."""

from __future__ import annotations

import dataclasses
import enum
import random
import re
import string
from typing import Literal, Optional, Protocol, TypeAlias

import fidl_fuchsia_wlan_policy as f_wlan_policy
from honeydew.typing.custom_types import MacAddress
from openwrt_access_point.lib.hostapd_options import HostapdOptions
from openwrt_access_point.lib.uci_bss_options import UciBssOptions
from openwrt_access_point.lib.uci_radio_options import UciRadioOptions


class Pmf(enum.IntEnum):
    """Protected Management Frames (PMF) configuration."""

    DISABLED = 0
    OPTIONAL = 1
    REQUIRED = 2


class Band(enum.StrEnum):
    """The Wi-Fi frequency band."""

    BAND_2G = "2G"
    BAND_5G = "5G"

    @property
    def default_channel(self) -> int:
        """Returns the default channel number for this band."""
        match self:
            case Band.BAND_2G:
                return DEFAULT_2G_CHANNEL.number
            case Band.BAND_5G:
                return DEFAULT_5G_CHANNEL.number
            case _:
                raise ValueError(f"Unsupported band: {self}")

    @property
    def default_bss_channel(self) -> BssChannel:
        """Returns the default BssChannel configuration for this band."""
        match self:
            case Band.BAND_2G:
                return DEFAULT_2G_CHANNEL
            case Band.BAND_5G:
                return DEFAULT_5G_CHANNEL
            case _:
                raise ValueError(f"Unsupported band: {self}")


# TODO(https://fxbug.dev/487800358): Create to_fidl function.


class Security(Protocol):
    """Protocol to abstract Wi-Fi security modes and their UCI mapping."""

    @property
    def uci_encryption(self) -> str:
        """Returns the string used for OpenWrt's wireless 'encryption' setting."""
        ...

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        """Returns the Fuchsia WLAN Policy FIDL SecurityType corresponding to this mode."""
        ...

    @property
    def pmf_support(self) -> Pmf:
        """Returns the PMF support level."""
        ...


@dataclasses.dataclass(frozen=True)
class SecurityOpen:
    pmf_support: Literal[Pmf.DISABLED] = Pmf.DISABLED

    @property
    def uci_encryption(self) -> str:
        return "none"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.NONE


@dataclasses.dataclass(frozen=True)
class SecurityOwe:
    pmf_support: Literal[Pmf.REQUIRED] = Pmf.REQUIRED

    @property
    def uci_encryption(self) -> str:
        return "owe"

    # TODO(https://fxbug.dev/528397348): OWE isn't in WLAN policy's SecurityType yet, use OWE here when it is added.
    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        raise NotImplementedError(
            "OWE is not yet supported in fuchsia.wlan.policy.SecurityType (https://fxbug.dev/528397348)"
        )


@dataclasses.dataclass(frozen=True)
class SecurityOweTransition:
    open_bssid: MacAddress
    owe_bssid: MacAddress
    pmf_support: Literal[Pmf.REQUIRED] = Pmf.REQUIRED

    @property
    def uci_encryption(self) -> str:
        return "none"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.NONE


@dataclasses.dataclass(frozen=True)
class SecurityWpa:
    cipher: Literal["ccmp", "tkip", "ccmp+tkip"] | None = None
    pmf_support: Literal[Pmf.DISABLED] = Pmf.DISABLED

    @property
    def uci_encryption(self) -> str:
        if self.cipher is None:
            return "psk"
        return f"psk+{self.cipher}"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.WPA


@dataclasses.dataclass(frozen=True)
class SecurityWpa2:
    cipher: Literal["ccmp", "tkip", "ccmp+tkip"] | None = None
    pmf_support: Pmf = Pmf.DISABLED

    @property
    def uci_encryption(self) -> str:
        if self.cipher is None:
            return "psk2"
        return f"psk2+{self.cipher}"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.WPA2


@dataclasses.dataclass(frozen=True)
class SecurityWpa3:
    cipher: Literal["ccmp", "ccmp+tkip"] | None = None
    pmf_support: Literal[Pmf.REQUIRED] = Pmf.REQUIRED

    @property
    def uci_encryption(self) -> str:
        if self.cipher is None:
            return "sae"
        return f"sae+{self.cipher}"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.WPA3


@dataclasses.dataclass(frozen=True)
class SecurityWpaWpa2Mixed:
    cipher: Literal["ccmp", "tkip", "ccmp+tkip"] | None = None
    pmf_support: Pmf = Pmf.DISABLED

    @property
    def uci_encryption(self) -> str:
        if self.cipher is None:
            return "psk-mixed"
        return f"psk-mixed+{self.cipher}"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.WPA2


@dataclasses.dataclass(frozen=True)
class SecurityWpa2Wpa3Mixed:
    cipher: Literal["ccmp", "ccmp+tkip"] | None = None
    pmf_support: Literal[Pmf.OPTIONAL, Pmf.REQUIRED] = Pmf.OPTIONAL

    @property
    def uci_encryption(self) -> str:
        if self.cipher is None:
            return "sae-mixed"
        return f"sae-mixed+{self.cipher}"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.WPA3


@dataclasses.dataclass(frozen=True)
class SecurityWep:
    auth_mode: Literal["open", "shared"] = "open"
    pmf_support: Literal[Pmf.DISABLED] = Pmf.DISABLED

    @property
    def uci_encryption(self) -> str:
        return f"wep+{self.auth_mode}"

    def to_fidl_wlan_policy(self) -> f_wlan_policy.SecurityType:
        return f_wlan_policy.SecurityType.WEP


Bandwidth: TypeAlias = Literal[20, 40, 80, 160, 320]
WifiModeStr: TypeAlias = Literal["HT", "VHT", "HE", "EHT", "NOHT"]


class PhyMode(Protocol):
    """Protocol to abstract Wi-Fi modes and their associated bandwidth settings."""

    @property
    def mode_str(self) -> WifiModeStr:
        """Returns the base Wi-Fi standard string."""
        ...

    @property
    def bandwidth(self) -> Bandwidth:
        """Returns the channel bandwidth in MHz."""
        ...

    @property
    def uci_htmode(self) -> str:
        """Returns the string used for OpenWrt's wireless 'htmode' setting."""
        ...


@dataclasses.dataclass(frozen=True)
class LegacyMode:
    """802.11a/b/g (No HT) implementation."""

    @property
    def mode_str(self) -> WifiModeStr:
        return "NOHT"

    @property
    def bandwidth(self) -> Bandwidth:
        return 20

    @property
    def uci_htmode(self) -> str:
        return "NOHT"


@dataclasses.dataclass(frozen=True)
class HtMode:
    """802.11n (HT) implementation, handling optional extension channels."""

    bw: Literal[20, 40]
    extension: Optional[Literal["+", "-"]] = None

    def __post_init__(self) -> None:
        if self.bw == 40 and not self.extension:
            raise ValueError("HT40 requires extension channel (+ or -)")

    @property
    def mode_str(self) -> WifiModeStr:
        return "HT"

    @property
    def bandwidth(self) -> Bandwidth:
        return self.bw

    @property
    def uci_htmode(self) -> str:
        ext = self.extension or ""
        return f"HT{self.bw}{ext}"


@dataclasses.dataclass(frozen=True)
class VhtMode:
    """802.11ac (VHT) implementation."""

    bw: Literal[20, 40, 80, 160]

    @property
    def mode_str(self) -> WifiModeStr:
        return "VHT"

    @property
    def bandwidth(self) -> Bandwidth:
        return self.bw

    @property
    def uci_htmode(self) -> str:
        return f"VHT{self.bw}"


@dataclasses.dataclass(frozen=True)
class HeMode:
    """802.11ax (HE) implementation."""

    bw: Literal[20, 40, 80, 160]

    @property
    def mode_str(self) -> WifiModeStr:
        return "HE"

    @property
    def bandwidth(self) -> Bandwidth:
        return self.bw

    @property
    def uci_htmode(self) -> str:
        return f"HE{self.bw}"


@dataclasses.dataclass(frozen=True)
class EhtMode:
    """802.11be (EHT) implementation."""

    bw: Literal[20, 40, 80, 160, 320]

    @property
    def mode_str(self) -> WifiModeStr:
        return "EHT"

    @property
    def bandwidth(self) -> Bandwidth:
        return self.bw

    @property
    def uci_htmode(self) -> str:
        return f"EHT{self.bw}"


@dataclasses.dataclass(frozen=True)
class BssChannel:
    """Represents a Wi-Fi channel configuration using the PhyMode protocol.

    Attributes:
        band: The Wi-Fi frequency band (e.g., 2G, 5G).
        number: The primary channel number.
        phy_mode: An implementation of PhyMode.
    """

    band: Band
    number: int
    phy_mode: PhyMode


DEFAULT_2G_CHANNEL = BssChannel(Band.BAND_2G, 1, HtMode(bw=40, extension="+"))
DEFAULT_5G_CHANNEL = BssChannel(Band.BAND_5G, 36, VhtMode(bw=80))

# see http://go/openwrt-one-dfs-bypass
DFS_BYPASS_COUNTRY_CODE = "XF"

US_DFS_CHANNELS = [
    52,
    56,
    60,
    64,
    100,
    104,
    108,
    112,
    116,
    120,
    124,
    128,
    132,
    136,
    140,
    144,
]


@dataclasses.dataclass
class BssSettings:
    """Settings for a BSS (Multiple SSIDs on the same radio).

    Attributes:
        ssid: The Service Set Identifier (network name)
        security: The security encryption protocol
        password: The passphrase or key for the network
        custom_uci_options: UciBssOptions or Mapping to set on the BSS.
    """

    ssid: str
    security: Security
    password: str | None = None
    hidden: bool = False
    custom_uci_options: UciBssOptions = dataclasses.field(
        default_factory=lambda: UciBssOptions()
    )

    @property
    def name(self) -> str:
        """
        Returns a UCI-safe section name based on the SSID.
        Example: "Guest Wi-Fi!" -> "bss_guest_wi_fi"
        """
        # 1. Convert to lowercase
        normalized = self.ssid.lower()
        # 2. Replace non-alphanumeric characters with underscores
        normalized = re.sub(r"[^a-z0-9]+", "_", normalized)

        # 3. Strip leading/trailing underscores and prefix it
        # Prefixing (e.g., 'bss_') prevents issues if an SSID starts with a digit
        safe_name = normalized.strip("_")

        return f"bss_{safe_name}"


@dataclasses.dataclass(frozen=True)
class CapabilitySelection:
    """Defines the selection of Wi-Fi capabilities."""

    mode: Literal["DEFAULT", "DISABLED", "CUSTOM"]
    capabilities: list[str] = dataclasses.field(default_factory=list)

    @classmethod
    def DEFAULT(cls) -> CapabilitySelection:
        """Use OpenWrt system defaults."""
        return cls(mode="DEFAULT")

    @classmethod
    def DISABLED(cls) -> CapabilitySelection:
        """Explicitly disable all capabilities."""
        return cls(mode="DISABLED")

    @classmethod
    def CUSTOM(cls, capabilities: list[str]) -> CapabilitySelection:
        """Provide a custom list of capabilities to enable."""
        return cls(mode="CUSTOM", capabilities=capabilities)


# TODO(https://fxbug.dev/489258440): Make channel required param and provide easy way to use
# default 2g/5g channels.
@dataclasses.dataclass
class RadioConfig:
    """Configuration required to set up a specific radio on an Access Point.

    Attributes:
        channel: The specific channel within the band
        bss_settings: The settings for all additional bss
        country: The country code for the radio (default: "US")
        n_capabilities: Selection of 802.11n capabilities.
        ac_capabilities: Selection of 802.11ac capabilities.
        custom_uci_options: Arbitrary UCI options to set on the radio.
        custom_hostapd_options: Arbitrary hostapd options to pass through via UCI list hostapd_options.
    """

    channel: BssChannel
    bss_settings: list[BssSettings] | None = None
    country: str = "US"
    n_capabilities: CapabilitySelection = CapabilitySelection.DEFAULT()
    ac_capabilities: CapabilitySelection = CapabilitySelection.DEFAULT()
    custom_uci_options: UciRadioOptions = dataclasses.field(
        default_factory=lambda: UciRadioOptions()
    )
    custom_hostapd_options: HostapdOptions = dataclasses.field(
        default_factory=lambda: HostapdOptions()
    )

    @classmethod
    def generate(
        cls,
        channel: BssChannel,
        bss_settings: list[BssSettings] | None = None,
        country: str = "US",
        n_capabilities: CapabilitySelection = CapabilitySelection.DEFAULT(),
        ac_capabilities: CapabilitySelection = CapabilitySelection.DEFAULT(),
        custom_uci_options: UciRadioOptions | None = None,
        custom_hostapd_options: HostapdOptions | None = None,
    ) -> RadioConfig:
        """Creates a RadioConfig object with the specified channel and BSS settings.

        Args:
            channel: The Wi-Fi channel configuration.
            bss_settings: Optional list of additional BSS settings.
            country: The country code for the radio.
            n_capabilities: Selection of 802.11n capabilities.
            ac_capabilities: Selection of 802.11ac capabilities.
            custom_uci_options: Structured UciRadioOptions to set on the radio.
            custom_hostapd_options: Structured HostapdOptions to set on the radio.

        Returns:
            A RadioConfig object.
        """
        if bss_settings is None:
            bss_settings = []
        if custom_uci_options is None:
            custom_uci_options = {}
        if custom_hostapd_options is None:
            custom_hostapd_options = {}

        return cls(
            channel=channel,
            bss_settings=bss_settings,
            country=country,
            n_capabilities=n_capabilities,
            ac_capabilities=ac_capabilities,
            custom_uci_options=custom_uci_options,
            custom_hostapd_options=custom_hostapd_options,
        )


@dataclasses.dataclass
class AccessPointConfig:
    """Configuration required to set up an Access Point.

    Attributes:
        radios: A list of RadioConfig objects defining the radios to configure.
    """

    radios: list[RadioConfig]

    @classmethod
    def random_string(cls, length: int = 8) -> str:
        """Generates a random string of letters and digits.

        Args:
            length: The length of the random string.

        Returns:
            A random string.
        """
        return "".join(
            random.choices(string.ascii_letters + string.digits, k=length)
        )

    @classmethod
    def random_hex_string(cls, length: int = 10) -> str:
        """Generates a random hexadecimal string.

        Args:
            length: The length of the random string.

        Returns:
            A random hexadecimal string.
        """
        return "".join(random.choices(string.hexdigits, k=length))

    @classmethod
    def generate(
        cls,
        radios: list["RadioConfig"] | None = None,
    ) -> AccessPointConfig:
        """Creates an AccessPointConfig containing the specified radio configurations.

        Args:
            radios: A list of RadioConfig objects.

        Returns:
            An AccessPointConfig object.
        """
        if radios is None:
            radios = []

        return cls(radios=radios)
