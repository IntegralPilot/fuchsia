# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.custom_types.py."""

import ipaddress
import unittest
from typing import Any

from honeydew.typing import custom_types
from parameterized import parameterized


class CustomTypesTests(unittest.TestCase):
    """Unit tests for honeydew.custom_types.py."""

    @parameterized.expand(
        [
            (
                "valid_ipv4",
                "127.0.0.1:8081",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("127.0.0.1"), port=8081
                ),
            ),
            (
                "valid_ipv6_scope_numeric",
                "[::1%1]:8081",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("::1%1"), port=8081
                ),
            ),
            (
                "valid_ipv6_no_brackets",
                "::1:8081",
                custom_types.IpPort(ip=ipaddress.ip_address("::1"), port=8081),
            ),
        ]
    )
    def test_create_using_ip_and_port(
        self, _: str, addr: str, expected: custom_types.IpPort
    ) -> None:
        """Test cases for IpPort.create_using_ip_and_port()."""
        got: custom_types.IpPort = custom_types.IpPort.create_using_ip_and_port(
            addr
        )
        self.assertEqual(got, expected)

    @parameterized.expand(
        [
            (
                "valid_ipv4",
                "127.0.0.1",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("127.0.0.1"), port=None
                ),
            ),
            (
                "valid_ipv6",
                "::1",
                custom_types.IpPort(ip=ipaddress.ip_address("::1"), port=None),
            ),
            (
                "valid_ipv6_scope_numeric",
                "::1%1",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("::1%1"), port=None
                ),
            ),
            (
                "valid_ipv6_with_brackets",
                "[::1]",
                custom_types.IpPort(ip=ipaddress.ip_address("::1"), port=None),
            ),
        ]
    )
    def test_create_using_ip(
        self, _: str, addr: str, expected: custom_types.IpPort
    ) -> None:
        """Test cases for IpPort.create_using_ip()."""
        got: custom_types.IpPort = custom_types.IpPort.create_using_ip(addr)
        self.assertEqual(got, expected)

    @parameterized.expand(
        [
            ("invalid", "some_str"),
            ("invalid_double_scope", "[::1%2%1]:100"),
            ("invalid_double_percent", "[::1%%1]:100"),
            ("invalid_negative_port", "[::1]:-1"),
            ("invalid_zero_port", "[::1]:0"),
            ("invalid_port_number", "[::1]:asdf"),
        ]
    )
    def test_create_using_ip_and_port_raises(self, _: str, addr: str) -> None:
        """Test cases for IpPort.create_using_ip_and_port() which raise
        exceptions."""
        with self.assertRaises(ValueError):
            custom_types.IpPort.create_using_ip_and_port(addr)

    @parameterized.expand(
        [
            ("invalid", "some_str"),
            ("invalid_double_scope", "[::1%2%1]"),
            ("invalid_double_percent", "[::1%%1]"),
        ]
    )
    def test_create_using_ip_raises(self, _: str, addr: str) -> None:
        """Test cases for IpPort.create_using_ip() which raise exceptions."""
        with self.assertRaises(ValueError):
            custom_types.IpPort.create_using_ip(addr)

    @parameterized.expand(
        [
            (
                "valid_ipv4_and_port",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("127.0.0.1"), port=8081
                ),
                "127.0.0.1:8081",
            ),
            (
                "valid_ipv6_and_port",
                custom_types.IpPort(ip=ipaddress.ip_address("::1"), port=8081),
                "[::1]:8081",
            ),
            (
                "valid_ipv6_with_scope_and_port",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("::1%1"), port=8081
                ),
                "[::1%1]:8081",
            ),
            (
                "valid_ipv4_without_port",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("127.0.0.1"), port=None
                ),
                "127.0.0.1",
            ),
            (
                "valid_ipv6_without_port",
                custom_types.IpPort(
                    ip=ipaddress.ip_address("::1%1"), port=None
                ),
                "[::1%1]",
            ),
        ]
    )
    def test_ipport_str(
        self, _: str, ip_port: custom_types.IpPort, expected: str
    ) -> None:
        """Test cases for IpPort.__str__."""
        got = str(ip_port)
        self.assertEqual(got, expected)

    def test_ipport_ip_str(self) -> None:
        """Test cases for IpPort.ip_str."""
        ip_port = custom_types.IpPort(
            ip=ipaddress.ip_address("127.0.0.1"), port=8081
        )
        self.assertEqual(ip_port.ip_str, "127.0.0.1")
        ip_port = custom_types.IpPort(ip=ipaddress.ip_address("::1"), port=None)
        self.assertEqual(ip_port.ip_str, "::1")

    @parameterized.expand(
        [
            (
                "usb_valid",
                "usb:cid:12345",
                custom_types.TargetUsb(12345),
            ),
            (
                "ip_port_valid",
                "192.168.1.1:8022",
                custom_types.IpPort(ipaddress.ip_address("192.168.1.1"), 8022),
            ),
            (
                "ip_valid",
                "192.168.1.1",
                custom_types.IpPort(ipaddress.ip_address("192.168.1.1"), None),
            ),
            (
                "ipv6_no_port",
                "fe80::1",
                custom_types.IpPort(ipaddress.ip_address("fe80::1"), None),
            ),
            (
                "ipv6_with_port",
                "[fe80::1]:1234",
                custom_types.IpPort(ipaddress.ip_address("fe80::1"), 1234),
            ),
        ]
    )
    def test_target_addr_from_str(
        self, _: str, query: str, expected: custom_types.TargetAddr
    ) -> None:
        """Test cases for TargetAddr.from_str() success."""
        got = custom_types.TargetAddr.from_str(query)
        self.assertEqual(got, expected)

    @parameterized.expand(
        [
            ("invalid_usb", "usb:cid:notanint"),
            ("invalid_usb_format", "usb123"),
            ("negative_usb", "usb:cid:-1"),
            ("invalid_ip", "256.256.256.256"),
            ("random_string", "my-fuchsia-device"),
        ]
    )
    def test_target_addr_from_str_raises(self, _: str, query: str) -> None:
        """Test cases for TargetAddr.from_str() that raise ValueError."""
        with self.assertRaises(ValueError):
            custom_types.TargetAddr.from_str(query)

    @parameterized.expand(
        [
            (
                "ip_default_port_none",
                {"type": "Ip", "ip": "192.168.1.1"},
                custom_types.IpPort(ipaddress.ip_address("192.168.1.1"), None),
            ),
            (
                "ip_explicit_port",
                {"type": "Ip", "ip": "192.168.1.1", "ssh_port": 8022},
                custom_types.IpPort(ipaddress.ip_address("192.168.1.1"), 8022),
            ),
            (
                "ip_port_zero_is_none",
                {"type": "Ip", "ip": "192.168.1.1", "ssh_port": 0},
                custom_types.IpPort(ipaddress.ip_address("192.168.1.1"), None),
            ),
            (
                "ipv6_no_scope",
                {"type": "Ip", "ip": "fe80::1"},
                custom_types.IpPort(ipaddress.ip_address("fe80::1"), None),
            ),
            (
                "ipv6_with_scope",
                {"type": "Ip", "ip": "fe80::1%1"},
                custom_types.IpPort(ipaddress.ip_address("fe80::1%1"), None),
            ),
            (
                "ipv6_with_lexical_scope",
                {"type": "Ip", "ip": "fe80::1%lo"},
                custom_types.IpPort(ipaddress.ip_address("fe80::1%lo"), None),
            ),
            (
                "usb_valid",
                {"type": "Usb", "cid": 12345},
                custom_types.TargetUsb(12345),
            ),
        ]
    )
    def test_target_addr_from_json(
        self, _: str, obj: dict[str, Any], expected: custom_types.TargetAddr
    ) -> None:
        """Test cases for TargetAddr.from_json() success."""
        got = custom_types.TargetAddr.from_json(obj)
        self.assertEqual(got, expected)

    @parameterized.expand(
        [
            ("invalid_ip", {"type": "Ip", "ip": "256.256.256.256"}),
            ("missing_ip", {"type": "Ip"}),
            ("usb_missing_cid", {"type": "Usb"}),
            ("usb_negative_cid", {"type": "Usb", "cid": -1}),
            ("usb_bool_cid", {"type": "Usb", "cid": True}),
            ("unsupported_type", {"type": "Unknown"}),
        ]
    )
    def test_target_addr_from_json_raises(
        self, _: str, obj: dict[str, Any]
    ) -> None:
        """Test cases for TargetAddr.from_json() raising ValueError."""
        with self.assertRaises(ValueError):
            custom_types.TargetAddr.from_json(obj)

    def test_target_usb_str(self) -> None:
        """Test cases for TargetUsb.__str__."""
        got = str(custom_types.TargetUsb(12345))
        self.assertEqual(got, "usb:cid:12345")

    def test_target_usb_ip_str_raises(self) -> None:
        """Test cases for TargetUsb.ip_str."""
        with self.assertRaises(ValueError):
            _ = custom_types.TargetUsb(12345).ip_str

    def test_mac_with_bytes(self) -> None:
        """Test if initialization works for valid bytes."""
        mac = custom_types.MacAddress(
            bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB])
        )
        self.assertEqual(
            bytes(mac),
            bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB]),
        )
        self.assertEqual(
            str(mac),
            "01:23:45:67:89:ab",
        )

    def test_mac_with_str(self) -> None:
        """Test if initialization works for valid strings."""
        mac = custom_types.MacAddress("01:23:45:67:89:ab")
        self.assertEqual(
            bytes(mac),
            bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB]),
        )
        self.assertEqual(
            str(mac),
            "01:23:45:67:89:ab",
        )

    def test_mac_with_bytes_invalid(self) -> None:
        """Test if initialization fails for invalid bytes."""
        for msg, mac_bytes in [
            ("empty", bytes([])),
            (
                "too short (01:23:45:67:89)",
                bytes([0x01, 0x23, 0x45, 0x67, 0x89]),
            ),
            (
                "too long (01:23:45:67:89:ab:cd)",
                bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD]),
            ),
            ("invalid type inside list", [0x01, 0x23, 0x45, 0x67, 0x89, "a"]),
        ]:
            with self.subTest(msg=msg, mac_bytes=mac_bytes):
                with self.assertRaises((ValueError, TypeError)):
                    custom_types.MacAddress(mac_bytes)  # type: ignore

    def test_mac_with_str_invalid(self) -> None:
        """Test if initialization fails for invalid strings."""
        for msg, mac_str in [
            ("empty", ""),
            ("too short (01:23:45:67:89)", "01:23:45:67:89"),
            (
                "too long (01:23:45:67:89:ab:cd)",
                "01:23:45:67:89:ab:cd",
            ),
            ("invalid byte", "01:23:45:67:89:abcd"),
            ("invalid hex", "hello world!"),
        ]:
            with self.subTest(msg=msg, mac_str=mac_str):
                with self.assertRaises(ValueError):
                    custom_types.MacAddress(mac_str)

    def test_mac_random(self) -> None:
        """Test random MAC generation and ensure they differ."""
        for _ in range(10):
            mac1 = custom_types.MacAddress.random()
            mac2 = custom_types.MacAddress.random()

            self.assertEqual(len(bytes(mac1)), 6)
            self.assertEqual(len(bytes(mac2)), 6)

            if mac1 != mac2:
                return  # Test passes if we successfully generated two different ones!

        self.fail("Generated identical MAC addresses 10 times in a row")

    def test_mac_with_unicast_bit(self) -> None:
        mac = custom_types.MacAddress("ff:ff:ff:ff:ff:ff").with_unicast_bit()
        self.assertEqual(bytes(mac)[0] & 0x01, 0)

    def test_mac_with_multicast_bit(self) -> None:
        mac = custom_types.MacAddress("00:00:00:00:00:00").with_multicast_bit()
        self.assertEqual(bytes(mac)[0] & 0x01, 1)

    def test_mac_with_locally_administered_bit(self) -> None:
        mac = custom_types.MacAddress(
            "00:00:00:00:00:00"
        ).with_locally_administered_bit()
        self.assertEqual(bytes(mac)[0] & 0x02, 2)

    def test_mac_with_universally_administered_bit(self) -> None:
        mac = custom_types.MacAddress(
            "ff:ff:ff:ff:ff:ff"
        ).with_universally_administered_bit()
        self.assertEqual(bytes(mac)[0] & 0x02, 0)

    def test_mac_with_last_octet_incremented(self) -> None:
        mac = custom_types.MacAddress(
            "00:00:00:00:00:00"
        ).with_last_octet_incremented()
        self.assertEqual(str(mac), "00:00:00:00:00:01")

        # Test wrap around
        mac = custom_types.MacAddress(
            "ff:ff:ff:ff:ff:ff"
        ).with_last_octet_incremented()
        self.assertEqual(str(mac), "ff:ff:ff:ff:ff:00")

    def test_mac_with_last_octet(self) -> None:
        mac = custom_types.MacAddress("00:00:00:00:00:00").with_last_octet(0x0A)
        self.assertEqual(str(mac), "00:00:00:00:00:0a")

        with self.assertRaises(ValueError):
            mac.with_last_octet(0x100)

    def test_mac_eq_and_hash(self) -> None:
        """Test MAC addresses correctly act as dictionary keys."""
        mac1 = custom_types.MacAddress("01:23:45:67:89:ab")
        mac2 = custom_types.MacAddress(
            bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB])
        )
        mac3 = custom_types.MacAddress("11:22:33:44:55:66")

        self.assertEqual(mac1, mac2)
        self.assertNotEqual(mac1, mac3)
        self.assertNotEqual(mac1, "01:23:45:67:89:ab")  # Not equal to string

        # Test dictionary keys (hashing)
        mac_dict = {mac1: "value"}
        self.assertEqual(mac_dict[mac2], "value")


if __name__ == "__main__":
    unittest.main()
