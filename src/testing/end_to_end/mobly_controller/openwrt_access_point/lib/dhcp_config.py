# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""DHCP configuration dataclasses for OpenWrt Access Point."""

import dataclasses


@dataclasses.dataclass
class Lan:
    """UCI dhcp.lan configuration.

    Attributes:
        dynamic_dhcp: Whether to enable dynamic DHCP.
        lease_time: The lease time for IP addresses (e.g., '12h', '30m').
        start: The start offset for the DHCP pool.
        limit: The number of addresses in the DHCP pool.
    """

    dynamic_dhcp: bool = True
    lease_time: str = "12h"
    start: int | None = None
    limit: int | None = None


@dataclasses.dataclass
class Dnsmasq:
    """UCI dhcp.dnsmasq configuration.

    Attributes:
        noping: Disable ping check before assigning IP.
    """

    noping: bool = False


@dataclasses.dataclass
class DhcpConfig:
    """DHCP configuration for OpenWrt.

    Attributes:
        lan: The LAN interface DHCP configuration.
        dnsmasq: The dnsmasq DHCP configuration.
    """

    lan: Lan = dataclasses.field(default_factory=Lan)
    dnsmasq: Dnsmasq = dataclasses.field(default_factory=Dnsmasq)
