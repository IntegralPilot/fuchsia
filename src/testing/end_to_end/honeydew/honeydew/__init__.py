# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Honeydew python module."""

import logging
from typing import Any

from honeydew import errors
from honeydew.fuchsia_device import fuchsia_device
from honeydew.transports.ffx.config import FfxConfigData
from honeydew.typing import custom_types

_LOGGER: logging.Logger = logging.getLogger(__name__)

_CUSTOM_FUCHSIA_DEVICE_CLASS: type[
    fuchsia_device.FuchsiaDevice
] = fuchsia_device.FuchsiaDevice


# The return type of this function can change at runtime with register_custom_fuchsia_device which
# modifies _CUSTOM_FUCHSIA_DEVICE_CLASS.
def create_device(
    device_info: custom_types.DeviceInfo,
    ffx_config_data: FfxConfigData,
    # intentionally made this a Dict instead of dataclass to minimize the changes in remaining Lacewing stack every time we need to add a new configuration item
    config: dict[str, Any] | None = None,
) -> fuchsia_device.FuchsiaDevice:
    """Factory method that creates and returns the device class.

    Args:
        device_info: Fuchsia device information.

        ffx_config_data: Ffx configuration that need to be used while running ffx
            commands.

        config: Honeydew device configuration, if any.
            Format:
                {
                    "transports": {
                        <transport_name>: {
                            <key>: <value>,
                            ...
                        },
                        ...
                    },
                    "affordances": {
                        <affordance_name>: {
                            <key>: <value>,
                            ...
                        },
                        ...
                    },
                }
            Example:
                {
                    "transports": {
                        "fuchsia_controller": {
                            "timeout": 30,
                        }
                    },
                    "affordances": {
                        "bluetooth": {
                            "implementation": "fuchsia-controller",
                        },
                    },
                }

    Returns:
        Either a FuchsiaDevice or an instance of the class configured
        with register_custom_fuchsia_device.

    Raises:
        errors.FuchsiaDeviceError: Failed to create Fuchsia device object.
    """
    _LOGGER.debug("create_device has been called with: %s", locals())

    try:
        if device_info.ip_port:
            _LOGGER.info(
                "CAUTION: device_ip_port='%s' argument has been passed. Please "
                "make sure this value associated with the device is persistent "
                "across the reboots. Otherwise, host-target interactions will not "
                "work consistently.",
                device_info.ip_port,
            )

        device_class = get_custom_fuchsia_device()
        return device_class(
            device_info=device_info,
            ffx_config_data=ffx_config_data,
            config=config,
        )
    except errors.HoneydewError as err:
        raise errors.FuchsiaDeviceError(
            f"Failed to create device for '{device_info.name}': {err}"
        ) from err


def register_custom_fuchsia_device(
    fuchsia_device_class: type[fuchsia_device.FuchsiaDevice],
) -> None:
    """Registers a custom fuchsia device class implementation.

    Args:
        fuchsia_device_class: custom fuchsia device class implementation.
    """
    _LOGGER.info(
        "Registering custom FuchsiaDevice class '%s' with Honeydew",
        fuchsia_device_class,
    )
    global _CUSTOM_FUCHSIA_DEVICE_CLASS
    _CUSTOM_FUCHSIA_DEVICE_CLASS = fuchsia_device_class


def get_custom_fuchsia_device() -> type:
    """Returns if any custom fuchsia device class implementation is available. Otherwise, None.

    Returns:
        Custom fuchsia device class
    """
    return _CUSTOM_FUCHSIA_DEVICE_CLASS
