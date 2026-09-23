#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth Utils for Sample Test"""
import asyncio
import logging
import re
from typing import List

import fidl_fuchsia_bluetooth as f_bt
from honeydew.fuchsia_device import fuchsia_device

_LOGGER: logging.Logger = logging.getLogger(__name__)
DEFAULT_WAITING_SECS = 10
DEFAULT_RETRIES_ATTEMPT = 6


def sl4f_bt_mac_address(mac_address: str) -> List[int]:
    """Converts MAC addresses to reversed BT byte lists.
    ex. AA:BB:CC:DD:EE:FF
        AABBCCDDEEFF
    """
    if ":" in mac_address:
        return _convert_reverse_hex(mac_address.split(":"))
    return _convert_reverse_hex(re.findall("..", mac_address))


def _convert_reverse_hex(address: List[str]) -> List[int]:
    """Reverses ASCII mac address to 64-bit byte lists."""
    return [int(x, 16) for x in reversed(address)]


async def forget_all_bt_devices(
    device: fuchsia_device.FuchsiaDevice,
) -> None:
    """Unpairs and deletes any BT peer pairing data from the device."""
    data = await device.bluetooth_gap.get_known_remote_devices()
    _LOGGER.info(data)
    for peer in data.values():
        if peer.connected:
            await device.bluetooth_gap.forget_device(identifier=peer.id)


async def verify_bt_connection(
    identifier: f_bt.PeerId,
    device: fuchsia_device.FuchsiaDevice,
    wait_secs: int = DEFAULT_WAITING_SECS,
    num_retries: int = DEFAULT_RETRIES_ATTEMPT,
) -> bool:
    """Verifies BT connection between peer identifier and device."""
    _LOGGER.info("Checking if device is connected to %s", identifier)
    for _ in range(num_retries):
        data = await device.bluetooth_gap.get_known_remote_devices()
        _LOGGER.info(data)
        for peer in data.values():
            if peer.id.value == identifier.value and peer.connected:
                _LOGGER.info("Connection is active")
                return True
        _LOGGER.info("Connection is not active, Checking in 10 seconds")
        await asyncio.sleep(wait_secs)
    return False


async def verify_bt_pairing(
    identifier: f_bt.PeerId,
    device: fuchsia_device.FuchsiaDevice,
    wait_secs: int = DEFAULT_WAITING_SECS,
    num_retries: int = DEFAULT_RETRIES_ATTEMPT,
) -> bool:
    """Verifies BT pairing between peer identifier and device"""
    _LOGGER.info("Checking if device is paired to %s", identifier)
    for _ in range(num_retries):
        data = await device.bluetooth_gap.get_known_remote_devices()
        for peer in data.values():
            if peer.id.value == identifier.value and peer.bonded:
                _LOGGER.info("Pairing is complete")
                return True
        _LOGGER.info("Pairing is not completed, Checking in 10 seconds")
        await asyncio.sleep(wait_secs)
    return False
