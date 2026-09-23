#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def, attr-defined"
from __future__ import annotations

import logging
import os
import socket
import subprocess
import threading
from abc import ABC, abstractmethod

from antlion import context
from antlion.capabilities.ssh import SSHConfig
from antlion.controllers.adb_lib.error import AdbCommandError
from antlion.controllers.android_device import AndroidDevice
from antlion.controllers.fuchsia_lib.ssh import SSHProvider
from antlion.types import ControllerConfig, Json
from antlion.validation import MapValidator
from libs.commands.date import LinuxDateCommand

MOBLY_CONTROLLER_CONFIG_NAME: str = "IPerfClient"


class IPerfError(Exception):
    """Raised on execution errors of iPerf."""


def create(configs: list[ControllerConfig]) -> list[IPerfClientBase]:
    """Factory method for iperf clients.

    The function creates iperf clients based on at least one config.
    If configs contain ssh settings or and AndroidDevice, remote iperf clients
    will be started on those devices, otherwise, a the client will run on the
    local machine.

    Args:
        configs: config parameters for the iperf server
    """
    results: list[IPerfClientBase] = []
    for config in configs:
        c = MapValidator(config)
        if "ssh_config" in config:
            results.append(
                IPerfClientOverSsh(
                    SSHProvider(
                        SSHConfig.from_config(c.get(dict, "ssh_config"))
                    ),
                    test_interface=c.get(str, "test_interface"),
                    sync_date=c.get(bool, "sync_date", True),
                )
            )
        else:
            results.append(IPerfClient())
    return results


def destroy(objects: list[IPerfClientBase]) -> None:
    # No cleanup needed.
    pass


def get_info(objects: list[IPerfClientBase]) -> list[Json]:
    return []


class RouteNotFound(ConnectionError):
    """Failed to find a route to the iperf server."""


class IPerfClientBase(ABC):
    """The Base class for all IPerfClients.

    This base class is responsible for synchronizing the logging to prevent
    multiple IPerfClients from writing results to the same file, as well
    as providing the interface for IPerfClient objects.
    """

    # Keeps track of the number of IPerfClient logs to prevent file name
    # collisions.
    __log_file_counter = 0

    __log_file_lock = threading.Lock()

    @property
    @abstractmethod
    def test_interface(self) -> str | None:
        """Find the test interface.

        Returns:
            Name of the interface used to communicate with server_ap, or None if
            not set.
        """
        ...

    @staticmethod
    def _get_full_file_path(tag: str = "") -> str:
        """Returns the full file path for the IPerfClient log file.

        Note: If the directory for the file path does not exist, it will be
        created.

        Args:
            tag: The tag passed in to the server run.
        """
        current_context = context.get_current_context()
        full_out_dir = os.path.join(
            current_context.get_full_output_path(), "iperf_client_files"
        )

        with IPerfClientBase.__log_file_lock:
            os.makedirs(full_out_dir, exist_ok=True)
            tags = ["IPerfClient", tag, IPerfClientBase.__log_file_counter]
            out_file_name = "%s.log" % (
                ",".join([str(x) for x in tags if x != "" and x is not None])
            )
            IPerfClientBase.__log_file_counter += 1

        return os.path.join(full_out_dir, out_file_name)

    def start(
        self,
        ip: str,
        iperf_args: str,
        tag: str,
        timeout: int = 3600,
        iperf_binary: str | None = None,
    ) -> str:
        """Starts iperf client, and waits for completion.

        Args:
            ip: iperf server ip address.
            iperf_args: A string representing arguments to start iperf
                client. Eg: iperf_args = "-t 10 -p 5001 -w 512k/-u -b 200M -J".
            tag: A string to further identify iperf results file
            timeout: the maximum amount of time the iperf client can run.
            iperf_binary: Location of iperf3 binary. If none, it is assumed the
                the binary is in the path.

        Returns:
            full_out_path: iperf result path.
        """
        raise NotImplementedError("start() must be implemented.")


class IPerfClient(IPerfClientBase):
    """Class that handles iperf3 client operations."""

    @property
    def test_interface(self) -> str | None:
        return None

    def start(
        self,
        ip: str,
        iperf_args: str,
        tag: str,
        timeout: int = 3600,
        iperf_binary: str | None = None,
    ) -> str:
        """Starts iperf client, and waits for completion.

        Args:
            ip: iperf server ip address.
            iperf_args: A string representing arguments to start iperf
            client. Eg: iperf_args = "-t 10 -p 5001 -w 512k/-u -b 200M -J".
            tag: tag to further identify iperf results file
            timeout: unused.
            iperf_binary: Location of iperf3 binary. If none, it is assumed the
                the binary is in the path.

        Returns:
            full_out_path: iperf result path.
        """
        if not iperf_binary:
            logging.debug(
                "No iperf3 binary specified.  "
                "Assuming iperf3 is in the path."
            )
            iperf_binary = "iperf3"
        else:
            logging.debug(f"Using iperf3 binary located at {iperf_binary}")
        iperf_cmd = [str(iperf_binary), "-c", ip] + iperf_args.split(" ")
        full_out_path = self._get_full_file_path(tag)

        with open(full_out_path, "w") as out_file:
            subprocess.call(iperf_cmd, stdout=out_file)

        return full_out_path


