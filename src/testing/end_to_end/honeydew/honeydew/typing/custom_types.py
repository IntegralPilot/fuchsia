# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Custom data types."""

from __future__ import annotations

import abc
import builtins
import enum
import ipaddress
import random
from dataclasses import dataclass
from typing import Any, TypeVar

AnyString = TypeVar("AnyString", str, bytes)


class LEVEL(enum.StrEnum):
    """Logging level that need to specified to log a message onto device"""

    INFO = "Info"
    WARNING = "Warning"
    ERROR = "Error"


class TargetAddr(abc.ABC):
    """Abstract base class representing a generic Fuchsia Target Address."""

    @classmethod
    def from_str(cls, query: str) -> TargetAddr:
        """Attempts to parse a string query into a TargetAddr.

        Args:
            query: The string representation of a target address (e.g. 'usb:cid:123', '127.0.0.1:8022')

        Returns:
            A TargetAddr subclass instance (like TargetUsb or IpPort).

        Raises:
            ValueError: If the query cannot be cleanly parsed as a valid resolved address
                (e.g., if it's just a hostname).
            NotImplementedError: If called on the base class TargetAddr; subclasses must implement this.
        """
        if cls is TargetAddr:
            for subclass in TargetAddr.__subclasses__():
                try:
                    return subclass.from_str(query)
                except ValueError:
                    continue
            raise ValueError(f"Could not parse '{query}' as TargetAddr")
        raise NotImplementedError("Subclasses must implement from_str")

    @classmethod
    def from_json(cls, obj: dict[str, Any]) -> TargetAddr:
        """Parses a FFX target address JSON object into a TargetAddr.

        Args:
            obj: The dictionary parsed from 'ffx --machine json target list'.

        Returns:
            A TargetAddr subclass instance (like TargetUsb or IpPort).

        Raises:
            ValueError: If the object type is not supported or missing required fields.
            NotImplementedError: If called on the base class TargetAddr; subclasses must implement this.
        """
        if cls is TargetAddr:
            for subclass in TargetAddr.__subclasses__():
                try:
                    return subclass.from_json(obj)
                except ValueError:
                    continue

            raise ValueError(f"Unable to create TargetAddr for {obj}")
        raise NotImplementedError("Subclasses must implement from_json")


@dataclass(frozen=True)
class TargetUsb(TargetAddr):
    """Dataclass that holds a USB Target Address

    Args:
        target_id: USB Target ID
    """

    target_id: int

    def __post_init__(self) -> None:
        """Validates target_id arg.

        Raises:
            ValueError
        """
        if self.target_id < 0:
            raise ValueError(f"target_id: {self.target_id} was negative")

    @classmethod
    def from_str(cls, query: str) -> TargetUsb:
        """Attempts to parse a string query into a TargetUsb.

        Args:
            query: The string representation of a target address (e.g. 'usb:cid:12345')

        Returns:
            A TargetUsb.

        Raises:
            ValueError: If the query cannot be cleanly parsed as a valid USB address
        """
        if query.startswith("usb:cid:"):
            usb_id_str = query.removeprefix("usb:cid:")
            try:
                usb_id = int(usb_id_str)
            except ValueError as e:
                raise ValueError(f"Invalid USB id in '{query}'") from e
            return cls(usb_id)
        raise ValueError(f"Invalid USB address '{query}': no 'usb:' prefix")

    @classmethod
    def from_json(cls, obj: dict[str, Any]) -> TargetUsb:
        """Parses a FFX target address JSON object into a TargetUsb.

        Args:
            obj: The dictionary parsed from 'ffx --machine json target list'.

        Returns:
            A TargetUsb.

        Raises:
            ValueError: If cid is missing or invalid.
        """
        # For USB, the field is "cid" -> int
        cid = obj.get("cid")
        if not isinstance(cid, int) or isinstance(cid, bool):
            raise ValueError(f"USB address has invalid or missing 'cid': {obj}")
        return cls(cid)

    def __str__(self) -> str:
        return f"usb:cid:{self.target_id}"

    @property
    def ip_str(self) -> str:
        """USB targets do not have an IP string.

        Raises:
            ValueError
        """
        raise ValueError(f"USB targets do not have an IP string: {self}")


