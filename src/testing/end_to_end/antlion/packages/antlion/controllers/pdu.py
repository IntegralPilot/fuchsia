#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import enum
import logging
import time
from enum import IntEnum, unique
from typing import Protocol

from antlion.types import ControllerConfig, Json
from antlion.validation import MapValidator

MOBLY_CONTROLLER_CONFIG_NAME: str = "PduDevice"

# Allow time for capacitors to discharge.
DEFAULT_REBOOT_DELAY_SEC = 5.0


class PduType(enum.StrEnum):
    NP02B = "synaccess.np02b"
    WEBPOWERSWITCH = "digital_loggers.webpowerswitch"


class PduError(Exception):
    """An exception for use within PduDevice implementations"""


def create(configs: list[ControllerConfig]) -> list[PduDevice]:
    """Creates a PduDevice for each config in configs.

    Args:
        configs: List of configs from PduDevice field.
            Fields:
                device: a string "<brand>.<model>" that corresponds to module
                    in pdu_lib/
                host: a string of the device ip address
                username (optional): a string of the username for device sign-in
                password (optional): a string of the password for device sign-in
    Return:
        A list of PduDevice objects.
    """
    pdus: list[PduDevice] = []
    for config in configs:
        c = MapValidator(config)
        device = c.get(str, "device")
        pduType = PduType(device)

        host = c.get(str, "host")
        username = c.get(str, "username", None)
        password = c.get(str, "password", None)

        match pduType:
            case PduType.NP02B:
                from antlion.controllers.pdu_lib.synaccess.np02b import (
                    PduDevice as NP02B,
                )

                pdus.append(NP02B(host, username, password))
            case PduType.WEBPOWERSWITCH:
                from antlion.controllers.pdu_lib.digital_loggers.webpowerswitch import (
                    PduDevice as WebPowerSwitch,
                )

                pdus.append(WebPowerSwitch(host, username, password))
    return pdus


def destroy(objects: list[PduDevice]) -> None:
    """Ensure any connections to devices are closed.

    Args:
        pdu_list: A list of PduDevice objects.
    """
    for pdu in objects:
        pdu.close()


def get_info(objects: list[PduDevice]) -> list[Json]:
    """Retrieves info from a list of PduDevice objects.

    Args:
        pdu_list: A list of PduDevice objects.
    Return:
        A list containing a dictionary for each PduDevice, with keys:
            'host': a string of the device ip address
            'username': a string of the username
            'password': a string of the password
    """
    info: list[Json] = []
    for pdu in objects:
        info.append(
            {
                "host": pdu.host,
                "username": pdu.username,
                "password": pdu.password,
            }
        )
    return info


def get_pdu_port_for_device(
    device_pdu_config: dict[str, Json], pdus: list[PduDevice]
) -> tuple[PduDevice, int]:
    """Retrieves the pdu object and port of that PDU powering a given device.
    This is especially necessary when there are multilpe devices on a single PDU
    or multiple PDUs registered.

    Args:
        device_pdu_config: a dict, representing the config of the device.
        pdus: a list of registered PduDevice objects.

    Returns:
        A tuple: (PduObject for the device, string port number on that PDU).

    Raises:
        ValueError, if there is no PDU matching the given host in the config.

    Example ACTS config:
        ...
        "testbed": [
            ...
            "FuchsiaDevice": [
                {
                    "ip": "<device_ip>",
                    "ssh_config": "/path/to/sshconfig",
                    "PduDevice": {
                        "host": "192.168.42.185",
                        "port": 2
                    }
                }
            ],
            "AccessPoint": [
                {
                    "ssh_config": {
                        ...
                    },
                    "PduDevice": {
                        "host": "192.168.42.185",
                        "port" 1
                    }
                }
            ],
            "PduDevice": [
                {
                    "device": "synaccess.np02b",
                    "host": "192.168.42.185"
                }
            ]
        ],
        ...
    """
    config = MapValidator(device_pdu_config)
    pdu_ip = config.get(str, "host")
    port = config.get(int, "port")
    for pdu in pdus:
        if pdu.host == pdu_ip:
            return pdu, port
    raise ValueError(f"No PduDevice with host: {pdu_ip}")


class PDU(Protocol):
    """Control power delivery to a device with a PDU."""

    def port(self, index: int) -> Port:
        """Access a single port.

        Args:
            index: Index of the port, likely the number identifier above the outlet.

        Returns:
            Controller for the specified port.
        """
        ...

    def __len__(self) -> int:
        """Count the number of ports.

        Returns:
            Number of ports on this PDU.
        """
        ...


class Port(Protocol):
    """Controlling the power delivery to a single port of a PDU."""

    def status(self) -> PowerState:
        """Return the power state for this port.

        Returns:
            Power state
        """
        ...

    def set(self, state: PowerState) -> None:
        """Set the power state for this port.

        Args:
            state: Desired power state
        """
        ...

    def reboot(self, delay_sec: float = DEFAULT_REBOOT_DELAY_SEC) -> None:
        """Set the power state OFF then ON after a delay.

        Args:
            delay_sec: Length to wait before turning back ON. This is important to allow
                the device's capacitors to discharge.
        """
        self.set(PowerState.OFF)
        time.sleep(delay_sec)
        self.set(PowerState.ON)


@unique
class PowerState(IntEnum):
    OFF = 0
    ON = 1


class PduDevice(object):
    """An object that defines the basic Pdu functionality and abstracts
    the actual hardware.

    This is a pure abstract class. Implementations should be of the same
    class name (eg. class PduDevice(pdu.PduDevice)) and exist in
    pdu_lib/<brand>/<device_name>.py. PduDevice objects should not be
    instantiated by users directly.

    TODO(http://b/318877544): Replace PduDevice with PDU
    """

    def __init__(
        self, host: str, username: str | None, password: str | None
    ) -> None:
        if type(self) is PduDevice:
            raise NotImplementedError(
                "Base class: cannot be instantiated directly"
            )
        self.host = host
        self.username = username
        self.password = password
        self.log = logging.getLogger()

    def on_all(self) -> None:
        """Turns on all outlets on the device."""
        raise NotImplementedError("Base class: cannot be called directly")

    def off_all(self) -> None:
        """Turns off all outlets on the device."""
        raise NotImplementedError("Base class: cannot be called directly")

    def on(self, outlet: int) -> None:
        """Turns on specific outlet on the device.
        Args:
            outlet: index of the outlet to turn on.
        """
        raise NotImplementedError("Base class: cannot be called directly")

    def off(self, outlet: int) -> None:
        """Turns off specific outlet on the device.
        Args:
            outlet: index of the outlet to turn off.
        """
        raise NotImplementedError("Base class: cannot be called directly")

    def reboot(self, outlet: int) -> None:
        """Toggles a specific outlet on the device to off, then to on.
        Args:
            outlet: index of the outlet to reboot.
        """
        raise NotImplementedError("Base class: cannot be called directly")

    def status(self) -> dict[str, bool]:
        """Retrieves the status of the outlets on the device.

        Return:
            A dictionary matching outlet string to:
                True: if outlet is On
                False: if outlet is Off
        """
        raise NotImplementedError("Base class: cannot be called directly")

    def close(self) -> None:
        """Closes connection to the device."""
        raise NotImplementedError("Base class: cannot be called directly")
