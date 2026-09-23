# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tracing affordance implementation using ffx."""

import logging
import os
import shutil
import tempfile
from datetime import datetime

import fidl_fuchsia_tracing as f_tracing
from honeydew import affordances_capable
from honeydew.affordances.tracing import tracing
from honeydew.affordances.tracing.errors import TracingError, TracingStateError
from honeydew.transports.ffx import ffx
from honeydew.transports.ffx.errors import FfxCommandError

_LOGGER: logging.Logger = logging.getLogger(__name__)

DEFAULT_CATEGORIES: list[str] = [
    "app",
    "audio",
    "benchmark",
    "blobfs",
    "gfx",
    "input",
    "magma",
    "modular",
    "system_metrics",
    "view",
]


class TracingUsingFfx(tracing.Tracing):
    """Async Tracing affordance implementation using ffx."""

    def __init__(
        self,
        device_name: str,
        ffx_inst: ffx.FFX,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        self._name: str = device_name
        self._ffx: ffx.FFX = ffx_inst
        self._reboot_affordance = reboot_affordance
        self._session_initialized: bool = False
        self._session_started: bool = False
        self._buffer_size: int | None = None
        self._categories: list[str] | None = None
        self._buffering_mode: f_tracing.BufferingMode | None = None
        self._compression: bool | None = None
        self._temp_trace_file: str | None = None

        reboot_affordance.register_for_on_device_boot(fn=self.reboot_handler)

    async def reboot_handler(self) -> None:
        """A method for handling reboots meant to be passed to the reboot affordance."""
        _LOGGER.info("Received device boot signal. Resetting")
        await self._reset_state()

    async def _reset_state(self) -> None:
        """Resets internal state tracking variables to correspond to an inactive
        state; i.e. tracing uninitialized and not started.
        """
        try:
            await self.terminate()
        except TracingError:
            pass

        self._buffer_size = None
        self._categories = None
        self._buffering_mode = None
        self._compression = None

    def verify_supported(self) -> None:
        """Check if Trace is supported on the DUT.
        Raises:
            NotSupportedError: Tracing affordance is not supported by Fuchsia device.
        """
        # TODO(http://b/409625325): Implement the method logic

    def is_active(self) -> bool:
        """Checks if there is a currently active trace.

        Returns:
            True if the tracing is currently running, False otherwise.
        """
        return self._session_started

    def is_session_initialized(self) -> bool:
        """Checks if the session is initialized or not."""
        return self._session_initialized

    def initialize(
        self,
        categories: list[str] | None = None,
        buffer_size: int | None = None,
        start_timeout_milliseconds: int | None = None,
        buffering_mode: f_tracing.BufferingMode | None = None,
        defer_transfer: bool | None = None,
        compression: bool | None = True,
    ) -> None:
        """Initializes a trace session.

        Args:
            categories: list of categories to trace.
            buffer_size: buffer size to use in MB.
            start_timeout_milliseconds: milliseconds to wait for trace providers
                to acknowledge that they've started tracing. NB: trace providers
                that don't ACK by this deadline may still emit tracing events
                starting at some later point.
            buffering_mode: Tells tracing providers how to buffer data
                ONESHOT - When the buffer fills the provider drops subsequent records
                CIRCULAR - When the buffer fills, older records are discarded to make space
                STREAMING - Data is streamed back to the trace_manager. Providers may still drop
                            records if events are produced faster than they can be streamed
            defer_transfer: Ignored by this implementation. Instead, this behavior is triggered
                automatically when using STREAMING mode.
            compression: If true, compress the trace data. Defaults to True.

        Raises:
            TracingStateError: When trace session is already initialized.
        """
        if compression is None:
            compression = True

        if categories is None:
            categories = DEFAULT_CATEGORIES
        else:
            new_categories: set[str] = set()
            for category in categories:
                if category == "#default" or category == "default":
                    new_categories.update(DEFAULT_CATEGORIES)
                else:
                    new_categories.add(category)
            categories = list(new_categories)

        if self._session_initialized:
            raise TracingStateError(
                f"Trace session is already initialized on {self._name}. "
                "Can be initialized only once"
            )
        _LOGGER.info("Initializing trace session via FFX on '%s'", self._name)

        self._categories = categories
        self._buffer_size = buffer_size
        self._buffering_mode = buffering_mode
        self._compression = compression
        self._session_initialized = True

    async def start(self) -> None:
        """Starts tracing."""
        if not self._session_initialized:
            raise TracingStateError(
                f"Trace session is not initialized on {self._name}"
            )
        if self._session_started:
            raise TracingStateError(
                f"Trace session is already started on {self._name}"
            )
        _LOGGER.info("Starting trace session via FFX on '%s'", self._name)

        # While `ffx trace start` can take a duration, we run it infinitely with
        # --background instead so that it will keep going until we stop it.
        cmd = ["trace", "start", "--background"]
        if self._categories:
            cmd.extend(["--categories", ",".join(self._categories)])
        if self._buffer_size is not None:
            cmd.extend(["--buffer-size", str(self._buffer_size)])
        if self._buffering_mode is not None:
            if self._buffering_mode == f_tracing.BufferingMode.ONESHOT:
                cmd.extend(["--buffering-mode", "oneshot"])
            elif self._buffering_mode == f_tracing.BufferingMode.CIRCULAR:
                cmd.extend(["--buffering-mode", "circular"])
            elif self._buffering_mode == f_tracing.BufferingMode.STREAMING:
                cmd.extend(["--buffering-mode", "streaming"])

        if not self._compression:
            cmd.append("--nocompress")

        try:
            self._ffx.run(cmd)
        except FfxCommandError as err:
            raise TracingError(
                f"Failed to start FFX trace on {self._name}: {err}"
            ) from err

        self._session_started = True

    async def stop(self) -> None:
        """Stops the current trace."""
        if not self._session_started:
            raise TracingStateError(
                f"Trace session is not started on {self._name}"
            )
        _LOGGER.info("Stopping trace session via FFX on '%s'", self._name)

        # FFX trace stop does both stopping and downloading.
        # We save it to a tempfile, so terminate_and_download can move it later.
        fd, temp_file_path = tempfile.mkstemp(suffix=".fxt")
        os.close(fd)

        try:
            self._ffx.run(["trace", "stop", "--output", temp_file_path])
        except FfxCommandError as err:
            raise TracingError(
                f"Failed to stop FFX trace on {self._name}: {err}"
            ) from err

        self._temp_trace_file = temp_file_path
        self._session_started = False

    async def terminate(self) -> None:
        """Terminates the trace session."""
        if self._session_started:
            try:
                self._ffx.run(["trace", "stop", "--abort"])
            except FfxCommandError as err:
                raise TracingError(
                    f"Failed to abort FFX trace on {self._name}: {err}"
                ) from err
            self._session_started = False

        if self._temp_trace_file and os.path.exists(self._temp_trace_file):
            try:
                os.remove(self._temp_trace_file)
            except OSError:
                pass
            self._temp_trace_file = None

        self._session_initialized = False

    async def terminate_and_download(
        self, directory: str, trace_file: str | None = None
    ) -> str:
        """Terminates the trace session and downloads the trace data."""
        if not self._session_initialized:
            raise TracingStateError(
                f"Trace session is not initialized on {self._name}"
            )

        if not os.path.isabs(directory):
            raise ValueError(
                f"Provide a valid absolute path to download the trace. Given: {directory}"
            )
        os.makedirs(directory, exist_ok=True)

        if not trace_file:
            now = datetime.now()
            trace_file = (
                f"trace_{self._name}_{now.strftime('%Y-%m-%d-%I-%M-%S-%p')}.fxt"
            )

        dest_path = os.path.join(directory, trace_file)

        if self._session_started:
            await self.stop()

        if self._temp_trace_file and os.path.exists(self._temp_trace_file):
            shutil.move(self._temp_trace_file, dest_path)
            self._temp_trace_file = None
        else:
            raise TracingError(
                f"Trace on {self._name} was not properly stopped or file is missing."
            )

        self._session_initialized = False
        return dest_path
