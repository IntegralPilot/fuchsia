#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from antlion import utils
from antlion.libs.ota.ota_runners import ota_runner_factory

"""Maps AndroidDevices to OtaRunners."""
ota_runners = {}


def initialize(user_params, android_devices):
    """Initialize OtaRunners for each device.

    Args:
        user_params: The user_params from the ACTS config.
        android_devices: The android_devices in the test.
    """
    for ad in android_devices:
        ota_runners[ad] = ota_runner_factory.create_from_configs(
            user_params, ad
        )


def _check_initialization(android_device):
    """Check if a given device was initialized."""
    if android_device not in ota_runners:
        raise KeyError(
            'Android Device with serial "%s" has not been '
            "initialized for OTA Updates. Did you forget to call"
            "ota_updater.initialize()?" % android_device.serial
        )


def update(android_device, ignore_update_errors=False):
    """Update a given AndroidDevice.

    Args:
        android_device: The device to update
        ignore_update_errors: Whether or not to ignore update errors such as
           no more updates available for a given device. Default is false.
    Throws:
        OtaError if ignore_update_errors is false and the OtaRunner has run out
        of packages to update the phone with.
    """
    _check_initialization(android_device)
    ota_runners[android_device].validate_update()
    try:
        ota_runners[android_device].update()
    except Exception as e:
        if ignore_update_errors:
            return
        android_device.log.error(e)
        android_device.take_bug_report(
            "ota_update", utils.get_current_epoch_time()
        )
        raise e


def can_update(android_device):
    """Whether or not a device can be updated."""
    _check_initialization(android_device)
    return ota_runners[android_device].can_update()
