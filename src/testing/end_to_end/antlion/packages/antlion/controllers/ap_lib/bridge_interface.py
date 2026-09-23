#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import time

from libs.proc.runner import CalledProcessError
from libs.ssh.connection import SshConnection

_BRCTL = "brctl"
BRIDGE_NAME = "br-lan"
CREATE_BRIDGE = f"{_BRCTL} addbr {BRIDGE_NAME}"
DELETE_BRIDGE = f"{_BRCTL} delbr {BRIDGE_NAME}"
BRING_DOWN_BRIDGE = f"ifconfig {BRIDGE_NAME} down"


class BridgeInterfaceConfigs(object):
    """Configs needed for creating bridge interface between LAN and WLAN."""

    def __init__(self, iface_wlan: str, iface_lan: str, bridge_ip: str) -> None:
        """Set bridge interface configs based on the channel info.

        Args:
            iface_wlan: the wlan interface as part of the bridge
            iface_lan: the ethernet LAN interface as part of the bridge
            bridge_ip: the ip address assigned to the bridge interface
        """
        self.iface_wlan = iface_wlan
        self.iface_lan = iface_lan
        self.bridge_ip = bridge_ip


class BridgeInterface(object):
    """Class object for bridge interface betwen WLAN and LAN"""

    def __init__(self, ssh: SshConnection) -> None:
        """Initialize the BridgeInterface class.

        Bridge interface will be added between ethernet LAN port and WLAN port.
        Args:
            ap: AP object within ACTS
        """
        self.ssh = ssh

    def startup(self, brconfigs: BridgeInterfaceConfigs) -> None:
        """Start up the bridge interface.

        Args:
            brconfigs: the bridge interface config, type BridgeInterfaceConfigs
        """

        logging.info("Create bridge interface between LAN and WLAN")
        # Create the bridge
        try:
            self.ssh.run(CREATE_BRIDGE)
        except CalledProcessError:
            logging.warning(
                f"Bridge interface {BRIDGE_NAME} already exists, no action needed"
            )

        # Enable 4addr mode on for the wlan interface
        ENABLE_4ADDR = f"iw dev {brconfigs.iface_wlan} set 4addr on"
        try:
            self.ssh.run(ENABLE_4ADDR)
        except CalledProcessError:
            logging.warning(
                f"4addr is already enabled on {brconfigs.iface_wlan}"
            )

        # Add both LAN and WLAN interfaces to the bridge interface
        for interface in [brconfigs.iface_lan, brconfigs.iface_wlan]:
            ADD_INTERFACE = f"{_BRCTL} addif {BRIDGE_NAME} {interface}"
            try:
                self.ssh.run(ADD_INTERFACE)
            except CalledProcessError:
                logging.warning(
                    f"{interface} has already been added to {BRIDGE_NAME}"
                )
        time.sleep(5)

        # Set IP address on the bridge interface to bring it up
        SET_BRIDGE_IP = f"ifconfig {BRIDGE_NAME} {brconfigs.bridge_ip}"
        self.ssh.run(SET_BRIDGE_IP)
        time.sleep(2)

        # Bridge interface is up
        logging.info("Bridge interface is up and running")

    def teardown(self, brconfigs: BridgeInterfaceConfigs) -> None:
        """Tear down the bridge interface.

        Args:
            brconfigs: the bridge interface config, type BridgeInterfaceConfigs
        """
        logging.info("Bringing down the bridge interface")
        # Delete the bridge interface
        self.ssh.run(BRING_DOWN_BRIDGE)
        time.sleep(1)
        self.ssh.run(DELETE_BRIDGE)

        # Bring down wlan interface and disable 4addr mode
        BRING_DOWN_WLAN = f"ifconfig {brconfigs.iface_wlan} down"
        self.ssh.run(BRING_DOWN_WLAN)
        time.sleep(2)
        DISABLE_4ADDR = f"iw dev {brconfigs.iface_wlan} set 4addr off"
        self.ssh.run(DISABLE_4ADDR)
        time.sleep(1)
        logging.info("Bridge interface is down")
