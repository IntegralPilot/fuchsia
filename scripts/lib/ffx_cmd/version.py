# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
from dataclasses import dataclass

import fx_cmd
from async_utils.command import CommandOutput

from .lib import FfxCmd, FfxOutputFormat


class VersionCommand(fx_cmd.CommandTransformer[None, "VersionOutput"]):
    """Transformer for `ffx version` command."""

    # overrides base class
    def _to_output(self, output: CommandOutput) -> "VersionOutput":
        data = json.loads(output.stdout)
        return VersionOutput(api_level=data["tool_version"]["api_level"])


@dataclass
class VersionOutput:
    """Schema for ffx version output"""

    api_level: int


def version(inner: fx_cmd.ExecutableCommand | None = None) -> VersionCommand:
    return VersionCommand(
        "version",
        inner=FfxCmd(inner=inner, output_format=FfxOutputFormat.JSON),
    )
