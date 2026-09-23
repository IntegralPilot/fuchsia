# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections

from antlion.controllers.ap_lib.hostapd_security import Security


class BssSettings(object):
    """Settings for a bss.

    Settings for a bss to allow multiple network on a single device.

    Attributes:
        name: The name that this bss will go by.
        ssid: The name of the ssid to broadcast.
        hidden: If true then the ssid will be hidden.
        security: The security settings to use.
        bssid: The bssid to use.
    """

    def __init__(
        self,
        name: str,
        ssid: str,
        security: Security,
        hidden: bool = False,
        bssid: str | None = None,
    ):
        self.name = name
        self.ssid = ssid
        self.security = security
        self.hidden = hidden
        self.bssid = bssid

    def generate_dict(self) -> dict[str, str | int]:
        """Returns: A dictionary of bss settings."""
        settings: dict[str, str | int] = collections.OrderedDict()
        settings["bss"] = self.name
        if self.bssid:
            settings["bssid"] = self.bssid
        if self.ssid:
            settings["ssid"] = self.ssid
            settings["ignore_broadcast_ssid"] = 1 if self.hidden else 0

        security_settings = self.security.generate_dict()
        for k, v in security_settings.items():
            settings[k] = v

        return settings
