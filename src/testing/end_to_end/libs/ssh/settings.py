# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Create a SshSettings from a dictionary from an ACTS config

Args:
    config dict instance from an ACTS config

Returns:
    An instance of SshSettings or None
"""

from typing import TypeAlias

from libs.validation import MapValidator

Json: TypeAlias = (
    dict[str, "Json"] | list["Json"] | str | int | float | bool | None
)


class SshSettings(object):
    """Contains settings for ssh.

    Container for ssh connection settings.

    Attributes:
        username: The name of the user to log in as.
        hostname: The name of the host to connect to.
        executable: The ssh executable to use.
        port: The port to connect through (usually 22).
        host_file: The known host file to use.
        connect_timeout: How long to wait on a connection before giving a
                         timeout.
        alive_interval: How long between ssh heartbeat signals to keep the
                        connection alive.
    """

    def __init__(
        self,
        hostname: str,
        username: str,
        identity_file: str,
        port: int = 22,
        host_file: str = "/dev/null",
        connect_timeout: int = 30,
        alive_interval: int = 300,
        executable: str = "/usr/bin/ssh",
        ssh_config: str | None = None,
    ):
        self.username = username
        self.hostname = hostname
        self.executable = executable
        self.port = port
        self.host_file = host_file
        self.connect_timeout = connect_timeout
        self.alive_interval = alive_interval
        self.identity_file = identity_file
        self.ssh_config = ssh_config

    def construct_ssh_options(self) -> dict[str, str | int | bool]:
        """Construct the ssh options.

        Constructs a dictionary of option that should be used with the ssh
        command.

        Returns:
            A dictionary of option name to value.
        """
        current_options: dict[str, str | int | bool] = {}
        current_options["StrictHostKeyChecking"] = False
        current_options["UserKnownHostsFile"] = self.host_file
        current_options["ConnectTimeout"] = self.connect_timeout
        current_options["ServerAliveInterval"] = self.alive_interval
        return current_options

    def construct_ssh_flags(self) -> dict[str, None | str | int]:
        """Construct the ssh flags.

        Constructs what flags should be used in the ssh connection.

        Returns:
            A dictionary of flag name to value. If value is none then it is
            treated as a binary flag.
        """
        current_flags: dict[str, None | str | int] = {}
        current_flags["-a"] = None
        current_flags["-x"] = None
        current_flags["-p"] = self.port
        if self.identity_file:
            current_flags["-i"] = self.identity_file
        if self.ssh_config:
            current_flags["-F"] = self.ssh_config
        return current_flags


def from_config(config: Json) -> SshSettings:
    """Parse SSH settings from config JSON."""

    if not isinstance(config, dict):
        raise ValueError(f"config must be a dict, got {type(config)}")

    c = MapValidator(config)
    return SshSettings(
        hostname=c.get(str, "host"),
        username=c.get(str, "user"),
        identity_file=c.get(str, "identity_file"),
        port=c.get(int, "port", 22),
        ssh_config=c.get(str, "ssh_config", None),
        connect_timeout=c.get(int, "connect_timeout", 30),
        executable=c.get(str, "ssh_binary_path", "/usr/bin/ssh"),
    )
