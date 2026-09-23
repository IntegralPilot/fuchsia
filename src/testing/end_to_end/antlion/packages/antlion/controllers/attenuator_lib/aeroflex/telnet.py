#!/usr/bin/env python3

# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Class for Telnet control of Aeroflex 832X and 833X Series Attenuator Modules

This class provides a wrapper to the Aeroflex attenuator modules for purposes
of simplifying and abstracting control down to the basic necessities. It is
not the intention of the module to expose all functionality, but to allow
interchangeable HW to be used.

See http://www.aeroflex.com/ams/weinschel/PDFILES/IM-608-Models-8320-&-8321-preliminary.pdf
"""

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from antlion.controllers import attenuator
from antlion.controllers.attenuator_lib import _tnhelper


class AttenuatorInstrument(attenuator.AttenuatorInstrument):
    def __init__(self, num_atten: int = 0) -> None:
        self._num_atten = num_atten
        self._max_atten = attenuator.INVALID_MAX_ATTEN

        self._tnhelper = _tnhelper.TelnetHelper(
            tx_cmd_separator="\r\n", rx_cmd_separator="\r\n", prompt=">"
        )
        self._properties: dict[str, str] | None = None
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

    def open(self, host: str, port: int, _timeout_sec: int = 5) -> None:
        """Initiate a connection to the attenuator.

        Args:
            host: A valid hostname to an attenuator
            port: Port number to attempt connection
            timeout_sec: Seconds to wait to initiate a connection
        """
        self._tnhelper.open(host, port)

        # work around a bug in IO, but this is a good thing to do anyway
        self._tnhelper.cmd("*CLS", False)
        self._address = host

        if self._num_atten == 0:
            self._num_atten = int(self._tnhelper.cmd("RFCONFIG? CHAN"))

        configstr = self._tnhelper.cmd("RFCONFIG? ATTN 1")

        self._properties = dict(
            zip(
                [
                    "model",
                    "max_atten",
                    "min_step",
                    "unknown",
                    "unknown2",
                    "cfg_str",
                ],
                configstr.split(", ", 5),
            )
        )

        self._max_atten = float(self._properties["max_atten"])

    def close(self) -> None:
        """Close the connection to the attenuator."""
        self._tnhelper.close()

    def set_atten(
        self, idx: int, value: float, _strict: bool = True, _retry: bool = False
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

        if value > self._max_atten:
            raise ValueError(
                "Attenuator value out of range!", self._max_atten, value
            )

        self._tnhelper.cmd(f"ATTN {idx + 1} {value}", False)

    def get_atten(self, idx: int, _retry: bool = False) -> float:
        """Returns the current attenuation given its index in the instrument.

        Args:
            idx: Index used to identify a particular attenuator in an instrument
            retry: If True, command will be retried if possible

        Raises:
            InvalidOperationError if the telnet connection is not open.

        Returns:
            The current attenuation value
        """
        if not self._tnhelper.is_open():
            raise attenuator.InvalidOperationError("Connection not open!")

        #       Potentially redundant safety check removed for the moment
        #       if idx >= self.num_atten:
        #           raise IndexError("Attenuator index out of range!", self.num_atten, idx)

        atten_val = self._tnhelper.cmd(f"ATTN? {idx + 1}")

        return float(atten_val)
