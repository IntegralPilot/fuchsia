# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia Inspect utilities."""

import json
from typing import Any

from honeydew.transports.ffx.ffx import FFX as ffx_interface


class Inspect:
    """ffx inspect helper"""

    def __init__(self, ffx: ffx_interface) -> None:
        self._ffx: ffx_interface = ffx

    def show(
        self,
        selector: str,
    ) -> list[dict[str, Any]]:
        """Issues an `ffx inspect show [selector]` command, returning the parsed json output"""
        # This creates a lot of log spam: b/326273353
        txt_output = self._ffx.run(
            cmd=["--machine", "json", "inspect", "show", selector],
            log_output=False,
        )
        json_output: list[dict[str, Any]] = json.loads(txt_output)
        return json_output

    def show_from_component(
        self,
        component_query: str,
    ) -> list[dict[str, Any]]:
        """Issues an `ffx inspect show [manifest]` command, returning the parsed json output"""
        # This creates a lot of log spam: b/326273353
        txt_output = self._ffx.run(
            cmd=[
                "--machine",
                "json",
                "inspect",
                "show",
                component_query,
            ],
            log_output=False,
        )
        json_output: list[dict[str, Any]] = json.loads(txt_output)
        return json_output

    def show_text(self, selector: str) -> str:
        """Issues an `ffx inspect show [selector]` command, returning the parsed json output"""
        # This creates a lot of log spam: b/326273353
        return self._ffx.run(
            cmd=["inspect", "show", selector],
            log_output=False,
        )
