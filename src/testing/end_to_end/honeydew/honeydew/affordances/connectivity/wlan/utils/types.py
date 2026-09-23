# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data types used by wlan affordance."""

from __future__ import annotations

import enum
from dataclasses import dataclass
from typing import Protocol

import fidl_fuchsia_wlan_device_service as f_wlan_device_service
import fidl_fuchsia_wlan_ieee80211 as f_wlan_ieee80211
import fidl_fuchsia_wlan_policy as f_wlan_policy
from honeydew.typing.custom_types import MacAddress as _MacAddress

MacAddress = _MacAddress

# Length of a pre-shared key (PSK) used as a password.
_PSK_LENGTH = 64


@dataclass(frozen=True)
class NetworkConfig:
    """Network information used to establish a connection.

    Defined by https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.policy/types.fidl
    """

    ssid: str
    security_type: f_wlan_policy.SecurityType
    credential_type: str
    credential_value: str

    @staticmethod
    def from_fidl(fidl: f_wlan_policy.NetworkConfig) -> NetworkConfig:
        """Parse from a fuchsia.wlan.policy/NetworkConfig."""
        assert fidl.id_ is not None, f"{fidl!r} missing id"
        assert fidl.credential is not None, f"{fidl!r} missing credential"
        identifier = NetworkIdentifier.from_fidl(fidl.id_)
        credential = Credential.from_fidl(fidl.credential)
        return NetworkConfig(
            ssid=identifier.ssid,
            security_type=identifier.security_type,
            credential_type=credential.type(),
            credential_value=credential.value(),
        )

    def to_fidl(self) -> f_wlan_policy.NetworkConfig:
        """Convert to equivalent FIDL."""
        return f_wlan_policy.NetworkConfig(
            id_=NetworkIdentifier(self.ssid, self.security_type).to_fidl(),
            credential=Credential.from_password(
                self.credential_value
            ).to_fidl(),
        )

    def __lt__(self, other: NetworkConfig) -> bool:
        return self.ssid < other.ssid


@dataclass(frozen=True)
class NetworkIdentifier:
    """Combination of ssid and the security type.

    Primary means of distinguishing between available networks.
    Defined by https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.policy/types.fidl
    """

    ssid: str
    security_type: f_wlan_policy.SecurityType

    @staticmethod
    def from_fidl(fidl: f_wlan_policy.NetworkIdentifier) -> NetworkIdentifier:
        """Parse from a fuchsia.wlan.policy/NetworkIdentifier."""

        return NetworkIdentifier(
            ssid=bytes(fidl.ssid).decode("utf-8"),
            security_type=f_wlan_policy.SecurityType(fidl.type_),
        )

    def to_fidl(self) -> f_wlan_policy.NetworkIdentifier:
        """Convert to a fuchsia.wlan.policy/NetworkIdentifier."""
        return f_wlan_policy.NetworkIdentifier(
            ssid=list(self.ssid.encode("utf-8")),
            type_=self.security_type,
        )

    def __lt__(self, other: NetworkIdentifier) -> bool:
        return self.ssid < other.ssid


class Credential(Protocol):
    """Information used to verify access to a target network."""

    def type(self) -> str:
        """Type of credential."""

    def value(self) -> str:
        """Value of the credential, or empty string if not applicable."""

    def to_fidl(self) -> f_wlan_policy.Credential:
        """Convert to a fuchsia.wlan.policy/Credential."""

    @staticmethod
    def from_password(password: str | None) -> Credential:
        """Parse a password into a Credential.

        Args:
            password: String password, pre-shared key in hex form with length 64, or
                None/empty to represent open.

        Return:
            A fuchsia.wlan.policy/Credential union object.
        """
        if not password:
            return CredentialNone()
        elif len(password) == _PSK_LENGTH:
            return CredentialPsk(password)
        else:
            return CredentialPassword(password)

    @staticmethod
    def from_fidl(fidl: f_wlan_policy.Credential) -> Credential:
        """Parse a fuchsia.wlan.policy/Credential."""
        if fidl.none is not None:
            return CredentialNone()
        if fidl.password is not None:
            return CredentialPassword(bytes(fidl.password).decode("utf-8"))
        if fidl.psk is not None:
            return CredentialPsk(bytes(fidl.psk).hex())
        raise TypeError(
            f"Unknown value for fuchsia.wlan.policy/Credential: {fidl}"
        )


class CredentialNone(Credential):
    """Credentials to connect to an unprotected network."""

    def type(self) -> str:
        return "None"

    def value(self) -> str:
        return ""

    def to_fidl(self) -> f_wlan_policy.Credential:
        cred = f_wlan_policy.Credential(none=f_wlan_policy.Empty())
        return cred


