# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Tracing affordance."""

import abc
import os
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager

import fidl_fuchsia_tracing as f_tracing


class Tracing(abc.ABC):
    """Abstract base class for an async Tracing affordance."""

    @abc.abstractmethod
    def initialize(
        self,
        categories: list[str] | None = None,
        buffer_size: int | None = None,
        start_timeout_milliseconds: int | None = None,
        buffering_mode: f_tracing.BufferingMode | None = None,
        defer_transfer: bool | None = None,
        compression: bool | None = None,
    ) -> None:
        """Initializes a trace sessions.

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
            defer_transfer: If true, the trace_manager will delay sending data until tracing has stopped
            compression: If true, compress the trace data.

        Raises:
            TracingStateError: When trace session is already initialized.
            TracingError: On FIDL communication failure.
        """

    @abc.abstractmethod
    def is_active(self) -> bool:
        """Checks if there is a currently active trace.

        Returns:
            True if the tracing is currently running, False otherwise.
        """

    @abc.abstractmethod
    def is_session_initialized(self) -> bool:
        """Checks if the session is initialized or not.

        Returns:
            True if the session is initialized, False otherwise.
        """

    @abc.abstractmethod
    async def start(self) -> None:
        """Starts tracing.

        Raises:
           TracingStateError: When trace session is not initialized or
               already started.
           TracingError: On FIDL communication failure.
        """

    @abc.abstractmethod
    async def stop(self) -> None:
        """Stops the current trace.

        Raises:
           TracingStateError: When trace session is not initialized or
               not started.
           TracingError: On FIDL communication failure.
        """

    @abc.abstractmethod
    async def terminate(self) -> None:
        """Terminates the trace session.."""

    @abc.abstractmethod
    async def terminate_and_download(
        self, directory: str, trace_file: str | None = None
    ) -> str:
        """Terminates the trace session and downloads the trace data to the
            specified directory.

        Args:
            directory: Absolute path on the host where trace file will be
                saved. If this directory does not exist, this method will create
                it.

            trace_file: Name of the output trace file.
                If not provided, API will create a name using
                "Trace_{device_name}_{'%Y-%m-%d-%I-%M-%S-%p'}" format.

        Returns:
            The path to the trace file.

         Raises:
            TracingStateError: When trace session is not initialized or
                already started.
            TracingError: When the method fails to collect trace data.
        """

    @asynccontextmanager
    async def trace_session(
        self,
        categories: list[str] | None = None,
        buffer_size: int | None = None,
        download: bool = False,
        directory: str | None = None,
        trace_file: str | None = None,
        start_timeout_milliseconds: int | None = None,
        buffering_mode: f_tracing.BufferingMode | None = None,
        defer_transfer: bool | None = None,
        compression: bool | None = None,
    ) -> AsyncGenerator[None, None]:
        """Starts and captures trace data within a context and automatically
            cleans up trace sessions, making it easier to capture trace data.

        Args:
            categories: list of categories to trace.
            buffer_size: buffer size to use in MB.
            download: True to download trace, False to skip trace download.
            directory: Absolute path on the host where trace file will be
                saved. Only required if `download` is True.
            trace_file: Name of the output trace file. Only required if
                `download` is True.
            start_timeout_milliseconds: milliseconds to wait for trace providers
                to acknowledge that they've started tracing.
            buffering_mode: Tells tracing providers how to buffer data
                ONESHOT - When the buffer fills the provider drops subsequent records
                CIRCULAR - When the buffer fills, older records are discarded to make space
                STREAMING - Data is streamed back to the trace_manager. Providers may still drop
                            records if events are produced faster than they can be streamed
            defer_transfer: If true, the trace_manager will delay sending data until tracing has stopped
            compression: If true, compress the trace data.

         Raises:
            ValueError: If `download` is True but either `directory` or
                `trace_file` is not provided.
        """
        if not self.is_session_initialized():
            self.initialize(
                categories=categories,
                buffer_size=buffer_size,
                start_timeout_milliseconds=start_timeout_milliseconds,
                buffering_mode=buffering_mode,
                defer_transfer=defer_transfer,
                compression=compression,
            )
        try:
            await self.start()
            yield
        finally:
            await self.stop()
            if download:
                if directory is None or not os.path.isabs(directory):
                    raise ValueError(
                        "Provide a valid absolute path to download the trace."
                    )
                if trace_file is None:
                    raise ValueError(
                        "Provide a valid trace file to download the trace."
                    )
                await self.terminate_and_download(
                    directory=directory,
                    trace_file=trace_file,
                )
            else:
                await self.terminate()
