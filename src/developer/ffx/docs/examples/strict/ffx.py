#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import ipaddress
import json
import subprocess
import sys
from dataclasses import dataclass

# Sample wrapper class, good enough to support the strict.py example.


@dataclass
class Target:
    address: str
    state: str | None


def format_target_address(addr):
    # Expects a dictionary representing an address returned by `ffx target list --machine json`.
    # Expected schema:
    # - type: "Ip" or "Usb" (case insensitive).
    # - ip: The IP address string (required for type="Ip").
    # - ssh_port: The SSH port number (optional/fallback for type="Ip").
    # - cid: The USB CID number (required for type="Usb").
    if addr.get("type", "").lower() == "usb":
        cid = addr.get("cid")
        if cid is None:
            raise ValueError("Address dict missing 'cid' field for USB target")
        return f"usb:cid:{cid}"

    port = addr.get("ssh_port")
    if port == 0 or port is None:
        port = 22

    ip = addr.get("ip")
    if not ip:
        raise ValueError("Address dict missing 'ip' field")

    if ":" in ip and not ip.startswith("["):
        return f"[{ip}]:{port}"
    return f"{ip}:{port}"


def is_already_resolved(target_name):
    """Checks if the target_name is already a resolved address.

    Returns True if it is an IPv4, IPv6 (with optional brackets, port, or scope ID),
    or starts with 'usb:'. Otherwise returns False.
    """
    if target_name.startswith("usb:"):
        return True

    # Strip optional brackets and port
    ip = target_name
    if ip.startswith("["):
        end_bracket = ip.find("]")
        if end_bracket != -1:
            ip = ip[1:end_bracket]
    else:
        if ":" in ip:
            parts = ip.rsplit(":", 1)
            if parts[0].count(":") == 0:
                ip = parts[0]

    if "%" in ip:
        ip = ip.split("%", 1)[0]

    try:
        ipaddress.ip_address(ip)
        return True
    except ValueError:
        return False


class FfxRunner:
    def __init__(self, log_file, ssh_key, target=None, verbose=False):
        """Initializes FfxRunner.

        Args:
            log_file: Path to the log file for ffx strict operations.
            ssh_key: Path to the private SSH key for target connection.
            target: Optional target name (nodename) or resolved socket address
              (IP:port or usb:cid:<num>). If None, the default target is
              discovered automatically.
            verbose: If True, prints verbose debug logs.
        """
        self.log_file = log_file
        self.ssh_key = ssh_key
        self.verbose = verbose
        self.target = None
        if target:
            self.resolve_target(target)
        else:
            # If no target is explicitly specified, discover the default target.
            self.discover_target()

    def debug(self, message):
        if self.verbose:
            print(message, file=sys.stderr)

    def resolve_target(self, target_name):
        # Resolves the target name to a formatted IP:PORT or usb:cid:<num> address
        # expected by ffx --strict.
        if is_already_resolved(target_name):
            self.target = Target(target_name, None)
            return self.target

        # Query ffx target list with target_name to resolve it to an IP address/port.
        target_list = self.target_list(None, query=target_name)
        if not target_list:
            raise Exception(
                f"No targets found in target list when matching '{target_name}'"
            )

        # The query returns only targets matching target_name, so we take the first match.
        target_dict = target_list[0]
        address = format_target_address(target_dict["addresses"][0])
        self.target = Target(address, target_dict.get("target_state"))
        return self.target

    def run_raw(self, opts, args):
        cmd = ["ffx"]
        cmdline = cmd + opts + args
        self.debug(" ".join(cmdline))
        try:
            return subprocess.run(
                cmdline, check=True, capture_output=True
            ).stdout
        except subprocess.CalledProcessError as e:
            print(e.stderr.decode("utf-8"))
            raise

    def run(self, target, opts, args):
        target_address = (
            target.address if isinstance(target, Target) else target
        )
        if target_address:
            opts.extend(["--target", target_address])
        out = self.run_raw(opts + ["--machine", "json"], args)
        return json.loads(out)

    def required_configs(self):
        return [f"ssh.priv={self.ssh_key}"]

    def run_strict(self, target, args, extra_configs=None):
        if extra_configs is None:
            extra_configs = []
        target_address = (
            target.address if isinstance(target, Target) else target
        )
        cfgs = ",".join(self.required_configs() + extra_configs)
        res = self.run(
            target_address,
            ["--strict", "--config", cfgs, "-o", self.log_file],
            args,
        )
        self.debug(res)
        if "unexpected_error" in res:
            raise Exception(res["unexpected_error"])
        return res

    def discover_target(self):
        targets = self.run(None, [], ["target", "list"])
        if len(targets) != 1:
            raise Exception("cannot determine default target")
        address = format_target_address(targets[0]["addresses"][0])
        self.target = Target(address, targets[0].get("target_state"))

    def target_echo(self, msg):
        """Runs 'ffx target echo' with a message and returns the response."""
        res = self.run_strict(self.target, ["target", "echo", msg])
        return res["message"]

    def target_list(self, emu_instance_dir, query=None):
        """Lists available targets, optionally filtering by query."""
        if emu_instance_dir:
            extra_configs = [f"emu.instance_dir={emu_instance_dir}"]
        else:
            extra_configs = ["emu.instance_dir="]
        args = ["target", "list"]
        if query:
            args.append(query)
        res = self.run_strict(self.target, args, extra_configs)
        return res

    def target_show(self):
        """Runs 'ffx target show' and returns the target details."""
        res = self.run_strict(self.target, ["target", "show"])
        return res

    def component_list(self):
        """Runs 'ffx component list' and returns the component list."""
        res = self.run_strict(self.target, ["component", "list"])
        return res
