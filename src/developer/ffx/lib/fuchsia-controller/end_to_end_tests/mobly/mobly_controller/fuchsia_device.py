# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly Controller for Fuchsia Device (controlled via Fuchsia Controller)."""

import ipaddress
import logging
import os
import os.path
import subprocess
import time
from typing import Any, Tuple

from fuchsia_controller_py import (
    Channel,
    Context,
    FcTransportStatus,
    IsolateDir,
)
from mobly import base_test

MOBLY_CONTROLLER_CONFIG_NAME = "FuchsiaDevice"
TIMEOUTS: dict[str, int] = {
    "OFFLINE": 120,
    "ONLINE": 180,
    "SLEEP": 1,
}


class FuchsiaDevice(object):
    def __init__(self, config: dict[str, Any]):
        target = config.get("ipv6") or config.get("ipv4") or config.get("name")
        if not target:
            raise ValueError(
                f"Unable to load config properly. Target has no address or name: {config}"
            )
        self.target = target
        self.config = config
        self.ctx: Context | None = None
        self.isolate: IsolateDir | None = None

    def set_ctx(self, test: base_test.BaseTestClass) -> None:
        log_dir = test.log_path
        isolation_path = None
        ctx_config = {
            "log.level": "trace",
            "log.enabled": "true",
        }
        if log_dir:
            isolation_path = os.path.join(log_dir, "isolate")
            ctx_config["log.dir"] = log_dir
        self.isolate = IsolateDir(dir=isolation_path)
        logging.info(
            f"Loading context, isolate_dir={isolation_path}, log_dir={log_dir}, target={self.target}"
        )
        try:
            ip = ipaddress.ip_address(self.target)
            logging.info(
                f"Target '{self.target}' is an IP address. Disabling mDNS discovery."
            )
            if isinstance(ip, ipaddress.IPv6Address):
                self.target = f"[{self.target}]"
            ctx_config["discovery.mdns.enabled"] = "false"

        except ValueError:
            logging.info(
                f"Target '{self.target}' determined to be nodename. Enabling mDNS discovery."
            )
            # It is likely necessary to set mDNS discovery to the underlying
            # config (after context creation) in order for this to function
            # properly. This, for the time being, is the same as what
            # Lacewing does.
            ctx_config["discovery.mdns.enabled"] = "true"
        self.ctx = Context(
            isolate_dir=self.isolate, target=self.target, config=ctx_config
        )

    def notify_intentional_disconnect(self) -> None:
        """Notifies the FFX monitor of an upcoming intentional disconnect."""
        nodename = self.config.get("name")
        if not nodename:
            logging.warning(
                "No nodename found in config. Skipping intentional disconnect notification."
            )
            return

        # Use the ffx path from config if available, otherwise default to "ffx"
        ffx_config = (
            self.config.get("honeydew_config", {})
            .get("transports", {})
            .get("ffx", {})
        )
        ffx_path = ffx_config.get("path", "ffx")

        cmd = [ffx_path]
        if self.isolate:
            cmd.extend(["--isolate-dir", self.isolate.directory()])

        # Inject configuration required to find the monitor in infra
        shared_data = ffx_config.get("shared_data")
        if shared_data:
            cmd.extend(["-c", f"shared_data={shared_data}"])

        subtools_path = ffx_config.get("subtools_search_path")
        if subtools_path:
            cmd.extend(["-c", f"ffx.subtool-search-paths={subtools_path}"])

        cmd.extend(
            ["monitor", "intentional-disconnect", "--nodename", nodename]
        )
        logging.info(f"Notifying intentional disconnect for {nodename}")
        try:
            # We want to see the output if it fails
            result = subprocess.run(
                cmd, check=False, capture_output=True, text=True
            )
            if result.returncode != 0:
                logging.warning(
                    f"FFX monitor notification failed (exit {result.returncode}):\n"
                    f"STDOUT: {result.stdout}\n"
                    f"STDERR: {result.stderr}"
                )
        except Exception as e:
            logging.warning(f"Failed to notify intentional disconnect: {e}")

    def channel_create(self) -> Tuple[Channel, Channel]:
        assert self.ctx is not None
        return self.ctx.channel_create()

    async def wait_offline(self, timeout: int = TIMEOUTS["OFFLINE"]) -> None:
        """Waits for the Fuchsia device to be offline.

        Args:
            timeout: Determines how long (in fractional seconds) to wait before considering this
            to be a timeout. Defaults to the global `TIMEOUTS["OFFLINE"]` value.

        Raises:
            TimeoutError: in the event that the timeout is reached.
        """
        logging.info(f"Waiting for target '{self.target}' to go offline")
        start_time = time.time()
        try:
            assert self.ctx is not None
            self.ctx.target_wait(timeout, True)
        except FcTransportStatus:
            raise TimeoutError()
        logging.info(
            f"Target '{self.target}' is now offline after {time.time() - start_time} seconds"
        )

    async def wait_online(self, timeout: int = TIMEOUTS["ONLINE"]) -> None:
        """Waits for the Fuchsia device to come online.

        A device is considered online when it is connected to the remote control proxy in the ffx
        daemon.

        Args:
            timeout: Determines how long (in fractional seconds) to wait before considering this
            to be a timeout. Defaults to the global `TIMEOUTS["ONLINE"]` value.

        Raises:
            TimeoutError: in the event that the timeout has been reached before the target device
            is considered online.
        """
        start_time = time.time()
        logging.info(f"Waiting for target '{self.target}' to come back online.")
        try:
            assert self.ctx is not None
            self.ctx.target_wait(timeout)
        except FcTransportStatus:
            raise TimeoutError()
        logging.info(
            f"Target '{self.target}' back online after {time.time() - start_time} seconds."
        )


def create(configs: list[dict[str, Any]]) -> list[FuchsiaDevice]:
    """Creates the all Fuchsia devices for tests.

    Args:
        configs: The list of configs describing each Fuchsia device.

    Returns:
        List of Fuchsia devices. Each instance is isolated, containing a config (for reference),
        and a fuchsia controller `Context`.

    Raises:
        ValueError: in the event that a config value lacks an "ipv4," "ipv6," or "name" key.
    """
    res = []
    for config in configs:
        res.append(FuchsiaDevice(config))
    return res


def destroy(_: list[FuchsiaDevice]) -> None:
    """Destroys all listed Fuchsia devices.

    Args:
        _: The list of Fuchsia devices being destroyed.
    """


def get_info(fuchsia_devices: list[FuchsiaDevice]) -> list[dict[str, Any]]:
    """Returns all info of each Fuchsia device.

    Args:
        fuchsia_devices: The list of Fuchsia devices being queried.

    Returns:
        The config for each Fuchsia device.
    """
    res = []
    for device in fuchsia_devices:
        res.append(device.config)
    return res
