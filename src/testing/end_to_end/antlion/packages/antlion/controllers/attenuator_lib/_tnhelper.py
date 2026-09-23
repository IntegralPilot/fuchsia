#!/usr/bin/env python3

# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""A helper module to communicate over telnet with AttenuatorInstruments.

User code shouldn't need to directly access this class.
"""

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import logging
import re
import telnetlib

from antlion.controllers import attenuator
from libs.proc import job


def _ascii_string(uc_string):
    return str(uc_string).encode("ASCII")


class TelnetHelper(object):
    """An internal helper class for Telnet+SCPI command-based instruments.

    It should only be used by those implementation control libraries and not by
    any user code directly.
    """

    def __init__(
        self,
        tx_cmd_separator: str = "\n",
        rx_cmd_separator: str = "\n",
        prompt: str = "",
    ) -> None:
        self._tn: telnetlib.Telnet | None = None
        self._ip_address: str | None = None
        self._port: int | None = None

        self.tx_cmd_separator = tx_cmd_separator
        self.rx_cmd_separator = rx_cmd_separator
        self.prompt = prompt

    def open(self, host: str, port: int = 23) -> None:
        self._ip_address = host
        self._port = port
        if self._tn:
            self._tn.close()
        logging.debug("Telnet Server IP = %s", host)
        self._tn = telnetlib.Telnet(host, port, timeout=10)

    def is_open(self) -> bool:
        return self._tn is not None

    def close(self) -> None:
        if self._tn:
            self._tn.close()
            self._tn = None

    def diagnose_telnet(self, host: str, port: int) -> bool:
        """Function that diagnoses telnet connections.

        This function diagnoses telnet connections and can be used in case of
        command failures. The function checks if the devices is still reachable
        via ping, and whether or not it can close and reopen the telnet
        connection.

        Returns:
            False when telnet server is unreachable or unresponsive
            True when telnet server is reachable and telnet connection has been
            successfully reopened
        """
        logging.debug("Diagnosing telnet connection")
        try:
            job_result = job.run(f"ping {host} -c 5 -i 0.2")
        except Exception as e:
            logging.error("Unable to ping telnet server: %s", e)
            return False
        ping_output = job_result.stdout.decode("utf-8")
        if not re.search(r" 0% packet loss", ping_output):
            logging.error("Ping Packets Lost. Result: %s", ping_output)
            return False
        try:
            self.close()
        except Exception as e:
            logging.error("Cannot close telnet connection: %s", e)
            return False
        try:
            self.open(host, port)
        except Exception as e:
            logging.error("Cannot reopen telnet connection: %s", e)
            return False
        logging.debug("Telnet connection likely recovered")
        return True

    def cmd(self, cmd_str: str, retry: bool = False) -> str:
        if not isinstance(cmd_str, str):
            raise TypeError("Invalid command string", cmd_str)

        if self._tn is None or self._ip_address is None or self._port is None:
            raise attenuator.InvalidOperationError(
                "Telnet connection not open for commands"
            )

        cmd_str.strip(self.tx_cmd_separator)
        self._tn.read_until(_ascii_string(self.prompt), 2)
        self._tn.write(_ascii_string(cmd_str + self.tx_cmd_separator))

        match_idx, match_val, ret_text = self._tn.expect(
            [_ascii_string(f"\\S+{self.rx_cmd_separator}")], 1
        )

        logging.debug("Telnet Command: %s", cmd_str)
        logging.debug(
            "Telnet Reply: (%s, %s, %s)", match_idx, match_val, ret_text
        )

        if match_idx == -1:
            telnet_recovered = self.diagnose_telnet(
                self._ip_address, self._port
            )
            if telnet_recovered and retry:
                logging.debug("Retrying telnet command once.")
                return self.cmd(cmd_str, retry=False)
            else:
                raise attenuator.InvalidDataError(
                    "Telnet command failed to return valid data"
                )

        ret_str = ret_text.decode()
        ret_str = ret_str.strip(
            self.tx_cmd_separator + self.rx_cmd_separator + self.prompt
        )
        return ret_str
