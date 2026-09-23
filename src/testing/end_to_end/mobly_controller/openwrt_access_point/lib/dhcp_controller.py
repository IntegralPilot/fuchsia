# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import logging

from libs.ssh import connection
from openwrt_access_point.lib.dhcp_config import DhcpConfig

_LOGGER: logging.Logger = logging.getLogger(__name__)


class DhcpController:
    """Handles DHCP server operations on OpenWRT."""

    def __init__(self, ssh: connection.SshConnection):
        self.ssh = ssh
        self._test_start_marker = "dnsmasq-dhcp: TEST_START"

    def start_dhcp(self, config: DhcpConfig | None = None) -> None:
        """Start DHCP for this AP object."""
        if config is None:
            config = DhcpConfig()
        _LOGGER.info("Starting DHCP server on OpenWrt")
        self.reset_dhcp_config()
        self.ssh.run(
            f"uci set dhcp.lan.dynamicdhcp='{int(config.lan.dynamic_dhcp)}'"
        )
        self.ssh.run(f"uci set dhcp.lan.leasetime='{config.lan.lease_time}'")
        if config.lan.start is not None:
            self.ssh.run(f"uci set dhcp.lan.start='{config.lan.start}'")
        if config.lan.limit is not None:
            self.ssh.run(f"uci set dhcp.lan.limit='{config.lan.limit}'")
        if config.dnsmasq.noping:
            self.ssh.run("uci set dhcp.@dnsmasq[0].noping='1'")
        self.ssh.run("uci commit dhcp")
        self.ssh.run("/etc/init.d/dnsmasq restart")
        self.mark_test_start()

    def stop_dhcp(self) -> None:
        """Stop DHCP for this AP object."""
        _LOGGER.info("Stopping DHCP server on OpenWrt")
        self.ssh.run("/etc/init.d/dnsmasq stop")

    def mark_test_start(self) -> None:
        """Mark the start of a test"""
        self.ssh.run(f"logger '{self._test_start_marker}'")

    def get_dhcp_logs_since_last_dhcp_start(self) -> str:
        """Get DHCP logs for this AP object since the last DHCP start."""
        result = self.ssh.run("logread | grep dnsmasq-dhcp")
        logs = result.stdout.decode("utf-8")

        if self._test_start_marker in logs:
            # Extract only the logs following the custom start marker
            logs = logs.split(self._test_start_marker)[-1]

        return logs

    def reset_dhcp_config(self) -> None:
        """Resets DHCP configuration to system defaults."""
        self.ssh.run("cp /rom/etc/config/dhcp /etc/config/dhcp")
        self.ssh.run("/etc/init.d/dnsmasq restart")
