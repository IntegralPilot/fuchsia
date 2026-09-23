#!/usr/bin/env python3

# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Class for Telnet control of Mini-Circuits RCDAT series attenuators

This class provides a wrapper to the MC-RCDAT attenuator modules for purposes
of simplifying and abstracting control down to the basic necessities. It is
not the intention of the module to expose all functionality, but to allow
interchangeable HW to be used.

See http://www.minicircuits.com/softwaredownload/Prog_Manual-6-Programmable_Attenuator.pdf
"""

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from antlion.controllers import attenuator
from antlion.controllers.attenuator_lib import _tnhelper


class AttenuatorInstrument(attenuator.AttenuatorInstrument):
    """A specific telnet-controlled implementation of AttenuatorInstrument for
    Mini-Circuits RC-DAT attenuators.

    With the exception of telnet-specific commands, all functionality is defined
    by the AttenuatorInstrument class. Because telnet is a stateful protocol,
    the functionality of AttenuatorInstrument is contingent upon a telnet
    connection being established.
    """

    def __init__(self, num_atten: int = 0) -> None:
        self._num_atten = num_atten
        self._max_atten = attenuator.INVALID_MAX_ATTEN
        self.properties: dict[str, str] | None = None
        self._tnhelper = _tnhelper.TelnetHelper(
            tx_cmd_separator="\r\n", rx_cmd_separator="\r\n", prompt=""
        )
        self._address: str | None = None

    @property
    def address(self) -> str | None:
        return self._address

    @property
    def num_atten(self) -> int:
        return self._num_atten

    @property
    def max_atten(self) -> float:
        return self._max_atten

    def __del__(self) -> None:
        if self._tnhelper.is_open():
            self.close()

    def open(self, host: str, port: int, _timeout_sec: int = 5) -> None:
        """Initiate a connection to the attenuator.

        Args:
            host: A valid hostname to an attenuator
            port: Port number to attempt connection
            timeout_sec: Seconds to wait to initiate a connection
        """
        self._tnhelper.open(host, port)
        self._address = host

        if self._num_atten == 0:
            self._num_atten = 1

        config_str = self._tnhelper.cmd("MN?")

        if config_str.startswith("MN="):
            config_str = config_str[len("MN=") :]

        self.properties = dict(
            zip(["model", "max_freq", "max_atten"], config_str.split("-", 2))
        )
        self._max_atten = float(self.properties["max_atten"])

    def close(self) -> None:
        """Close the connection to the attenuator."""
        self._tnhelper.close()

    def set_atten(
        self, idx: int, value: float, strict: bool = True, retry: bool = False
    ) -> None:
        """Sets the attenuation given its index in the instrument.

        Args:
            idx: Index used to identify a particular attenuator in an instrument
            value: Value for nominal attenuation to be set
            strict: If True, raise an error when given out of bounds attenuation
            retry: If True, command will be retried if possible

        Raises:
            InvalidOperationError if the telnet connection is not open.
            IndexError if the index is not valid for this instrument.
            ValueError if the requested set value is greater than the maximum
                attenuation value.
        """

        if not self._tnhelper.is_open():
            raise attenuator.InvalidOperationError("Connection not open!")

        if idx >= self._num_atten:
            raise IndexError(
                "Attenuator index out of range!", self._num_atten, idx
            )

        if value > self._max_atten and strict:
            raise ValueError(
                "Attenuator value out of range!", self._max_atten, value
            )
        # The actual device uses one-based index for channel numbers.
        adjusted_value = min(max(0, value), self._max_atten)
        self._tnhelper.cmd(
            f"CHAN:{idx + 1}:SETATT:{adjusted_value}", retry=retry
        )

    def get_atten(self, idx: int, retry: bool = False) -> float:
        """Returns the current attenuation given its index in the instrument.

        Args:
            idx: Index used to identify a particular attenuator in an instrument
            retry: If True, command will be retried if possible

        Returns:
            The current attenuation value

        Raises:
            InvalidOperationError if the telnet connection is not open.
        """
        if not self._tnhelper.is_open():
            raise attenuator.InvalidOperationError("Connection not open!")

        if idx >= self._num_atten or idx < 0:
            raise IndexError(
                "Attenuator index out of range!", self._num_atten, idx
            )

        if self._num_atten == 1:
            atten_val_str = self._tnhelper.cmd(":ATT?", retry=retry)
        else:
            atten_val_str = self._tnhelper.cmd(
                f"CHAN:{idx + 1}:ATT?", retry=retry
            )
        atten_val = float(atten_val_str)
        return atten_val