@dataclass(frozen=True)
class IpPort(TargetAddr):
    """Dataclass that holds IP Address and Port

    Args:
        ip: Ip Address
        port: Port Number
    """

    ip: ipaddress.IPv4Address | ipaddress.IPv6Address
    port: int | None

    def __post_init__(self) -> None:
        """Validates ip and port args.

        Raises:
            ValueError
        """
        if self.port is not None and self.port < 1:
            raise ValueError(
                f"port number: {self.port} was not a positive integer"
            )

    def __str__(self) -> str:
        host: str = f"{self.ip}"
        if isinstance(self.ip, ipaddress.IPv6Address):
            host = f"[{host}]"
        if self.port:
            return f"{host}:{self.port}"
        else:
            return f"{host}"

    @classmethod
    def from_str(cls, query: str) -> IpPort:
        """Attempts to parse a string query into a IpPort.

        Args:
            query: The string representation of a target address (e.g. '127.0.0.1:8022')

        Returns:
            An IpPort.

        Raises:
            ValueError: If the query cannot be cleanly parsed as a valid address
                (e.g., if it's just a hostname), or if it is an ipv6 address with
                a symbolic scope ID.
        """
        # If it's wrapped in brackets, it must be an IP or [IP]:PORT
        if query.startswith("["):
            try:
                ip_port = cls.create_using_ip_and_port(query)
                if ip_port.port is not None:
                    return ip_port
            except ValueError:
                pass
            try:
                return cls.create_using_ip(query)
            except ValueError:
                pass
        else:
            # No brackets.
            # An unbracketed string is first tested as a standalone IP address
            # (v4 or v6). If that fails, it's then tested as an address with a
            # port (e.g. "1.2.3.4:8022"). This resolves the ambiguity for some
            # valid IPv6 addresses that contain colons. Users can use
            # brackets for IPv6 (e.g. "[::1]:8022") to force port parsing.
            try:
                return cls.create_using_ip(query)
            except ValueError:
                pass
            try:
                return cls.create_using_ip_and_port(query)
            except ValueError:
                pass

        raise ValueError(f"Could not parse '{query}' as IpPort")

    @staticmethod
    def create_using_ip_and_port(ip_port: str) -> IpPort:
        """Factory method to create IpPort object using str that has both ip
        and port values.

        Args:
            ip_port: IP address and port of the fuchsia device. This is of
                     one the following formats:
                        {ipv4_address}:{port}
                        [{ipv6_address}]:{port}
                        {ipv6_address}:{port}

        Returns:
            A valid IpPort

        Raises:
          ValueError
        """
        try:
            # If we have something of form
            #     192.168.1.1:8888 ==> ["192.168.1.1", "8888"]
            # If we have something of form
            #     [::1]:8888 ==> ["[::1]", "8888"]
            arr: list[str] = ip_port.rsplit(":", 1)
            if len(arr) != 1 and len(arr) != 2:
                raise ValueError(
                    f"Value: {ip_port} was not a valid IpPort (needs "
                    f"IP Address and optional Port)"
                )
            addr_part: str = arr[0]
            # Remove [] that might be surrounding an IPv6 address
            if addr_part.startswith("[") and addr_part.endswith("]"):
                addr_part = addr_part[1:-1]

            port = None
            if len(arr) == 2:
                port_part: str = arr[1]
                port = int(port_part)
                if port < 1:
                    raise ValueError(
                        f"For IpPort: {ip_port}, port number: {port} was "
                        f"not a positive integer)"
                    )

            return IpPort(ipaddress.ip_address(addr_part), port)
        except ValueError as e:
            raise e

    @staticmethod
    def create_using_ip(ip: str) -> IpPort:
        """Factory method to create IpPort object using str that has ip address.

        Args:
            ip: IP address and port of the fuchsia device. This is of
                     one the following formats:
                        {ipv4_address}
                        [{ipv6_address}]
                        {ipv6_address}

        Returns:
            A valid IpPort

        Raises:
          ValueError
        """
        try:
            # Remove [] that might be surrounding an IPv6 address
            if ip.startswith("[") and ip.endswith("]"):
                ip = ip[1:-1]
            return IpPort(ipaddress.ip_address(ip), None)
        except ValueError as e:
            raise e

    @classmethod
    def from_json(cls, obj: dict[str, Any]) -> IpPort:
        """Parses a FFX target address JSON object into an IpPort.

        Args:
            obj: The dictionary parsed from 'ffx --machine json target list'.

        Returns:
            An IpPort.

        Raises:
            ValueError: If the object is missing required fields.
        """
        addr_type = obj.get("type")
        if addr_type != "Ip":
            raise ValueError(f"type not Ip in {obj}")
        ssh_ip = obj.get("ip")
        if ssh_ip is None:
            raise ValueError(f"Missing ip address in {obj}")
        ssh_port = obj.get("ssh_port")
        if ssh_port == 0:
            ssh_port = None

        ip_obj = ipaddress.ip_address(ssh_ip)
        return IpPort(ip=ip_obj, port=ssh_port)

    @property
    def ip_str(self) -> str:
        """The IP address as a string."""
        return str(self.ip)