@dataclass(frozen=True)
class CredentialPassword(Credential):
    """Credentials to connect to an password protected network."""

    password: str
    """Plaintext password."""

    def type(self) -> str:
        return "Password"

    def value(self) -> str:
        return self.password

    def to_fidl(self) -> f_wlan_policy.Credential:
        cred = f_wlan_policy.Credential(
            password=list(self.password.encode("utf-8"))
        )
        return cred


@dataclass(frozen=True)
class CredentialPsk(Credential):
    """Credentials to connect to an network using a pre-shared key."""

    psk: str
    """Hash representation of the network passphrase."""

    def type(self) -> str:
        return "Psk"

    def value(self) -> str:
        return self.psk

    def to_fidl(self) -> f_wlan_policy.Credential:
        cred = f_wlan_policy.Credential(psk=list(bytes.fromhex(self.psk)))
        return cred


@dataclass(frozen=True)
class NetworkState:
    """Information about a network's current connections and attempts.

    Defined by https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.policy/client_provider.fidl
    """

    network_identifier: NetworkIdentifier
    connection_state: f_wlan_policy.ConnectionState
    disconnect_status: f_wlan_policy.DisconnectStatus | None

    @staticmethod
    def from_fidl(fidl: f_wlan_policy.NetworkState) -> NetworkState:
        """Parse from a fuchsia.wlan.policy/NetworkState."""
        assert fidl.id_ is not None, f"{fidl!r} missing id"
        assert fidl.state is not None, f"{fidl!r} missing state"

        return NetworkState(
            network_identifier=NetworkIdentifier.from_fidl(fidl.id_),
            connection_state=f_wlan_policy.ConnectionState(fidl.state),
            disconnect_status=(
                f_wlan_policy.DisconnectStatus(fidl.status)
                if fidl.status
                else None
            ),
        )

    def __lt__(self, other: NetworkState) -> bool:
        return self.network_identifier < other.network_identifier


@dataclass(frozen=True)
class ClientStateSummary:
    """Information about the current client state for the device.

    This includes if the device will attempt to connect to access points
    (when applicable), any existing connections and active connection attempts
    and their outcomes.
    Defined by https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.policy/client_provider.fidl
    """

    state: f_wlan_policy.WlanClientState
    networks: list[NetworkState]

    @staticmethod
    def from_fidl(
        fidl: f_wlan_policy.ClientStateSummary,
    ) -> ClientStateSummary:
        """Parse from a fuchsia.wlan.policy/ClientStateSummary."""
        assert fidl.networks is not None, f"{fidl!r} missing networks"
        assert fidl.state is not None, f"{fidl!r} missing state"
        return ClientStateSummary(
            state=f_wlan_policy.WlanClientState(fidl.state),
            networks=[NetworkState.from_fidl(n) for n in fidl.networks],
        )

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ClientStateSummary):
            return NotImplemented
        return self.state == other.state and sorted(self.networks) == sorted(
            other.networks
        )


@dataclass(frozen=True)
class WlanInterfaces:
    """WLAN interfaces separated by device type and keyed by MAC address."""

    client: dict[MacAddress, f_wlan_device_service.QueryIfaceResponse]
    """Client WLAN interfaces keyed by MAC address."""
    ap: dict[MacAddress, f_wlan_device_service.QueryIfaceResponse]
    """AP WLAN interfaces keyed by MAC address."""


class InformationElementType(enum.IntEnum):
    """Information Element type.

    As defined by IEEE 802.11-1997 Section 7.3.2 and further expanded by
    802.11d, 802.11g, 802.11h, and 802.11i.

    https://www.oreilly.com/library/view/80211-wireless-networks/0596100523/ch04.html#wireless802dot112-CHP-4-TABLE-7
    """

    SSID = 0
    # Types 1-255 are not implemented. Only implement a new type if it is being used.


class BssDescriptionParser:
    """BssDescription with parsed information elements."""

    @staticmethod
    def ssid(bss_description: f_wlan_ieee80211.BssDescription) -> str | None:
        """Parse information elements for SSID."""
        ies = bytes(bss_description.ies)
        i = 0
        while i < len(ies):
            if not len(ies) > i + 1:
                raise TypeError(
                    "Invalid information element; requires at least 2 bytes for "
                    f"Element ID and Length, got {len(ies) - i}"
                )

            element = int(ies[i])
            length = int(ies[i + 1])
            i += 2

            try:
                ie_type = InformationElementType(int(element))
            except ValueError:
                # Type not implemented. It's okay to skip
                i += length
                continue

            match ie_type:
                case InformationElementType.SSID:
                    try:
                        return ies[i : i + length].decode("utf-8")
                    except UnicodeDecodeError:
                        # ssid is not valid UTF-8; fallback to counting bytes
                        return f"<ssid-{length}>"
                case _:
                    raise TypeError(
                        f"Unsupported InformationElementType: {ie_type}"
                    )

        return None


