#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import logging
import os
import unittest

import mock
from antlion.capabilities.ssh import SSHConfig, SSHProvider
from antlion.controllers import iperf_client
from antlion.controllers.iperf_client import (
    IPerfClient,
    IPerfClientBase,
    IPerfClientOverAdb,
    IPerfClientOverSsh,
)

# The position in the call tuple that represents the args array.
ARGS = 0

# The position in the call tuple that represents the kwargs dict.
KWARGS = 1


class IPerfClientModuleTest(unittest.TestCase):
    """Tests the antlion.controllers.iperf_client module functions."""

    def test_create_can_create_client_over_adb(self):
        self.assertIsInstance(
            iperf_client.create([{"AndroidDevice": "foo"}])[0],
            IPerfClientOverAdb,
            "Unable to create IPerfClientOverAdb from create().",
        )

    @mock.patch("subprocess.run")
    @mock.patch("socket.create_connection")
    def test_create_can_create_client_over_ssh(
        self, mock_socket_create_connection, mock_subprocess_run
    ):
        self.assertIsInstance(
            iperf_client.create(
                [
                    {
                        "ssh_config": {
                            "user": "root",
                            "host": "192.168.42.11",
                            "identity_file": "/dev/null",
                        }
                    }
                ]
            )[0],
            IPerfClientOverSsh,
            "Unable to create IPerfClientOverSsh from create().",
        )

    def test_create_can_create_local_client(self):
        self.assertIsInstance(
            iperf_client.create([{}])[0],
            IPerfClient,
            "Unable to create IPerfClient from create().",
        )


class IPerfClientBaseTest(unittest.TestCase):
    """Tests antlion.controllers.iperf_client.IPerfClientBase."""

    @mock.patch("os.makedirs")
    def test_get_full_file_path_creates_parent_directory(self, mock_makedirs):
        # Will never actually be created/used.
        logging.log_path = "/tmp/unit_test_garbage"

        full_file_path = IPerfClientBase._get_full_file_path(0)

        self.assertTrue(
            mock_makedirs.called, "Did not attempt to create a directory."
        )
        self.assertEqual(
            os.path.dirname(full_file_path),
            mock_makedirs.call_args[ARGS][0],
            "The parent directory of the full file path was not created.",
        )


class IPerfClientTest(unittest.TestCase):
    """Tests antlion.controllers.iperf_client.IPerfClient."""

    @mock.patch("builtins.open")
    @mock.patch("subprocess.call")
    def test_start_writes_to_full_file_path(self, mock_call, mock_open):
        client = IPerfClient()
        file_path = "/path/to/foo"
        client._get_full_file_path = lambda _: file_path

        client.start("127.0.0.1", "IPERF_ARGS", "TAG")

        mock_open.assert_called_with(file_path, "w")
        self.assertEqual(
            mock_call.call_args[KWARGS]["stdout"],
            mock_open().__enter__.return_value,
            "IPerfClient did not write the logs to the expected file.",
        )


class IPerfClientOverSshTest(unittest.TestCase):
    """Test antlion.controllers.iperf_client.IPerfClientOverSshTest."""

    @mock.patch("socket.create_connection")
    @mock.patch("subprocess.run")
    @mock.patch("builtins.open")
    def test_start_writes_output_to_full_file_path(
        self, mock_open, mock_subprocess_run, mock_socket_create_connection
    ):
        ssh_provider = SSHProvider(
            SSHConfig(
                user="root",
                host_name="192.168.42.11",
                identity_file="/dev/null",
            )
        )
        client = IPerfClientOverSsh(ssh_provider)
        file_path = "/path/to/foo"
        client._get_full_file_path = lambda _: file_path
        client.start("127.0.0.1", "IPERF_ARGS", "TAG")
        mock_open.assert_called_with(file_path, "w")
        mock_open().__enter__().write.assert_called()


class IPerfClientOverAdbTest(unittest.TestCase):
    """Test antlion.controllers.iperf_client.IPerfClientOverAdb."""

    @mock.patch("builtins.open")
    def test_start_writes_output_to_full_file_path(self, mock_open):
        client = IPerfClientOverAdb(None)
        file_path = "/path/to/foo"
        client._get_full_file_path = lambda _: file_path

        with mock.patch(
            "antlion.controllers.iperf_client."
            "IPerfClientOverAdb._android_device"
        ) as adb_device:
            adb_device.adb.shell.return_value = "output"
            client.start("127.0.0.1", "IPERF_ARGS", "TAG")

        mock_open.assert_called_with(file_path, "w")
        mock_open().__enter__().write.assert_called_with("output")


if __name__ == "__main__":
    unittest.main()
