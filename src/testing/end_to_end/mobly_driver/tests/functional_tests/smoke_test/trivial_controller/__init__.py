# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Trivial Mobly controller.

This implements the required APIs to make this module compatible with Mobly.
 - create()
 - destroy()
 - MOBLY_CONTROLLER_CONFIG_NAME
"""

from typing import Any

MOBLY_CONTROLLER_CONFIG_NAME = "TrivialController"
CONFIG_KEY_NAME = "name"


class TrivialController:
    def __init__(self, config: dict[str, Any]) -> None:
        self._name = config[CONFIG_KEY_NAME]


def create(configs: list[dict[str, Any]]) -> list[TrivialController]:
    fds = []
    for config in configs:
        fds.append(TrivialController(config))
    return fds


def destroy(unused_controllers: list[TrivialController]) -> None:
    pass
