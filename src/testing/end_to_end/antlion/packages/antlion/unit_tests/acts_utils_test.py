#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import ipaddress
import logging
import subprocess
import unittest

import mock
from antlion import utils
from antlion.capabilities.ssh import SSHConfig, SSHResult
from antlion.controllers.android_device import AndroidDevice
from antlion.controllers.fuchsia_device import FuchsiaDevice
from antlion.controllers.fuchsia_lib.ssh import FuchsiaSSHProvider
from libs.proc import job
from libs.ssh.connection import SshConnection

PROVISIONED_STATE_GOOD = 1

MOCK_ENO1_IP_ADDRESSES = """100.127.110.79
2401:fa00:480:7a00:8d4f:85ff:cc5c:787e
2401:fa00:480:7a00:459:b993:fcbf:1419
fe80::c66d:3c75:2cec:1d72"""

MOCK_WLAN1_IP_ADDRESSES = ""

FUCHSIA_INTERFACES = {
    "id": "1",
    "result": [
        {
            "id": 1,
            "name": "lo",
            "ipv4_addresses": [
                [127, 0, 0, 1],
            ],
            "ipv6_addresses": [
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            ],
            "online": True,
            "mac": [0, 0, 0, 0, 0, 0],
        },
        {
            "id": 2,
            "name": "eno1",
            "ipv4_addresses": [
                [100, 127, 110, 79],
            ],
            "ipv6_addresses": [
                list(ipaddress.IPv6Address("fe80::c66d:3c75:2cec:1d72").packed),
                list(
                    ipaddress.IPv6Address(
                        "2401:fa00:480:7a00:8d4f:85ff:cc5c:787e"
                    ).packed
                ),
                list(
                    ipaddress.IPv6Address(
                        "2401:fa00:480:7a00:459:b993:fcbf:1419"
                    ).packed
                ),
            ],
            "online": True,
            "mac": [0, 224, 76, 5, 76, 229],
        },
        {
            "id": 3,
            "name": "wlanxc0",
            "ipv4_addresses": [],
            "ipv6_addresses": [
                list(ipaddress.IPv6Address("fe80::60ff:5d60:34fd:fdf3").packed),
                list(ipaddress.IPv6Address("fe80::4607:bff:fe76:7ec0").packed),
            ],
            "online": False,
            "mac": [68, 7, 11, 118, 126, 192],
        },
    ],
    "error": None,
}

CORRECT_FULL_IP_LIST = {
    "ipv4_private": [],
    "ipv4_public": ["100.127.110.79"],
    "ipv6_link_local": ["fe80::c66d:3c75:2cec:1d72"],
    "ipv6_private_local": [],
    "ipv6_public": [
        "2401:fa00:480:7a00:8d4f:85ff:cc5c:787e",
        "2401:fa00:480:7a00:459:b993:fcbf:1419",
    ],
}

CORRECT_EMPTY_IP_LIST = {
    "ipv4_private": [],
    "ipv4_public": [],
    "ipv6_link_local": [],
    "ipv6_private_local": [],
    "ipv6_public": [],
}


