# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tracing affordance implementation using Fuchsia-Controller."""

import asyncio
import logging
import os
from datetime import datetime

import fidl_fuchsia_tracing as f_tracing
import fidl_fuchsia_tracing_controller as f_tracingcontroller
import fuchsia_controller_py as fc
from fidl import AsyncSocket
from fuchsia_controller_py import ZxStatus
from honeydew import affordances_capable
from honeydew.affordances.tracing import tracing
from honeydew.affordances.tracing.errors import TracingError, TracingStateError
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing import custom_types
from honeydew.utils import decorators

_FC_PROXIES: dict[str, custom_types.FidlEndpoint] = {
    "TraceProvisioner": custom_types.FidlEndpoint(
        "/core/trace_manager", "fuchsia.tracing.controller.Provisioner"
    ),
    "TracingController": custom_types.FidlEndpoint(
        "/core/trace_manager", "fuchsia.tracing.controller.Session"
    ),
}

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Specified here: https://source.corp.google.com/h/fuchsia/fuchsia/+/main:src/developer/ffx/plugins/trace/data/config.json;l=3
# LINT.IfChange
DEFAULT_CATEGORIES: list[str] = [
    "app",
    "audio",
    "benchmark",
    "blobfs",
    "driver_dispatcher",
    "fxfs",
    "gfx",
    "input",
    "kernel:meta",
    "kernel:sched",
    "magma",
    "memory:kernel",
    "minfs",
    "modular",
    "net",
    "sdmmc",
    "starnix",
    "starnix:binder",
    "starnix:pager",
    "storage",
    "system_metrics",
    "view",
    "flutter",
    "dart",
    "dart:compiler",
    "dart:dart",
    "dart:debugger",
    "dart:embedder",
    "dart:gc",
    "dart:isolate",
    "dart:profiler",
    "dart:vm",
]
# LINT.ThenChange(//src/developer/ffx/plugins/trace/data/config.json)