class IPerfClientOverSsh(IPerfClientBase):
    """Class that handles iperf3 client operations on remote machines."""

    def __init__(
        self,
        ssh_provider: SSHProvider,
        test_interface: str | None = None,
        sync_date: bool = True,
    ):
        self._ssh_provider = ssh_provider
        self._test_interface = test_interface

        if sync_date:
            # iperf clients are not given internet access, so their system time
            # needs to be manually set to be accurate.
            LinuxDateCommand(self._ssh_provider).sync()

    @property
    def test_interface(self) -> str | None:
        return self._test_interface

    def start(
        self,
        ip: str,
        iperf_args: str,
        tag: str,
        timeout: int = 3600,
        iperf_binary: str | None = None,
    ) -> str:
        """Starts iperf client, and waits for completion.

        Args:
            ip: iperf server ip address.
            iperf_args: A string representing arguments to start iperf
            client. Eg: iperf_args = "-t 10 -p 5001 -w 512k/-u -b 200M -J".
            tag: tag to further identify iperf results file
            timeout: the maximum amount of time to allow the iperf client to run
            iperf_binary: Location of iperf3 binary. If none, it is assumed the
                the binary is in the path.

        Returns:
            full_out_path: iperf result path.
        """
        if not iperf_binary:
            logging.debug(
                "No iperf3 binary specified.  "
                "Assuming iperf3 is in the path."
            )
            iperf_binary = "iperf3"
        else:
            logging.debug(f"Using iperf3 binary located at {iperf_binary}")
        iperf_cmd = f"{iperf_binary} -c {ip} {iperf_args}"
        full_out_path = self._get_full_file_path(tag)

        try:
            iperf_process = self._ssh_provider.run(
                iperf_cmd, timeout_sec=timeout
            )
            iperf_output = iperf_process.stdout
            with open(full_out_path, "wb") as out_file:
                out_file.write(iperf_output)
        except socket.timeout:
            raise TimeoutError(
                "Socket timeout. Timed out waiting for iperf "
                "client to finish."
            )
        except Exception as err:
            logging.exception(f"iperf run failed: {err}")

        return full_out_path


class IPerfClientOverAdb(IPerfClientBase):
    """Class that handles iperf3 operations over ADB devices."""

    def __init__(
        self, android_device: AndroidDevice, test_interface: str | None = None
    ):
        """Creates a new IPerfClientOverAdb object.

        Args:
            android_device_or_serial: Either an AndroidDevice object, or the
                serial that corresponds to the AndroidDevice. Note that the
                serial must be present in an AndroidDevice entry in the ACTS
                config.
            test_interface: The network interface that will be used to send
                traffic to the iperf server.
        """
        self._android_device = android_device
        self._test_interface = test_interface

    @property
    def test_interface(self) -> str | None:
        return self._test_interface

    def start(
        self,
        ip: str,
        iperf_args: str,
        tag: str,
        timeout: int = 3600,
        iperf_binary: str | None = None,
    ) -> str:
        """Starts iperf client, and waits for completion.

        Args:
            ip: iperf server ip address.
            iperf_args: A string representing arguments to start iperf
            client. Eg: iperf_args = "-t 10 -p 5001 -w 512k/-u -b 200M -J".
            tag: tag to further identify iperf results file
            timeout: the maximum amount of time to allow the iperf client to run
            iperf_binary: Location of iperf3 binary. If none, it is assumed the
                the binary is in the path.

        Returns:
            The iperf result file path.
        """
        clean_out = ""
        try:
            if not iperf_binary:
                logging.debug(
                    "No iperf3 binary specified.  "
                    "Assuming iperf3 is in the path."
                )
                iperf_binary = "iperf3"
            else:
                logging.debug(f"Using iperf3 binary located at {iperf_binary}")
            iperf_cmd = f"{iperf_binary} -c {ip} {iperf_args}"
            out = self._android_device.adb.shell(
                str(iperf_cmd), timeout=timeout
            )
            clean_out = out.split("\n")
            if "error" in clean_out[0].lower():
                raise IPerfError(clean_out)
        except (subprocess.TimeoutExpired, AdbCommandError):
            logging.warning("TimeoutError: Iperf measurement failed.")

        full_out_path = self._get_full_file_path(tag)
        with open(full_out_path, "w") as out_file:
            out_file.write("\n".join(clean_out))

        return full_out_path
