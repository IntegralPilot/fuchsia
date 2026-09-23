# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import os
from enum import Enum
from typing import Callable

import fx_cmd
from async_utils.command import (
    AsyncCommand,
    CommandOutput,
    StderrEvent,
    StdoutEvent,
)

logger = logging.getLogger(__name__)


class FfxOutputFormat(Enum):
    TEXT = 0
    JSON = 1
    PRETTY_JSON = 2


class FfxCmd(fx_cmd.ExecutableCommand):
    def __init__(
        self,
        inner: fx_cmd.ExecutableCommand | None = None,
        output_format: FfxOutputFormat = FfxOutputFormat.TEXT,
    ):
        """Wrapper for executing ffx commands through fx.

        Args:
            inner (ExecutableCommand, optional): Configured command wrapper. If unset, use a default FxCmd.
            output_format (FfxOutputFormat, optional): Output format for ffx. Defaults to FfxOutputFormat.TEXT.
        """
        self._ffx_prefix_args: list[str] = []
        match output_format:
            case FfxOutputFormat.TEXT:
                pass
            case FfxOutputFormat.JSON:
                self._ffx_prefix_args = ["--machine", "json"]
            case FfxOutputFormat.PRETTY_JSON:
                self._ffx_prefix_args = ["--machine", "json-pretty"]
            case _:
                assert False, f"Unknown output format {output_format}"

        if inner is None:
            inner = fx_cmd.FxCmd()
        self._inner: fx_cmd.ExecutableCommand = inner

    @staticmethod
    def create_test_inner(
        path_to_ffx: str,
        *extra_args: str,
    ) -> fx_cmd.ExecutableCommand:
        """Create an ExecutableCommand that calls a specific ffx for tests.

        Args:
            path_to_ffx (str): Path to an ffx binary to execute.
            extra_args (str): Extra arguments to pass to ffx.

        Returns:
            ExecutableCommand: Configured wrapper for use as FfxCmd inner.
        """
        if not os.path.isfile(path_to_ffx):
            raise RuntimeError(
                f"Expected ffx at path {path_to_ffx}, but it is not a file"
            )

        class TestExecutor(fx_cmd.ExecutableCommand):
            async def start(self, *args: str) -> AsyncCommand:
                logger.debug("Processing command line...")
                index_of_ffx = None
                for i, val in enumerate(args):
                    if val == "ffx":
                        index_of_ffx = i
                        break
                logger.debug(f"Old command line was {args}")
                remaining_args = (
                    args[index_of_ffx + 1 :]
                    if index_of_ffx is not None
                    else args
                )
                command_line = [path_to_ffx, *extra_args, *remaining_args]
                logger.info(f"Executing ffx for test\n  {command_line}")
                return await AsyncCommand.create(*command_line)

        return TestExecutor()

    def command_line(self, *args: str) -> list[str]:
        """Format a command line with ffx-specific settings.

        Returns:
            list[str]: Formatted command line.
        """
        return ["ffx"] + self._ffx_prefix_args + list(args)

    # overrides base class
    async def start(self, *args: str) -> AsyncCommand:
        return await self._inner.start(*self.command_line(*args))

    # overrides base class
    async def sync(
        self,
        *args: str,
        stdout_callback: Callable[[StdoutEvent], None] | None = None,
        stderr_callback: Callable[[StderrEvent], None] | None = None,
    ) -> CommandOutput:
        return await self._inner.sync(
            *self.command_line(*args),
            stdout_callback=stdout_callback,
            stderr_callback=stderr_callback,
        )
