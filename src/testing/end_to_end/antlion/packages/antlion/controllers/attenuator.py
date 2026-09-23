#!/usr/bin/env python3.4
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from __future__ import annotations

import enum
import logging
from typing import Protocol, runtime_checkable

from antlion.types import ControllerConfig, Json
from antlion.validation import MapValidator
from libs.proc import job

MOBLY_CONTROLLER_CONFIG_NAME: str = "Attenuator"
ACTS_CONTROLLER_REFERENCE_NAME = "attenuators"
_ATTENUATOR_OPEN_RETRIES = 3


class Model(enum.StrEnum):
    AEROFLEX_TELNET = "aeroflex.telnet"
    MINICIRCUITS_HTTP = "minicircuits.http"
    MINICIRCUITS_TELNET = "minicircuits.telnet"

    def create(self, instrument_count: int) -> AttenuatorInstrument:
        match self:
            case Model.AEROFLEX_TELNET:
                import antlion.controllers.attenuator_lib.aeroflex.telnet

                return antlion.controllers.attenuator_lib.aeroflex.telnet.AttenuatorInstrument(
                    instrument_count
                )
            case Model.MINICIRCUITS_HTTP:
                import antlion.controllers.attenuator_lib.minicircuits.http

                return antlion.controllers.attenuator_lib.minicircuits.http.AttenuatorInstrument(
                    instrument_count
                )
            case Model.MINICIRCUITS_TELNET:
                import antlion.controllers.attenuator_lib.minicircuits.telnet

                return antlion.controllers.attenuator_lib.minicircuits.telnet.AttenuatorInstrument(
                    instrument_count
                )


def create(configs: list[ControllerConfig]) -> list[Attenuator]:
    attenuators: list[Attenuator] = []
    for config in configs:
        c = MapValidator(config)
        attn_model = c.get(str, "Model")
        protocol = c.get(str, "Protocol", "telnet")
        model = Model(f"{attn_model}.{protocol}")

        instrument_count = c.get(int, "InstrumentCount")
        attenuator_instrument = model.create(instrument_count)

        address = c.get(str, "Address")
        port = c.get(int, "Port")

        for attempt_number in range(1, _ATTENUATOR_OPEN_RETRIES + 1):
            try:
                attenuator_instrument.open(address, port)
            except Exception as e:
                logging.error(
                    "Attempt %s to open connection to attenuator " "failed: %s",
                    attempt_number,
                    e,
                )
                if attempt_number == _ATTENUATOR_OPEN_RETRIES:
                    ping_output = job.run(
                        f"ping {address} -c 1 -w 1", ignore_status=True
                    )
                    if ping_output.returncode == 1:
                        logging.error(
                            "Unable to ping attenuator at %s", address
                        )
                    else:
                        logging.error("Able to ping attenuator at %s", address)
                        job.run(
                            ["telnet", address, str(port)],
                            stdin=b"q",
                            ignore_status=True,
                        )
                    raise
        for i in range(instrument_count):
            attenuators.append(Attenuator(attenuator_instrument, idx=i))
    return attenuators


def destroy(objects: list[Attenuator]) -> None:
    for attn in objects:
        attn.instrument.close()


def get_info(objects: list[Attenuator]) -> list[Json]:
    """Get information on a list of Attenuator objects.

    Args:
        attenuators: A list of Attenuator objects.

    Returns:
        A list of dict, each representing info for Attenuator objects.
    """
    return [
        {
            "Address": attenuator.instrument.address,
            "Attenuator_Port": attenuator.idx,
        }
        for attenuator in objects
    ]


def get_attenuators_for_device(
    device_attenuator_configs: list[ControllerConfig],
    attenuators: list[Attenuator],
    attenuator_key: str,
) -> list[Attenuator]:
    """Gets the list of attenuators associated to a specified device and builds
    a list of the attenuator objects associated to the ip address in the
    device's section of the ACTS config and the Attenuator's IP address.  In the
    example below the access point object has an attenuator dictionary with
    IP address associated to an attenuator object.  The address is the only
    mandatory field and the 'attenuator_ports_wifi_2g' and
    'attenuator_ports_wifi_5g' are the attenuator_key specified above.  These
    can be anything and is sent in as a parameter to this function.  The numbers
    in the list are ports that are in the attenuator object.  Below is an
    standard Access_Point object and the link to a standard Attenuator object.
    Notice the link is the IP address, which is why the IP address is mandatory.

    "AccessPoint": [
        {
          "ssh_config": {
            "user": "root",
            "host": "192.168.42.210"
          },
          "Attenuator": [
            {
              "Address": "192.168.42.200",
              "attenuator_ports_wifi_2g": [
                0,
                1,
                3
              ],
              "attenuator_ports_wifi_5g": [
                0,
                1
              ]
            }
          ]
        }
      ],
      "Attenuator": [
        {
          "Model": "minicircuits",
          "InstrumentCount": 4,
          "Address": "192.168.42.200",
          "Port": 23
        }
      ]
    Args:
        device_attenuator_configs: A list of attenuators config information in
            the acts config that are associated a particular device.
        attenuators: A list of all of the available attenuators objects
            in the testbed.
        attenuator_key: A string that is the key to search in the device's
            configuration.

    Returns:
        A list of attenuator objects for the specified device and the key in
        that device's config.
    """
    attenuator_list = []
    for device_attenuator_config in device_attenuator_configs:
        c = MapValidator(device_attenuator_config)
        ports = c.list(attenuator_key).all(int)
        for port in ports:
            for attenuator in attenuators:
                if (
                    attenuator.instrument.address
                    == device_attenuator_config["Address"]
                    and attenuator.idx is port
                ):
                    attenuator_list.append(attenuator)
    return attenuator_list


