# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data types that match the machine output of ffx."""

import enum
from dataclasses import dataclass, field
from typing import Any

from honeydew.typing.custom_types import IpPort

# LINT.IfChange


@dataclass(frozen=True)
class TargetData:
    """Dataclass for target level information.

    Args:
        name: Node name of the target device.
        ssh_address: SSH address of the target device.
        last_reboot_graceful: True if the last reboot was graceful.
        last_reboot_reason: Reason for last reboot, if available.
        uptime_nanos: Target device update in nanoseconds.
    """

    name: str
    ssh_address: IpPort | None
    last_reboot_graceful: bool
    last_reboot_reason: str | None
    uptime_nanos: int

    def __init__(self, **kwargs: Any) -> None:
        for k, v in kwargs.items():
            if k == "ssh_address":
                if v is not None:
                    ip = IpPort.create_using_ip_and_port(
                        f"{v['host']}:{v['port']}"
                    )
                    object.__setattr__(self, k, ip)
                else:
                    object.__setattr__(self, k, None)
            else:
                object.__setattr__(self, k, v)


@dataclass(frozen=True)
class BoardData:
    """Information about the hardware board of the target device.

    Args:
        name: Board name, if known.
        revision: oard revision information, if known.
        instruction_set: Instruction set, if known.
    """

    name: str | None
    revision: str | None
    instruction_set: str | None


@dataclass(frozen=True)
class DeviceData:
    """Information about the product level device.

    Args:
        serial_number: Serial number, if known.
        retail_sku: SKU if known.
        retail_demo: Device configured for demo mode.
        device_id: Device ID for use in feedback messages.
    """

    serial_number: str | None
    retail_sku: str | None
    retail_demo: bool | None
    device_id: str | None


@dataclass(frozen=True)
class ProductData:
    """Product information.

    Args:
        audio_amplifier: Type of audio amp, if known.
        build_date: Product build date.
        build_name: Product build name
        colorway: Product's color scheme description.
        display: Display information, if known.
        emmc_storage: Size of EMMC storage.
        language: Product Language.
        regulatory_domain: Regulatory domain designation.
        locale_list: Supported locales.
        manufacturer: Manufacturer name, if known.
        microphone:  Type of microphone.
        model: Product Model information.
        name:  Product name.
        nand_storage: Size of NAND storage.
        memory: Amount of RAM.
        sku: SKU of the board.
    """

    audio_amplifier: str | None
    build_date: str | None
    build_name: str | None
    colorway: str | None
    display: str | None
    emmc_storage: str | None
    language: str | None
    regulatory_domain: str | None
    locale_list: list[str]
    manufacturer: str | None
    microphone: str | None
    model: str | None
    name: str | None
    nand_storage: str | None
    memory: str | None
    sku: str | None


@dataclass(frozen=True)
class UpdateData:
    """OTA channel information."""

    current_channel: str
    next_channel: str | None


@dataclass(frozen=True)
class BuildData:
    """Information about the Fuchsia build.

    Args:
        version: Build version, if known.
        product: Fuchsia product.
        board: Board targeted for this build.
        commit: Integration commit date.
    """

    version: str | None
    product: str | None
    board: str | None
    commit: str | None


@dataclass(frozen=True)
class TargetInfoData:
    """Information returned from `ffx target show`."""

    target: TargetData
    board: BoardData
    device: DeviceData
    product: ProductData
    update: UpdateData
    build: BuildData

    def __init__(self, **kwargs: Any) -> None:
        """Initialize the member attributes.

        Unfortunately, the initialization of a class object from kwargs
        does not get applied recursively, so each member needs to be set
        to the correct object type when initializing.
        """
        for k, v in kwargs.items():
            if k == "target":
                object.__setattr__(self, k, TargetData(**v))
            if k == "board":
                object.__setattr__(self, k, BoardData(**v))
            if k == "device":
                object.__setattr__(self, k, DeviceData(**v))
            if k == "product":
                object.__setattr__(self, k, ProductData(**v))
            if k == "update":
                object.__setattr__(self, k, UpdateData(**v))
            if k == "build":
                object.__setattr__(self, k, BuildData(**v))


# LINT.ThenChange(/src/developer/ffx/plugins/target/show/src/show.rs)


# LINT.IfChange
@dataclass(frozen=True)
class MonitorTargetInfo:
    """Represents a single target's information from ffx monitor status.

    Attributes:
        nodename: The node name of the target.
        target_state: The current state of the target (e.g., "Product").
        rcs_state: Whether RCS is connected ("Y" or "N").
        addresses: A list of addresses associated with the target.
        target_type: The type of the target (e.g., "core.x64").
        is_default: Whether this is the default target.
        is_manual: Whether this target is added manually.
        serial: The serial number of the target.
    """

    nodename: str = ""
    target_state: str = "Disconnected"
    rcs_state: str = "N"
    addresses: list[IpPort] = field(default_factory=list)
    target_type: str = "."
    is_default: bool = False
    is_manual: bool = False
    serial: str = "<unknown>"


# LINT.ThenChange(//src/developer/ffx/lib/target/src/info.rs)


class MachineFormat(enum.StrEnum):
    """Supported machine formats"""

    # All ffx subtools support "--machine raw", so that should be used
    # if the tool does not support structured output. Best practice is
    # to add a comment above uses of MachineFormat.RAW, such as:
    # * the command has not yet been ported (provide a TODO(bug))
    # * it makes no sense to provide structured output (e.g. a command
    #   such as "ffx target shell" that cannot have a useful schema)
    # * inspecting the structured output requires too much complexity
    RAW = "raw"
    JSON = "json"
    JSON_PRETTY = "json-pretty"
