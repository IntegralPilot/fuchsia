# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


import fx_cmd
from async_utils.command import CommandOutput
from fuchsia_inspect import InspectDataCollection

from .lib import FfxCmd, FfxOutputFormat


class InspectCommand(fx_cmd.CommandTransformer[None, InspectDataCollection]):
    """Transformer for `ffx version` command."""

    # overrides base class
    def _to_output(self, output: CommandOutput) -> InspectDataCollection:
        return InspectDataCollection.from_json_list(output.stdout)


def inspect(
    *selectors: str, inner: fx_cmd.ExecutableCommand | None = None
) -> InspectCommand:
    """Select Inspect data from a Fuchsia device.

    Args:
        *selectors (str). The selectors to fetch.
        inner (fx_cmd.ExecutableCommand | None, optional): An inner
            command wrapper. Defaults to creating a default-configured
            fx invocation of ffx.

    Returns:
        InspectCommand: Wrapper for the command, which can be
            synchronously or asynchronously executed.
    """
    return InspectCommand(
        "inspect",
        "show",
        *selectors,
        inner=FfxCmd(inner=inner, output_format=FfxOutputFormat.JSON),
    )