class CountryCode:
    """Country codes used for configuring WLAN."""

    _code: bytes

    def __init__(self, code: str | bytes | bytearray) -> None:
        if isinstance(code, str):
            code_bytes = code.encode("ascii")
        else:
            code_bytes = bytes(code)

        if len(code_bytes) != 2:
            raise ValueError(
                f"Expected exactly 2 ASCII bytes, got {len(code_bytes)}"
            )

        self._code = code_bytes

    def __bytes__(self) -> bytes:
        return self._code

    def __str__(self) -> str:
        return self._code.decode("ascii")

    def __repr__(self) -> str:
        return f"CountryCode('{self}')"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, CountryCode):
            return False
        return self._code == other._code

    def __hash__(self) -> int:
        return hash(self._code)


KNOWN_COUNTRY_CODES = {
    "AUSTRIA": CountryCode("AT"),
    "AUSTRALIA": CountryCode("AU"),
    "BELGIUM": CountryCode("BE"),
    "BULGARIA": CountryCode("BG"),
    "CANADA": CountryCode("CA"),
    "SWITZERLAND": CountryCode("CH"),
    "CHILE": CountryCode("CL"),
    "COLOMBIA": CountryCode("CO"),
    "CYPRUS": CountryCode("CY"),
    "CZECHIA": CountryCode("CZ"),
    "GERMANY": CountryCode("DE"),
    "DENMARK": CountryCode("DK"),
    "ESTONIA": CountryCode("EE"),
    "GREECE_EU": CountryCode("EL"),
    "SPAIN": CountryCode("ES"),
    "FINLAND": CountryCode("FI"),
    "FRANCE": CountryCode("FR"),
    "UNITED_KINGDOM_OF_GREAT_BRITAIN": CountryCode("GB"),
    "GREECE": CountryCode("GR"),
    "CROATIA": CountryCode("HR"),
    "HUNGARY": CountryCode("HU"),
    "IRELAND": CountryCode("IE"),
    "INDIA": CountryCode("IN"),
    "ICELAND": CountryCode("IS"),
    "ITALY": CountryCode("IT"),
    "JAPAN": CountryCode("JP"),
    "KOREA": CountryCode("KR"),
    "LIECHTENSTEIN": CountryCode("LI"),
    "LITHUANIA": CountryCode("LT"),
    "LUXEMBOURG": CountryCode("LU"),
    "LATVIA": CountryCode("LV"),
    "MALTA": CountryCode("MT"),
    "MEXICO": CountryCode("MX"),
    "NETHERLANDS": CountryCode("NL"),
    "NORWAY": CountryCode("NO"),
    "NEW_ZEALAND": CountryCode("NZ"),
    "PERU": CountryCode("PE"),
    "POLAND": CountryCode("PL"),
    "PORTUGAL": CountryCode("PT"),
    "ROMANIA": CountryCode("RO"),
    "SWEDEN": CountryCode("SE"),
    "SINGAPORE": CountryCode("SG"),
    "SLOVENIA": CountryCode("SI"),
    "SLOVAKIA": CountryCode("SK"),
    "TURKEY": CountryCode("TR"),
    "TAIWAN": CountryCode("TW"),
    "UNITED_STATES_OF_AMERICA": CountryCode("US"),
    "USER_XZ": CountryCode("XZ"),
    "WORLDWIDE_ZEROES": CountryCode("00"),
}


@dataclass(frozen=True)
class AccessPointState:
    """Information about the individual operating access points.

    This includes limited information about any connected clients.
    """

    state: f_wlan_policy.OperatingState
    """Current access point operating state."""

    mode: f_wlan_policy.ConnectivityMode
    """Requested operating connectivity mode."""

    band: f_wlan_policy.OperatingBand
    """Access point operating band."""

    frequency: int | None
    """Access point operating frequency (in MHz)."""

    clients: f_wlan_policy.ConnectedClientInformation | None
    """Information about connected clients."""

    id_: NetworkIdentifier
    """Identifying information of the access point whose state has changed."""

    @staticmethod
    def from_fidl(
        fidl: f_wlan_policy.AccessPointState,
    ) -> AccessPointState:
        """Parse from a fuchsia.wlan.policy/AccessPointState."""
        assert fidl.state is not None, f"{fidl!r} missing state"
        assert fidl.mode is not None, f"{fidl!r} missing mode"
        assert fidl.band is not None, f"{fidl!r} missing band"
        assert fidl.id_ is not None, f"{fidl!r} missing id"

        return AccessPointState(
            state=f_wlan_policy.OperatingState(fidl.state),
            mode=f_wlan_policy.ConnectivityMode(fidl.mode),
            band=f_wlan_policy.OperatingBand(fidl.band),
            frequency=fidl.frequency,
            clients=fidl.clients,
            id_=NetworkIdentifier.from_fidl(fidl.id_),
        )
