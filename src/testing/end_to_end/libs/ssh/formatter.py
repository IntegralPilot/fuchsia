# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


from typing import Iterator

from libs.ssh.settings import SshSettings


class SshFormatter(object):
    """Handles formatting ssh commands.

    Handler for formatting chunks of the ssh command to run.
    """

    def format_ssh_executable(self, settings: SshSettings) -> str:
        """Format the executable name.

        Formats the executable name as a string.

        Args:
            settings: The ssh settings being used.

        Returns:
            A string for the ssh executable name.
        """
        return settings.executable

    def format_host_name(self, settings: SshSettings) -> str:
        """Format hostname.

        Formats the hostname to connect to.

        Args:
            settings: The ssh settings being used.

        Returns:
            A string of the connection host name to connect to.
        """
        return f"{settings.username}@{settings.hostname}"

    def format_value(self, value: object) -> str:
        """Formats a command line value.

        Takes in a value and formats it so it can be safely used in the
        command line.

        Args:
            value: The value to format.

        Returns:
            A string representation of the formatted value.
        """
        if isinstance(value, bool):
            return "yes" if value else "no"

        return str(value)

    def format_options_list(
        self, options: dict[str, str | int | bool]
    ) -> Iterator[str]:
        """Format the option list.

        Formats a dictionary of options into a list of strings to be used
        on the command line.

        Args:
            options: A dictionary of options.

        Returns:
            An iterator of strings that should go on the command line.
        """
        for option_name in options:
            option = options[option_name]

            yield "-o"
            yield f"{option_name}={self.format_value(option)}"

    def format_flag_list(
        self, flags: dict[str, str | int | None]
    ) -> Iterator[str]:
        """Format the flags list.

        Formats a dictionary of flags into a list of strings to be used
        on the command line.

        Args:
            flags: A dictionary of options.

        Returns:
            An iterator of strings that should be used on the command line.
        """
        for flag_name in flags:
            flag = flags[flag_name]

            yield flag_name
            if flag is not None:
                yield self.format_value(flag)

    def format_ssh_local_command(
        self,
        settings: SshSettings,
        extra_flags: dict[str, str | int | None] | None = None,
        extra_options: dict[str, str | int | bool] | None = None,
    ) -> list[str]:
        """Formats the local part of the ssh command.

        Formats the local section of the ssh command. This is the part of the
        command that will actual launch ssh on our local machine with the
        specified settings.

        Args:
            settings: The ssh settings.
            extra_flags: Extra flags to include.
            extra_options: Extra options to include.

        Returns:
            An array of strings that make up the command and its local
            arguments.
        """
        if extra_flags is None:
            extra_flags = {}
        if extra_options is None:
            extra_options = {}

        options = settings.construct_ssh_options()
        for extra_option_name in extra_options:
            options[extra_option_name] = extra_options[extra_option_name]
        options_list = list(self.format_options_list(options))

        flags = settings.construct_ssh_flags()
        for extra_flag_name in extra_flags:
            flags[extra_flag_name] = extra_flags[extra_flag_name]
        flags_list = list(self.format_flag_list(flags))

        all_options = options_list + flags_list
        host_name = self.format_host_name(settings)
        executable = self.format_ssh_executable(settings)

        base_command = [executable] + all_options + [host_name]

        return base_command

    def format_command(
        self,
        command: str,
        settings: SshSettings,
        extra_flags: dict[str, str | int | None] | None = None,
        extra_options: dict[str, str | int | bool] | None = None,
    ) -> list[str]:
        """Formats a full command.

        Formats the full command to run in order to run a command on a remote
        machine.

        Args:
            command: The command to run on the remote machine. Can either be
                     a string or a list of strings.
            env: The environment variables to include on the remote machine.
            settings: The ssh settings to use.
            extra_flags: Extra flags to include with the settings.
            extra_options: Extra options to include with the settings.

        Returns:
            A list of strings that make up the total ssh command.
        """
        if extra_flags is None:
            extra_flags = {}
        if extra_options is None:
            extra_options = {}

        local_command = self.format_ssh_local_command(
            settings, extra_flags, extra_options
        )
        return local_command + [command]
