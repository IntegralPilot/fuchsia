#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Protocol, TypeAlias, TypeVar

Json: TypeAlias = (
    dict[str, "Json"] | list["Json"] | str | int | float | bool | None
)
"""JSON serializable data."""

ControllerConfig: TypeAlias = dict[str, Json]
"""Mobly configuration specific to a controller.

Defined in the Mobly config under TestBeds -> Controllers ->
<MOBLY_CONTROLLER_CONFIG_NAME>.
"""

_T = TypeVar("_T")


class Controller(Protocol[_T]):
    MOBLY_CONTROLLER_CONFIG_NAME: str
    """Key used to get this controller's config from the Mobly config."""

    def create(self, configs: list[ControllerConfig]) -> list[_T]:
        """Create controller objects from configurations.

        Args:
            configs: A list of serialized data like string/dict. Each element of
                the list is a configuration for a controller object.

        Returns:
          A list of controller objects.
        """

    def destroy(self, objects: list[_T]) -> None:
        """Destroys controller objects.

        Each controller object shall be properly cleaned up and all the
        resources held should be released, e.g. memory allocation, sockets, file
        handlers etc.

        Args:
            objects: A list of controller objects created by the create
                function.
        """

    def get_info(self, objects: list[_T]) -> list[Json]:
        """Gets info from the controller objects.

        The info will be included in test_summary.yaml under the key
        'ControllerInfo'. Such information could include unique ID, version, or
        anything that could be useful for describing the test bed and debugging.

        Args:
            objects: A list of controller objects created by the create
                function.

        Returns:
            A list of json serializable objects: each represents the info of a
            controller object. The order of the info object should follow that
            of the input objects.
        """
        return []
