#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import logging
import urllib.parse
import urllib.request
from dataclasses import dataclass
from enum import StrEnum, unique
from typing import Protocol

from antlion.controllers import pdu
from mobly import signals
from mobly.logger import PrefixLoggerAdapter


class PduDevice(pdu.PduDevice):
    """Implementation of pure abstract PduDevice object for the Synaccess np02b
    Pdu.

    TODO(http://b/318877544): Replace with NP02B
    """

    def __init__(
        self, host: str, username: str | None, password: str | None
    ) -> None:
        username = username or "admin"  # default username
        password = password or "admin"  # default password
        super().__init__(host, username, password)
        self.np02b = NP02B(host, username, password)

    def on_all(self) -> None:
        for i in range(len(self.np02b)):
            self.np02b.port(i).set(pdu.PowerState.ON)

    def off_all(self) -> None:
        for i in range(len(self.np02b)):
            self.np02b.port(i).set(pdu.PowerState.OFF)

    def on(self, outlet: int) -> None:
        self.np02b.port(outlet).set(pdu.PowerState.ON)

    def off(self, outlet: int) -> None:
        self.np02b.port(outlet).set(pdu.PowerState.OFF)

    def reboot(self, outlet: int) -> None:
        self.np02b.port(outlet).reboot()

    def status(self) -> dict[str, bool]:
        """Returns the status of the np02b outlets.

        Return:
            Mapping of outlet index ('1' and '2') to true if ON, otherwise
            false.
        """
        return {
            "1": self.np02b.port(1).status() is pdu.PowerState.ON,
            "2": self.np02b.port(2).status() is pdu.PowerState.ON,
        }

    def close(self) -> None:
        """Ensure connection to device is closed.

        In this implementation, this shouldn't be necessary, but could be in
        others that open on creation.
        """
        return


class NP02B(pdu.PDU):
    """Controller for a Synaccess netBooter NP-02B.

    See https://www.synaccess-net.com/np-02b
    """

    def __init__(self, host: str, username: str, password: str) -> None:
        self.client = Client(host, username, password)

    def port(self, index: int) -> pdu.Port:
        return Port(self.client, index)

    def __len__(self) -> int:
        return 2


class ParsePDUResponseError(signals.TestError):
    """Error when the PDU returns an unexpected response."""


class Client:
    def __init__(self, host: str, user: str, password: str) -> None:
        self._url = f"http://{host}/cmd.cgi"

        password_manager = urllib.request.HTTPPasswordMgrWithDefaultRealm()
        password_manager.add_password(None, host, user, password)
        auth_handler = urllib.request.HTTPBasicAuthHandler(password_manager)
        self._opener = urllib.request.build_opener(auth_handler)

        self.log = PrefixLoggerAdapter(
            logging.getLogger(),
            {PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[pdu | {host}]"},
        )

    def request(self, command: Command) -> Response:
        cmd = command.code()
        args = command.args()
        if args:
            cmd += f' {" ".join(args)}'

        url = f"{self._url}?{urllib.parse.quote_plus(cmd)}"
        self.log.debug(f"Sending request {url}")

        with self._opener.open(url) as res:
            body = res.read().decode("utf-8")

        self.log.debug(f"Received response: {body}")

        # Syntax for the response should be in the form:
        #    "<StatusCode>[,<PowerStatus>]"
        # For example, StatusCommand returns "$A5,01" when Port 1 is ON and
        # Port 2 is OFF.
        try:
            tokens = body.split(",", 1)
            if len(tokens) == 0:
                raise ParsePDUResponseError(
                    f'Expected a response, found "{body}"'
                )
            code = tokens[0]
            status_code = StatusCode(code)
            power_status = PowerStatus(tokens[1]) if len(tokens) == 2 else None
        except Exception as e:
            raise ParsePDUResponseError(
                f'Failed to parse response from "{body}"'
            ) from e

        return Response(status_code, power_status)


class Port(pdu.Port):
    def __init__(self, client: Client, port: int) -> None:
        if port == 0:
            raise TypeError("Invalid port index 0: ports are 1-indexed")
        if port > 2:
            raise TypeError(
                f"Invalid port index {port}: NP-02B only has 2 ports"
            )

        self.client = client
        self.port = port

    def status(self) -> pdu.PowerState:
        resp = self.client.request(StatusCommand())
        if resp.status != StatusCode.OK:
            raise ParsePDUResponseError(
                f"Expected PDU response to be {StatusCode.OK}, got {resp.status}"
            )
        if not resp.power:
            raise ParsePDUResponseError(
                "Expected PDU response to contain power, got None"
            )
        return resp.power.state(self.port)

    def set(self, state: pdu.PowerState) -> None:
        """Set the power state for this port on the PDU.

        Args:
            state: Desired power state
        """
        resp = self.client.request(SetCommand(self.port, state))
        if resp.status != StatusCode.OK:
            raise ParsePDUResponseError(
                f"Expected PDU response to be {StatusCode.OK}, got {resp.status}"
            )

        # Verify the newly set power state.
        status = self.status()
        if status is not state:
            raise ParsePDUResponseError(
                f"Expected PDU port {self.port} to be {state}, got {status}"
            )


@dataclass
class Response:
    status: StatusCode
    power: PowerStatus | None


@unique
class StatusCode(StrEnum):
    OK = "$A0"
    FAILED = "$AF"


class Command(Protocol):
    def code(self) -> str:
        """Return the cmdCode for this command."""
        ...

    def args(self) -> list[str]:
        """Return the list of arguments for this command."""
        ...


class PowerStatus:
    """State of all ports"""

    def __init__(self, states: str) -> None:
        self.states: list[pdu.PowerState] = []
        for state in states:
            self.states.insert(0, pdu.PowerState(int(state)))

    def ports(self) -> int:
        return len(self.states)

    def state(self, port: int) -> pdu.PowerState:
        return self.states[port - 1]


class SetCommand(Command):
    def __init__(self, port: int, state: pdu.PowerState) -> None:
        self.port = port
        self.state = state

    def code(self) -> str:
        return "$A3"

    def args(self) -> list[str]:
        return [str(self.port), str(self.state)]


class RebootCommand(Command):
    def __init__(self, port: int) -> None:
        self.port = port

    def code(self) -> str:
        return "$A4"

    def args(self) -> list[str]:
        return [str(self.port)]


class StatusCommand(Command):
    def code(self) -> str:
        return "$A5"

    def args(self) -> list[str]:
        return []


class SetAllCommand(Command):
    def __init__(self, state: pdu.PowerState) -> None:
        self.state = state

    def code(self) -> str:
        return "$A7"

    def args(self) -> list[str]:
        return [str(self.state)]
