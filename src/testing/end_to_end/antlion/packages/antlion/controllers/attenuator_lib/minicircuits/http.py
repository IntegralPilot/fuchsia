#!/usr/bin/env python3

# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Class for HTTP control of Mini-Circuits RCDAT series attenuators

This class provides a wrapper to the MC-RCDAT attenuator modules for purposes
of simplifying and abstracting control down to the basic necessities. It is
not the intention of the module to expose all functionality, but to allow
interchangeable HW to be used.

See http://www.minicircuits.com/softwaredownload/Prog_Manual-6-Programmable_Attenuator.pdf
"""

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import urllib.request

from antlion.controllers import attenuator


class AttenuatorInstrument(attenuator.AttenuatorInstrument):
    """A specific HTTP-controlled implementation of AttenuatorInstrument for
    Mini-Circuits RC-DAT attenuators.

    With the exception of HTTP-specific commands, all functionality is defined
    by the AttenuatorInstrument class.
    """

    def __init__(self, num_atten: int = 1) -> None:
        self._num_atten = num_atten
        self._max_atten = attenuator.INVALID_MAX_ATTEN

        self._ip_address: str | None = None
        self._port: int | None = None
        self._timeout: int | None = None
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

    def open(self, host: str, port: int = 80, timeout_sec: int = 2) -> None:
        """Initiate a connection to the attenuator.

        Args:
            host: A valid hostname to an attenuator
            port: Port number to attempt connection
            timeout_sec: Seconds to wait to initiate a connection
        """
        self._ip_address = host
        self._port = port
        self._timeout = timeout_sec
        self._address = host

        att_req = urllib.request.urlopen(
            f"http://{self._ip_address}:{self._port}/MN?"
        )
        config_str = att_req.read().decode("utf-8").strip()
        if not config_str.startswith("MN="):
            raise attenuator.InvalidDataError(
                f"Attenuator returned invalid data. Attenuator returned: {config_str}"
            )

        config_str = config_str[len("MN=") :]
        properties = dict(
            zip(["model", "max_freq", "max_atten"], config_str.split("-", 2))
        )
        self._max_atten = float(properties["max_atten"])

    def close(self) -> None:
        """Close the connection to the attenuator."""
        # Since this controller is based on HTTP requests, there is no
        # connection teardown required.

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
            InvalidDataError if the attenuator does not respond with the
            expected output.
        """
        if not (0 <= idx < self._num_atten):
            raise IndexError(
                "Attenuator index out of range!", self._num_atten, idx
            )

        if value > self._max_atten and strict:
            raise ValueError(
                "Attenuator value out of range!", self._max_atten, value
            )
        # The actual device uses one-based index for channel numbers.
        adjusted_value = min(max(0, value), self._max_atten)
        att_req = urllib.request.urlopen(
            "http://{}:{}/CHAN:{}:SETATT:{}".format(
                self._ip_address, self._port, idx + 1, adjusted_value
            ),
            timeout=self._timeout,
        )
        att_resp = att_req.read().decode("utf-8").strip()
        if att_resp != "1":
            if retry:
                self.set_atten(idx, value, strict, retry=False)
            else:
                raise attenuator.InvalidDataError(
                    f"Attenuator returned invalid data. Attenuator returned: {att_resp}"
                )

    def get_atten(self, idx: int, retry: bool = False) -> float:
        """Returns the current attenuation of the attenuator at the given index.

        Args:
            idx: The index of the attenuator.
            retry: if True, command will be retried if possible

        Raises:
            InvalidDataError if the attenuator does not respond with the
            expected output

        Returns:
            the current attenuation value as a float
        """
        if not (0 <= idx < self._num_atten):
            raise IndexError(
                "Attenuator index out of range!", self._num_atten, idx
            )
        att_req = urllib.request.urlopen(
            f"http://{self._ip_address}:{self._port}/CHAN:{idx + 1}:ATT?",
            timeout=self._timeout,
        )
        att_resp = att_req.read().decode("utf-8").strip()
        try:
            return float(att_resp)
        except TypeError as e:
            if retry:
                return self.get_atten(idx, retry=False)

            raise attenuator.InvalidDataError(
                f"Attenuator returned invalid data. Attenuator returned: {att_resp}"
            ) from e