#
# Classes for accessing, managing, and manipulating attenuators.
#
# Users will instantiate a specific child class, but almost all operation should
# be performed on the methods and data members defined here in the base classes
# or the wrapper classes.
#


class AttenuatorError(Exception):
    """Base class for all errors generated by Attenuator-related modules."""


class InvalidDataError(AttenuatorError):
    """ "Raised when an unexpected result is seen on the transport layer.

    When this exception is seen, closing an re-opening the link to the
    attenuator instrument is probably necessary. Something has gone wrong in
    the transport.
    """


class InvalidOperationError(AttenuatorError):
    """Raised when the attenuator's state does not allow the given operation.

    Certain methods may only be accessed when the instance upon which they are
    invoked is in a certain state. This indicates that the object is not in the
    correct state for a method to be called.
    """


INVALID_MAX_ATTEN: float = 999.9


@runtime_checkable
class AttenuatorInstrument(Protocol):
    """Defines the primitive behavior of all attenuator instruments.

    The AttenuatorInstrument class is designed to provide a simple low-level
    interface for accessing any step attenuator instrument comprised of one or
    more attenuators and a controller. All AttenuatorInstruments should override
    all the methods below and call AttenuatorInstrument.__init__ in their
    constructors. Outside of setup/teardown, devices should be accessed via
    this generic "interface".
    """

    @property
    def address(self) -> str | None:
        """Return the address to the attenuator."""
        ...

    @property
    def num_atten(self) -> int:
        """Return the index used to identify this attenuator in an instrument."""
        ...

    @property
    def max_atten(self) -> float:
        """Return the maximum allowed attenuation value."""
        ...

    def open(self, host: str, port: int, timeout_sec: int = 5) -> None:
        """Initiate a connection to the attenuator.

        Args:
            host: A valid hostname to an attenuator
            port: Port number to attempt connection
            timeout_sec: Seconds to wait to initiate a connection
        """
        ...

    def close(self) -> None:
        """Close the connection to the attenuator."""
        ...

    def set_atten(
        self, idx: int, value: float, strict: bool = True, retry: bool = False
    ) -> None:
        """Sets the attenuation given its index in the instrument.

        Args:
            idx: Index used to identify a particular attenuator in an instrument
            value: Value for nominal attenuation to be set
            strict: If True, raise an error when given out of bounds attenuation
            retry: If True, command will be retried if possible
        """
        ...

    def get_atten(self, idx: int, retry: bool = False) -> float:
        """Returns the current attenuation given its index in the instrument.

        Args:
            idx: Index used to identify a particular attenuator in an instrument
            retry: If True, command will be retried if possible

        Returns:
            The current attenuation value
        """
        ...


class Attenuator(object):
    """An object representing a single attenuator in a remote instrument.

    A user wishing to abstract the mapping of attenuators to physical
    instruments should use this class, which provides an object that abstracts
    the physical implementation and allows the user to think only of attenuators
    regardless of their location.
    """

    def __init__(
        self, instrument: AttenuatorInstrument, idx: int = 0, offset: int = 0
    ) -> None:
        """This is the constructor for Attenuator

        Args:
            instrument: Reference to an AttenuatorInstrument on which the
                Attenuator resides
            idx: This zero-based index is the identifier for a particular
                attenuator in an instrument.
            offset: A power offset value for the attenuator to be used when
                performing future operations. This could be used for either
                calibration or to allow group operations with offsets between
                various attenuators.

        Raises:
            TypeError if an invalid AttenuatorInstrument is passed in.
            IndexError if the index is out of range.
        """
        if not isinstance(instrument, AttenuatorInstrument):
            raise TypeError("Must provide an Attenuator Instrument Ref")
        self.instrument = instrument
        self.idx = idx
        self.offset = offset

        if self.idx >= instrument.num_atten:
            raise IndexError(
                "Attenuator index out of range for attenuator instrument"
            )

    def set_atten(
        self, value: float, strict: bool = True, retry: bool = False
    ) -> None:
        """Sets the attenuation.

        Args:
            value: A floating point value for nominal attenuation to be set.
            strict: if True, function raises an error when given out of
                bounds attenuation values, if false, the function sets out of
                bounds values to 0 or max_atten.
            retry: if True, command will be retried if possible

        Raises:
            ValueError if value + offset is greater than the maximum value.
        """
        if value + self.offset > self.instrument.max_atten and strict:
            raise ValueError(
                "Attenuator Value+Offset greater than Max Attenuation!"
            )

        self.instrument.set_atten(
            self.idx, value + self.offset, strict=strict, retry=retry
        )

    def get_atten(self, retry: bool = False) -> float:
        """Returns the attenuation as a float, normalized by the offset."""
        return self.instrument.get_atten(self.idx, retry) - self.offset

    def get_max_atten(self) -> float:
        """Returns the max attenuation as a float, normalized by the offset."""
        if self.instrument.max_atten == INVALID_MAX_ATTEN:
            raise ValueError("Invalid Max Attenuator Value")

        return self.instrument.max_atten - self.offset
