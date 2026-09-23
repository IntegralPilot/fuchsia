# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data types used by PowerSource affordance."""

from __future__ import annotations

import enum
from dataclasses import dataclass

import fidl_fuchsia_hardware_power_source as f_power_source


class SourceType(enum.IntEnum):
    """Corresponds to fuchsia.hardware.power.source.SourceType."""

    AC = 1
    BATTERY = 2
    USB = 3


class Error(enum.IntEnum):
    """Corresponds to fuchsia.hardware.power.source.Error."""

    INTERNAL = 1
    NOT_SUPPORTED = 2
    INVALID_ARGS = 3
    ALREADY_BOUND = 4


@dataclass(frozen=True)
class Disconnected:
    """The node is disconnected or isolated from the power system."""

    @classmethod
    def from_fidl(cls, fidl: f_power_source.Disconnected) -> Disconnected:
        return cls()

    def to_fidl(self) -> f_power_source.Disconnected:
        return f_power_source.Disconnected()


@dataclass(frozen=True)
class SourceRole:
    """The node provides energy to the common system bus."""

    @classmethod
    def from_fidl(cls, fidl: f_power_source.SourceRole) -> SourceRole:
        return cls()

    def to_fidl(self) -> f_power_source.SourceRole:
        return f_power_source.SourceRole()


@dataclass(frozen=True)
class SinkRole:
    """The node consumes or stores energy from the common bus."""

    name: str | None = None
    type: SourceType | None = None

    @classmethod
    def from_fidl(cls, fidl: f_power_source.SinkRole) -> SinkRole:
        source_type = SourceType(fidl.type_) if fidl.type_ is not None else None
        return cls(name=fidl.name, type=source_type)

    def to_fidl(self) -> f_power_source.SinkRole:
        source_type = (
            f_power_source.SourceType(self.type.value)
            if self.type is not None
            else None
        )
        return f_power_source.SinkRole(name=self.name, type_=source_type)


@dataclass(frozen=True)
class Auto:
    """The hardware automatically decides the role based on physical detection."""

    @classmethod
    def from_fidl(cls, fidl: f_power_source.Auto) -> Auto:
        return cls()

    def to_fidl(self) -> f_power_source.Auto:
        return f_power_source.Auto()


@dataclass(frozen=True)
class Role:
    """The role a power node plays in the system."""

    disconnected: Disconnected | None = None
    source: SourceRole | None = None
    sink: SinkRole | None = None
    auto: Auto | None = None

    @classmethod
    def with_disconnected(cls) -> Role:
        return cls(disconnected=Disconnected())

    @classmethod
    def with_source(cls) -> Role:
        return cls(source=SourceRole())

    @classmethod
    def with_sink(
        cls, name: str | None = None, source_type: SourceType | None = None
    ) -> Role:
        return cls(sink=SinkRole(name=name, type=source_type))

    @classmethod
    def with_auto(cls) -> Role:
        return cls(auto=Auto())

    @classmethod
    def from_fidl(cls, fidl: f_power_source.Role) -> Role:
        if fidl.disconnected is not None:
            return cls(disconnected=Disconnected.from_fidl(fidl.disconnected))
        if fidl.source is not None:
            return cls(source=SourceRole.from_fidl(fidl.source))
        if fidl.sink is not None:
            return cls(sink=SinkRole.from_fidl(fidl.sink))
        if fidl.auto is not None:
            return cls(auto=Auto.from_fidl(fidl.auto))
        return cls()

    def to_fidl(self) -> f_power_source.Role:
        if self.disconnected is not None:
            return f_power_source.Role(disconnected=self.disconnected.to_fidl())
        if self.source is not None:
            return f_power_source.Role(source=self.source.to_fidl())
        if self.sink is not None:
            return f_power_source.Role(sink=self.sink.to_fidl())
        if self.auto is not None:
            return f_power_source.Role(auto=self.auto.to_fidl())
        return f_power_source.Role(disconnected=f_power_source.Disconnected())


@dataclass(frozen=True)
class Spec:
    """Static hardware characteristics of a power source."""

    name: str | None = None
    supported_roles: list[Role] | None = None
    manufacturer: str | None = None
    model_name: str | None = None
    type: SourceType | None = None

    @classmethod
    def from_fidl(cls, fidl: f_power_source.Spec) -> Spec:
        source_type = SourceType(fidl.type_) if fidl.type_ is not None else None
        roles = (
            [Role.from_fidl(role) for role in fidl.supported_roles]
            if fidl.supported_roles is not None
            else None
        )
        return cls(
            name=fidl.name,
            supported_roles=roles,
            manufacturer=fidl.manufacturer,
            model_name=fidl.model_name,
            type=source_type,
        )

    def to_fidl(self) -> f_power_source.Spec:
        source_type = (
            f_power_source.SourceType(self.type.value)
            if self.type is not None
            else None
        )
        roles = (
            [role.to_fidl() for role in self.supported_roles]
            if self.supported_roles is not None
            else None
        )
        return f_power_source.Spec(
            name=self.name,
            supported_roles=roles,
            manufacturer=self.manufacturer,
            model_name=self.model_name,
            type_=source_type,
        )


@dataclass(frozen=True)
class Status:
    """Dynamic status of a power source."""

    present: bool | None = None
    voltage_uv: int | None = None
    current_ua: int | None = None
    current_role: Role | None = None

    @classmethod
    def from_fidl(cls, fidl: f_power_source.Status) -> Status:
        current_role = (
            Role.from_fidl(fidl.current_role)
            if fidl.current_role is not None
            else None
        )
        return cls(
            present=fidl.present,
            voltage_uv=fidl.voltage_uv,
            current_ua=fidl.current_ua,
            current_role=current_role,
        )

    def to_fidl(self) -> f_power_source.Status:
        current_role = (
            self.current_role.to_fidl()
            if self.current_role is not None
            else None
        )
        return f_power_source.Status(
            present=self.present,
            voltage_uv=self.voltage_uv,
            current_ua=self.current_ua,
            current_role=current_role,
        )