@dataclass(frozen=True)
class TargetSshAddress(IpPort):
    """Dataclass that holds target's ssh address information.

    Args:
        ip: Target's SSH IP Address
        port: Target's SSH port
    """


@dataclass(frozen=True)
class Sl4fServerAddress(IpPort):
    """Dataclass that holds sl4f server address information.

    Args:
        ip: IP Address of SL4F server
        port: Port where SL4F server is listening for SL4F requests
    """


@dataclass(frozen=True)
class DeviceInfo:
    """Dataclass that holds Fuchsia device information.

    Args:
        name: Device name returned by `ffx target list`.
        serial_number: Device serial number.
        ip_port: IP Address and port of the device.
        serial_socket: Device serial socket path.
        fastboot_node_id: Fastboot node ID.
    """

    name: str
    serial_number: str | None
    ip_port: IpPort | None
    serial_socket: str | None
    fastboot_node_id: str | None = None

    def __str__(self) -> str:
        return (
            f"name={self.name}, "
            f"ip_port={self.ip_port}, "
            f"serial_socket={self.serial_socket}, "
            f"fastboot_node_id={self.fastboot_node_id}"
        )


@dataclass(frozen=True)
class FidlEndpoint:
    """Dataclass that holds FIDL end point information.

    Args:
        moniker: moniker pointing to the FIDL end point
        protocol: protocol name of the FIDL end point
    """

    moniker: str
    protocol: str


class MacAddress:
    """MAC address following the EUI-48 identifier format.

    Used by IEEE 802 networks as unique identifiers assigned to network
    interface controllers.
    """

    _mac: bytes

    def __init__(self, mac: str | bytes | bytearray) -> None:
        if isinstance(mac, str):
            mac = bytes([int(a, 16) for a in mac.split(":")])

        if len(mac) != 6:
            raise ValueError(f"Expected 6 bytes, got {len(mac)}")

        self._mac = bytes(mac)

    @classmethod
    def random(cls) -> MacAddress:
        """Create a random MAC address."""
        return cls(random.randbytes(6))

    def __bytes__(self) -> builtins.bytes:
        return self._mac

    def with_unicast_bit(self) -> MacAddress:
        """Return a copy of the MAC address with the unicast bit set."""
        b = bytearray(self._mac)
        b[0] &= 0xFE
        return MacAddress(b)

    def with_multicast_bit(self) -> MacAddress:
        """Return a copy of the MAC address with the multicast bit set."""
        b = bytearray(self._mac)
        b[0] |= 0x01
        return MacAddress(b)

    def with_locally_administered_bit(self) -> MacAddress:
        """Return a copy of the MAC address with the locally administered bit set."""
        b = bytearray(self._mac)
        b[0] |= 0x02
        return MacAddress(b)

    def with_universally_administered_bit(self) -> MacAddress:
        """Return a copy of the MAC address with the universally administered bit set."""
        b = bytearray(self._mac)
        b[0] &= 0xFD
        return MacAddress(b)

    def with_last_octet_incremented(self) -> MacAddress:
        """Return a copy of the MAC address with the last octet incremented by 1."""
        b = bytearray(self._mac)
        b[-1] = (b[-1] + 1) % 256
        return MacAddress(b)

    def with_last_octet(self, o: int) -> MacAddress:
        """Return a copy of the MAC address with the last octet set to `o`."""
        if not 0 <= o < 255:
            raise ValueError(f"Octet must fit in a single byte: {o}")
        b = bytearray(self._mac)
        b[-1] = o
        return MacAddress(b)

    def __str__(self) -> str:
        """Return MAC address in the form "xx:xx:xx:xx:xx:xx"."""
        return ":".join([f"{octet:0>2x}" for octet in self._mac])

    def __repr__(self) -> str:
        return f"MacAddress('{self}')"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, MacAddress):
            return False
        return self._mac == other._mac

    def __hash__(self) -> int:
        return hash(self._mac)
