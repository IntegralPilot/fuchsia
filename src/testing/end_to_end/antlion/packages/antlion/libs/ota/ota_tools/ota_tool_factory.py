#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def, attr-defined, import-untyped, var-annotated"
from antlion.libs.ota.ota_tools.adb_sideload_ota_tool import AdbSideloadOtaTool
from antlion.libs.ota.ota_tools.update_device_ota_tool import (
    UpdateDeviceOtaTool,
)

_CONSTRUCTORS = {
    AdbSideloadOtaTool.__name__: lambda command: AdbSideloadOtaTool(command),
    UpdateDeviceOtaTool.__name__: lambda command: UpdateDeviceOtaTool(command),
}
_constructed_tools = {}


def create(ota_tool_class, command):
    """Returns an OtaTool with the given class name.

    If the tool has already been created, the existing instance will be
    returned.

    Args:
        ota_tool_class: the class/type of the tool you wish to use.
        command: the command line tool being used.

    Returns:
        An OtaTool.
    """
    if ota_tool_class in _constructed_tools:
        return _constructed_tools[ota_tool_class]

    if ota_tool_class not in _CONSTRUCTORS:
        raise KeyError(
            "Given Ota Tool class name does not match a known "
            'name. Found "%s". Expected any of %s. If this tool '
            "does exist, add it to the _CONSTRUCTORS dict in this "
            "module." % (ota_tool_class, _CONSTRUCTORS.keys())
        )

    new_update_tool = _CONSTRUCTORS[ota_tool_class](command)
    _constructed_tools[ota_tool_class] = new_update_tool

    return new_update_tool