class IpAddressUtilTest(unittest.TestCase):
    def test_positive_ipv4_normal_address(self):
        ip_address = "192.168.1.123"
        self.assertTrue(utils.is_valid_ipv4_address(ip_address))

    def test_positive_ipv4_any_address(self):
        ip_address = "0.0.0.0"
        self.assertTrue(utils.is_valid_ipv4_address(ip_address))

    def test_positive_ipv4_broadcast(self):
        ip_address = "255.255.255.0"
        self.assertTrue(utils.is_valid_ipv4_address(ip_address))

    def test_negative_ipv4_with_ipv6_address(self):
        ip_address = "fe80::f693:9fff:fef4:1ac"
        self.assertFalse(utils.is_valid_ipv4_address(ip_address))

    def test_negative_ipv4_with_invalid_string(self):
        ip_address = "fdsafdsafdsafdsf"
        self.assertFalse(utils.is_valid_ipv4_address(ip_address))

    def test_negative_ipv4_with_invalid_number(self):
        ip_address = "192.168.500.123"
        self.assertFalse(utils.is_valid_ipv4_address(ip_address))

    def test_positive_ipv6(self):
        ip_address = "fe80::f693:9fff:fef4:1ac"
        self.assertTrue(utils.is_valid_ipv6_address(ip_address))

    def test_positive_ipv6_link_local(self):
        ip_address = "fe80::"
        self.assertTrue(utils.is_valid_ipv6_address(ip_address))

    def test_negative_ipv6_with_ipv4_address(self):
        ip_address = "192.168.1.123"
        self.assertFalse(utils.is_valid_ipv6_address(ip_address))

    def test_negative_ipv6_invalid_characters(self):
        ip_address = "fe80:jkyr:f693:9fff:fef4:1ac"
        self.assertFalse(utils.is_valid_ipv6_address(ip_address))

    def test_negative_ipv6_invalid_string(self):
        ip_address = "fdsafdsafdsafdsf"
        self.assertFalse(utils.is_valid_ipv6_address(ip_address))

    @mock.patch("libs.ssh.connection.SshConnection.run")
    def test_ssh_get_interface_ip_addresses_full(self, ssh_mock):
        ssh_mock.side_effect = [
            job.Result(
                stdout=bytes(MOCK_ENO1_IP_ADDRESSES, "utf-8"), encoding="utf-8"
            ),
        ]
        self.assertEqual(
            utils.get_interface_ip_addresses(
                SshConnection("mock_settings"), "eno1"
            ),
            CORRECT_FULL_IP_LIST,
        )

    @mock.patch("libs.ssh.connection.SshConnection.run")
    def test_ssh_get_interface_ip_addresses_empty(self, ssh_mock):
        ssh_mock.side_effect = [
            job.Result(
                stdout=bytes(MOCK_WLAN1_IP_ADDRESSES, "utf-8"), encoding="utf-8"
            ),
        ]
        self.assertEqual(
            utils.get_interface_ip_addresses(
                SshConnection("mock_settings"), "wlan1"
            ),
            CORRECT_EMPTY_IP_LIST,
        )

    @mock.patch("antlion.controllers.adb.AdbProxy")
    @mock.patch.object(AndroidDevice, "is_bootloader", return_value=True)
    def test_android_get_interface_ip_addresses_full(
        self, is_bootloader, adb_mock
    ):
        adb_mock().shell.side_effect = [
            MOCK_ENO1_IP_ADDRESSES,
        ]
        self.assertEqual(
            utils.get_interface_ip_addresses(AndroidDevice(), "eno1"),
            CORRECT_FULL_IP_LIST,
        )

    @mock.patch("antlion.controllers.adb.AdbProxy")
    @mock.patch.object(AndroidDevice, "is_bootloader", return_value=True)
    def test_android_get_interface_ip_addresses_empty(
        self, is_bootloader, adb_mock
    ):
        adb_mock().shell.side_effect = [
            MOCK_WLAN1_IP_ADDRESSES,
        ]
        self.assertEqual(
            utils.get_interface_ip_addresses(AndroidDevice(), "wlan1"),
            CORRECT_EMPTY_IP_LIST,
        )

    @mock.patch(
        "antlion.controllers.fuchsia_device.FuchsiaDevice.ffx",
        new_callable=mock.PropertyMock,
    )
    @mock.patch("antlion.controllers.fuchsia_lib.ssh.FuchsiaSSHProvider.run")
    @mock.patch("antlion.capabilities.ssh.SSHProvider.wait_until_reachable")
    @mock.patch(
        "antlion.controllers.fuchsia_device."
        "FuchsiaDevice._generate_ssh_config"
    )
    @mock.patch(
        "antlion.controllers."
        "fuchsia_lib.netstack.netstack_lib."
        "FuchsiaNetstackLib.netstackListInterfaces"
    )
    def test_fuchsia_get_interface_ip_addresses_full(
        self,
        list_interfaces_mock,
        generate_ssh_config_mock,
        ssh_wait_until_reachable_mock,
        ssh_run_mock,
        ffx_mock,
    ):
        # Configure the log path which is required by ACTS logger.
        logging.log_path = "/tmp/unit_test_garbage"

        FuchsiaSSHProvider(SSHConfig("192.168.1.1", 22, "/dev/null"))
        ssh_run_mock.return_value = SSHResult(
            subprocess.CompletedProcess([], 0, stdout=b"", stderr=b"")
        )

        ssh_wait_until_reachable_mock.return_value = None

        list_interfaces_mock.return_value = FUCHSIA_INTERFACES
        self.assertEqual(
            utils.get_interface_ip_addresses(
                FuchsiaDevice({"ip": "192.168.1.1"}), "eno1"
            ),
            CORRECT_FULL_IP_LIST,
        )

    @mock.patch(
        "antlion.controllers.fuchsia_device.FuchsiaDevice.ffx",
        new_callable=mock.PropertyMock,
    )
    @mock.patch("antlion.controllers.fuchsia_lib.ssh.FuchsiaSSHProvider.run")
    @mock.patch("antlion.capabilities.ssh.SSHProvider.wait_until_reachable")
    @mock.patch(
        "antlion.controllers.fuchsia_device."
        "FuchsiaDevice._generate_ssh_config"
    )
    @mock.patch(
        "antlion.controllers."
        "fuchsia_lib.netstack.netstack_lib."
        "FuchsiaNetstackLib.netstackListInterfaces"
    )
    def test_fuchsia_get_interface_ip_addresses_empty(
        self,
        list_interfaces_mock,
        generate_ssh_config_mock,
        ssh_wait_until_reachable_mock,
        ssh_run_mock,
        ffx_mock,
    ):
        # Configure the log path which is required by ACTS logger.
        logging.log_path = "/tmp/unit_test_garbage"

        FuchsiaSSHProvider(SSHConfig("192.168.1.1", 22, "/dev/null"))
        ssh_run_mock.return_value = SSHResult(
            subprocess.CompletedProcess([], 0, stdout=b"", stderr=b"")
        )

        list_interfaces_mock.return_value = FUCHSIA_INTERFACES
        self.assertEqual(
            utils.get_interface_ip_addresses(
                FuchsiaDevice({"ip": "192.168.1.1"}), "wlan1"
            ),
            CORRECT_EMPTY_IP_LIST,
        )


if __name__ == "__main__":
    unittest.main()