class TracingUsingFc(tracing.Tracing):
    """Async tracing affordance implementation using Fuchsia-Controller."""

    def __init__(
        self,
        device_name: str,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        self._name: str = device_name
        self._fc_transport: fc_transport.FuchsiaController = fuchsia_controller

        self._drain_task: asyncio.Task[None] | None = None
        self._trace_buffer: bytearray | None = None
        self._trace_socket: AsyncSocket | None = None
        self._trace_controller_proxy: f_tracingcontroller.SessionClient | None = (
            None
        )
        self._session_initialized: bool = False
        self._tracing_active: bool = False

        reboot_affordance.register_for_on_device_boot(fn=self.reboot_handler)

    def verify_supported(self) -> None:
        """Check if Trace is supported on the DUT.
        Raises:
            NotSupportedError: Tracing affordance is not supported by Fuchsia device.
        """
        # TODO(http://b/409625325): Implement the method logic

    async def reboot_handler(self) -> None:
        """A method for handling reboots meant to be passed to the reboot affordance."""
        _LOGGER.info("Received device boot signal. Resetting")
        await self._reset_state()

    async def _reset_state(self) -> None:
        """Resets internal state tracking variables to correspond to an inactive
        state; i.e. tracing uninitialized and not started.
        """
        if self._drain_task:
            _LOGGER.info(
                "Trace session reset before trace downloaded, attempting to cleanup."
            )
            if not self._drain_task.done():
                # We can't await this here because we might be in a sync context
                # via some legacy path, but in TracingUsingFc we are fine.
                await self._drain_task
            self._drain_task = None
        self._trace_buffer = None
        self._trace_socket = None
        self._trace_controller_proxy = None
        self._session_initialized = False
        self._tracing_active = False

    def is_active(self) -> bool:
        """Checks if there is a currently active trace.

        Returns:
            True if the tracing is currently running, False otherwise.
        """
        return self._tracing_active

    def is_session_initialized(self) -> bool:
        """Checks if the session is initialized or not.

        Returns:
            True if the session is initialized, False otherwise.
        """
        return self._session_initialized

    # List all the public methods
    def initialize(
        self,
        categories: list[str] | None = None,
        buffer_size: int | None = None,
        start_timeout_milliseconds: int | None = None,
        buffering_mode: f_tracing.BufferingMode | None = None,
        defer_transfer: bool | None = None,
        compression: bool | None = None,
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
            defer_transfer: If true, the trace_manager will delay sending data until tracing has stopped
            compression: If true, compress the trace data.

        Raises:
            TracingStateError: When trace session is already initialized.
            TracingError: On FIDL communication failure.
        """
        if compression:
            _LOGGER.warning("Compression is not supported in TracingUsingFc")
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
                f"Trace session is already initialized on {self._name}. Can be "
                "initialized only once"
            )
        _LOGGER.info("Initializing trace session on '%s'", self._name)
        _LOGGER.info("Trace categories: '%s'", categories)

        assert self._trace_controller_proxy is None
        trace_provisioner_proxy = f_tracingcontroller.ProvisionerClient(
            self._fc_transport.connect_device_proxy(
                _FC_PROXIES["TraceProvisioner"]
            )
        )
        client, server = self._fc_transport.channel_create()

        (
            trace_socket_server,
            trace_socket_client,
        ) = self._fc_transport.ctx.socket_create()

        try:
            trace_provisioner_proxy.initialize_tracing(
                controller=server.take(),
                config=f_tracingcontroller.TraceConfig(
                    categories=categories,
                    buffer_size_megabytes_hint=buffer_size,
                    start_timeout_milliseconds=start_timeout_milliseconds,
                    buffering_mode=buffering_mode,
                    defer_transfer=defer_transfer,
                ),
                output=trace_socket_server.take(),
            )
        except fc.FcTransportStatus as status:
            raise TracingError(
                "fuchsia.tracing.controller.Initialize FIDL Error"
            ) from status
        self._trace_controller_proxy = f_tracingcontroller.SessionClient(client)
        self._trace_socket = AsyncSocket(trace_socket_client)
        self._session_initialized = True

    async def start(self) -> None:
        """Starts tracing.

        Raises:
           TracingStateError: When trace session is not initialized or
               already started.
           TracingError: On FIDL communication failure.
        """
        if not self._session_initialized:
            raise TracingStateError(
                f"Cannot start: Trace session is not initialized on {self._name}"
            )
        if self._tracing_active:
            raise TracingStateError(
                f"Cannot start: Trace already started on {self._name}"
            )
        _LOGGER.info("Starting trace on '%s'", self._name)

        try:
            assert self._trace_controller_proxy is not None
            await self._trace_controller_proxy.start_tracing(
                buffer_disposition=f_tracing.BufferDisposition.CLEAR_ENTIRE
            )
        except fc.FcTransportStatus as status:
            raise TracingError(
                "fuchsia.tracing.controller.Start FIDL Error"
            ) from status
        self._tracing_active = True
        self._ensure_drain_task()

    @decorators.async_liveness_check
    async def stop(self) -> None:
        """Stops the current trace.

        Raises:
           TracingStateError: When trace session is not initialized or
               not started.
           TracingError: On FIDL communication failure.
        """
        if not self._session_initialized:
            raise TracingStateError(
                "Cannot stop: Trace session is not "
                f"initialized on {self._name}"
            )
        if not self._tracing_active:
            raise TracingStateError(
                f"Cannot stop: Trace not started on {self._name}"
            )
        _LOGGER.info("Stopping trace on '%s'", self._name)
        try:
            assert self._trace_controller_proxy is not None
            res = await self._trace_controller_proxy.stop_tracing(
                write_results=True
            )
            stop_tracing_response = res.unwrap()
            assert (
                stop_tracing_response.provider_stats is not None
            ), f"{stop_tracing_response!r} missing provider_stats"
            for p in stop_tracing_response.provider_stats:
                if p.records_dropped and p.records_dropped > 0:
                    _LOGGER.warning(
                        "%s records were dropped for %s!",
                        p.records_dropped,
                        p.name,
                    )
        except (AssertionError, fc.FcTransportStatus, ZxStatus) as e:
            raise TracingError(
                "fuchsia.tracing.controller.Stop FIDL Error"
            ) from e
        self._tracing_active = False

    def _ensure_drain_task(self) -> None:
        """Helper to make sure there is a background task for draining the socket."""
        if self._drain_task is None:
            self._drain_task = asyncio.create_task(
                self._drain_socket_and_store_buffer()
            )
            _LOGGER.debug("Spawned drain task: %s", self._drain_task)
        else:
            _LOGGER.debug("Skipping creation of drain task. Already running")

    async def _drain_socket_and_store_buffer(self) -> None:
        """Helper to run drain the trace socket and store the result."""
        assert self._trace_socket is not None
        self._trace_buffer = bytearray()
        _LOGGER.info("Reading trace data.")
        self._trace_buffer.extend(await self._trace_socket.read_all())
        _LOGGER.info("Finished reading the socket")

    async def terminate(self) -> None:
        """Terminates the trace session, waiting for it to fully stop."""
        if not self._session_initialized:
            await self._reset_state()
            return

        try:
            if self._trace_controller_proxy:
                self._trace_controller_proxy.close_cleanly()
            if self._drain_task:
                await self._drain_task
        except (RuntimeError, fc.FcTransportStatus, TracingError) as e:
            _LOGGER.warning(
                "Could not cleanly wait for trace termination: %s. "
                "Forcibly resetting state.",
                e,
            )
        finally:
            await self._reset_state()

    async def terminate_and_download(
        self, directory: str, trace_file: str | None = None
    ) -> str:
        """Terminates the trace session and downloads the trace data."""
        if not self._session_initialized:
            raise TracingStateError(
                "Cannot download: Trace session is not "
                f"initialized on {self._name}"
            )

        _LOGGER.info("Closing proxy on '%s'...", self._name)
        if self._trace_controller_proxy:
            self._trace_controller_proxy.close_cleanly()
        _LOGGER.info("Collecting trace on '%s'...", self._name)
        if self._drain_task:
            await self._drain_task
            self._drain_task = None

        if self._trace_buffer is None:
            if self._drain_task is None:
                raise TracingStateError(
                    "Cannot download: Trace was not stopped."
                )
            raise TracingError("Failed to collect trace data from socket.")

        directory = os.path.abspath(directory)
        try:
            os.makedirs(directory)
        except FileExistsError:
            pass

        if not trace_file:
            timestamp: str = datetime.now().strftime("%Y-%m-%d-%I-%M-%S-%p")
            trace_file = f"trace_{self._name}_{timestamp}.fxt"

        trace_file_path: str = os.path.join(directory, trace_file)
        with open(trace_file_path, "wb") as trace_file_handle:
            trace_file_handle.write(self._trace_buffer)
        _LOGGER.info("Trace downloaded at '%s'", trace_file_path)

        await self._reset_state()
        return trace_file_path
