# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import json
import logging
from typing import Any, Protocol

from .dap_types import DapBaseModel
from .models import (
    AttachRequestArguments,
    ContinueArguments,
    ContinueResponse,
    DisconnectArguments,
    EvaluateArguments,
    EvaluateResponse,
    InitializeArguments,
    LaunchArguments,
    MessageType,
    NextArguments,
    PauseArguments,
    Response,
    ScopesArguments,
    ScopesResponse,
    SetBreakpointsArguments,
    SetBreakpointsResponse,
    StackTraceArguments,
    StackTraceResponse,
    StepInArguments,
    StepOutArguments,
    ThreadsResponse,
    VariablesArguments,
    VariablesResponse,
)


class StreamWriterProtocol(Protocol):
    """Protocol for writer objects handling raw byte writes."""

    def write(self, data: bytes) -> None:
        ...

    async def drain(self) -> None:
        ...


READER_STOPPED_EVENT: str = "reader_stopped"
"""Synthetic event type emitted to `event_queue` when the reader task terminates.

This event is pushed to `event_queue` when the background reader task stops (due to EOF on stream,
cancellation, or an unrecoverable stream error).

* **DapClient State**: The `DapClient` instance is in the process of shutting down or already closed.
  `close()` is called immediately after emitting this event, cleaning up background reader/writer tasks
  and clearing pending requests. `is_running` will return `False`.
* **Subsequent Events**: This is the final event placed on `event_queue`. Callers must not expect any further
  events or responses from the client after receiving this.
* **Caller Expectations & Cleanup**: Callers consuming `event_queue` should catch this event and stop reading
  from the queue. The event payload is `{"type": READER_STOPPED_EVENT, "exception": exc}`, where `exception` is
  `None` if the reader exited cleanly or via cancellation, or the exception instance if caused by a failure.
  Callers should perform any necessary caller-side cleanup (e.g. tearing down test fixtures or session state).
  Once the client has finished closing (`is_running` is `False`), `run()` may be called again with a new reader, writer, and event_queue.
"""
logger = logging.getLogger(__name__)


class DapError(Exception):
    """Base exception for DAP client errors."""


class DapClient:
    """A client for the Debug Adapter Protocol."""

    DEFAULT_REQUEST_TIMEOUT: float = 5.0

    def __init__(self) -> None:
        """Initializes the DAP client."""
        self._pending_requests: dict[int, asyncio.Future[Any]] = {}
        self._seq_counter = 1
        self._write_queue: asyncio.Queue[
            tuple[
                dict[str, Any],
                asyncio.Future[None],
                asyncio.Future[Any],
            ]
        ] = asyncio.Queue()
        self._reader_task: asyncio.Task[None] | None = None
        self._writer_task: asyncio.Task[None] | None = None

    @property
    def is_running(self) -> bool:
        """Returns True if both the reader loop and writer tasks are currently running."""
        return (
            self._reader_task is not None
            and not self._reader_task.done()
            and self._writer_task is not None
            and not self._writer_task.done()
        )

    async def close(self) -> None:
        """Closes the client and cancels active writer and reader tasks if running."""
        curr_task = asyncio.current_task()
        tasks_to_cancel: list[asyncio.Task[None]] = []
        if (
            self._writer_task is not None
            and not self._writer_task.done()
            and self._writer_task != curr_task
        ):
            tasks_to_cancel.append(self._writer_task)
        self._writer_task = None

        if (
            self._reader_task is not None
            and not self._reader_task.done()
            and self._reader_task != curr_task
        ):
            tasks_to_cancel.append(self._reader_task)
        self._reader_task = None

        for task in tasks_to_cancel:
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):
                pass

        while True:
            try:
                _, sent_fut, data_fut = self._write_queue.get_nowait()
            except asyncio.QueueEmpty:
                break
            try:
                if not sent_fut.done():
                    sent_fut.cancel()
                if not data_fut.done():
                    data_fut.cancel()
            except Exception:
                pass
            finally:
                try:
                    self._write_queue.task_done()
                except ValueError:
                    pass

        for fut in self._pending_requests.values():
            if not fut.done():
                fut.cancel()
        self._pending_requests.clear()

    # The caller shouldn't directly manage this task. Use `run` and `close` instead.
    async def _run_writer_task(self, writer: StreamWriterProtocol) -> None:
        """Processes queued write requests sequentially in FIFO order.

        Args:
            writer: Output stream writer to transmit messages to debug adapter.
        """
        try:
            while True:
                # Cancellation is raised via client.close().
                request, sent_fut, data_fut = await self._write_queue.get()
                try:
                    # If the caller gave up or cancelled before dequeuing, skip sending.
                    if not data_fut.done():
                        await self._write_message(writer, request)
                        if not sent_fut.done():
                            sent_fut.set_result(None)
                    elif not sent_fut.done():
                        sent_fut.cancel()
                except Exception as e:
                    seq = request.get("seq")
                    if seq is not None:
                        self._pending_requests.pop(seq, None)
                    if not sent_fut.done():
                        sent_fut.set_exception(e)
                    if not data_fut.done():
                        data_fut.set_exception(e)
                    logger.error("DAP write message failed: %s", e)
                finally:
                    self._write_queue.task_done()
        except asyncio.CancelledError:
            pass

    async def _run_reader_task(
        self, reader: asyncio.StreamReader, event_queue: asyncio.Queue[Any]
    ) -> None:
        """Runs the client's read loop, processing messages from the reader.

        Args:
            reader: Input stream reader receiving messages from debug adapter.
            event_queue: Destination queue for incoming DAP events.
        """
        exc: Exception | None = None
        try:
            while True:
                msg = await self._read_message(reader)
                if msg is None:
                    break  # EOF

                msg_type = msg.get("type")
                if msg_type == MessageType.EVENT.value:
                    await event_queue.put(msg)
                elif msg_type == MessageType.RESPONSE.value:
                    req_seq = msg.get("request_seq")
                    if req_seq in self._pending_requests:
                        fut = self._pending_requests.pop(req_seq)
                        if not fut.done():
                            fut.set_result(msg)
        except asyncio.CancelledError:
            pass

        # We cannot recover when _read_message encounters errors such as an invalid Content-Length, as there is no reliable way to parse corrupt stream data.
        except Exception as e:
            logger.exception("Error in DAP client reader task")
            exc = e
            # Set exception on all in-flight pending request futures so callers fail fast with root cause.
            for fut in self._pending_requests.values():
                if not fut.done():
                    fut.set_exception(e)
            self._pending_requests.clear()
        finally:
            # Emit synthetic termination event so queue consumers wake up immediately and fail active event expectations without polling.
            try:
                await event_queue.put(
                    {"type": READER_STOPPED_EVENT, "exception": exc}
                )
            except Exception as put_err:
                logger.warning(
                    "Failed to emit READER_STOPPED_EVENT: %s", put_err
                )
            finally:
                await self.close()

    def run(
        self,
        reader: asyncio.StreamReader,
        writer: StreamWriterProtocol,
        event_queue: asyncio.Queue[Any],
    ) -> None:
        """Starts the client's read and write background tasks.

        Args:
            reader: Stream reader to receive messages from the debug adapter.
            writer: Stream writer to send messages to the debug adapter.
            event_queue: Queue to put received DAP events into.

        Raises:
            DapError: If the client is already running.
        """
        if self.is_running:
            raise DapError("DapClient is already running")

        self._reader_task = asyncio.create_task(
            self._run_reader_task(reader, event_queue)
        )
        self._writer_task = asyncio.create_task(self._run_writer_task(writer))

    # To make requests execute `_write_message` by FIFO order, we rely on the run_writer_task.
    def _send_request_future(
        self,
        command: str,
        arguments: DapBaseModel | None = None,
    ) -> tuple[int, asyncio.Future[None], asyncio.Future[dict[str, Any]]]:
        """Sends a request to the debug adapter synchronously by queueing it for the write worker.

        Args:
            command: The DAP command name.
            arguments: Optional arguments for the command.

        Returns:
            A tuple of (sequence number, sent future, response future).

        Raises:
            DapError: If the client is not running.
            TypeError: If arguments is not a DapBaseModel instance.
        """
        if not self.is_running:
            raise DapError(
                "DapClient is not running. Call 'run()' before sending requests."
            )

        seq = self._seq_counter
        self._seq_counter += 1

        loop = asyncio.get_running_loop()
        sent_fut: asyncio.Future[None] = loop.create_future()
        data_fut: asyncio.Future[dict[str, Any]] = loop.create_future()

        request: dict[str, Any] = {
            "seq": seq,
            "type": MessageType.REQUEST.value,
            "command": command,
        }
        if arguments is not None:
            if not isinstance(arguments, DapBaseModel):
                raise TypeError(
                    f"arguments must be a DapBaseModel, got {type(arguments)}"
                )
            request["arguments"] = arguments.dump_dap()

        self._pending_requests[seq] = data_fut
        self._write_queue.put_nowait((request, sent_fut, data_fut))

        return seq, sent_fut, data_fut

    async def _await_request_response(
        self,
        command: str,
        seq: int,
        sent_fut: asyncio.Future[None],
        data_fut: asyncio.Future[dict[str, Any]],
        timeout: float,
    ) -> dict[str, Any]:
        """Awaits wire transmission and response for a request with centralized cleanup."""
        try:
            async with asyncio.timeout(timeout):
                await sent_fut
                return await data_fut
        except TimeoutError:
            if not sent_fut.done():
                raise DapError(
                    f"Request {command} (seq={seq}) failed to write to socket within {timeout}s"
                )
            raise DapError(
                f"Request {command} (seq={seq}) sent over wire, but adapter timed out waiting for response after {timeout}s"
            )
        finally:
            self._pending_requests.pop(seq, None)
            if not sent_fut.done():
                sent_fut.cancel()
            if not data_fut.done():
                data_fut.cancel()

    async def _send_request(
        self,
        command: str,
        arguments: DapBaseModel | None = None,
        timeout: float = DEFAULT_REQUEST_TIMEOUT,
    ) -> dict[str, Any]:
        """Sends a request to the debug adapter and waits for the response.

        Args:
            command: The DAP command name.
            arguments: Optional arguments for the command.
            timeout: Maximum time to wait for response in seconds.

        Returns:
            The response message dictionary from the adapter.

        Raises:
            DapError: If the request times out or framing fails.
            ValueError: If timeout is not positive.
        """
        if timeout <= 0:
            raise ValueError(f"timeout must be positive, got {timeout}")

        # Instead of directly calling _write_message here,
        # we leverage _send_request_future, so this function inherently sends requests in FIFO order.
        # FIFO may not be necessary for this function and its client, but we keep it for consistency.
        seq, sent_fut, data_fut = self._send_request_future(command, arguments)
        resp = await self._await_request_response(
            command, seq, sent_fut, data_fut, timeout
        )
        if not resp.get("success", True):
            msg = resp.get("message", "Unknown DAP error")
            logger.error(
                "DAP request %s (seq=%s) failed: %s", command, seq, msg
            )
            raise DapError(f"DAP request {command} failed: {msg}")
        return resp

    async def initialize(self, args: InitializeArguments) -> Response:
        """Sends an initialize request.

        Args:
            args: Arguments for the initialize request.

        Returns:
            The response model.
        """
        resp = await self._send_request("initialize", args)
        return Response.model_validate(resp)

    async def disconnect(self, args: DisconnectArguments) -> Response:
        """Sends a disconnect request.

        Args:
            args: Arguments for the disconnect request.

        Returns:
            The response model.
        """
        resp = await self._send_request("disconnect", args)
        return Response.model_validate(resp)

    async def stack_trace(
        self, args: StackTraceArguments
    ) -> StackTraceResponse:
        """Sends a stackTrace request.

        Args:
            args: Arguments for the stackTrace request.

        Returns:
            The stackTrace response model.
        """
        resp = await self._send_request("stackTrace", args)
        return StackTraceResponse.model_validate(resp)

    async def continue_thread(
        self, args: ContinueArguments
    ) -> ContinueResponse:
        """Sends a continue request.

        Args:
            args: Arguments for the continue request.

        Returns:
            The continue response model.
        """
        resp = await self._send_request("continue", args)
        return ContinueResponse.model_validate(resp)

    async def pause_thread(self, args: PauseArguments) -> Response:
        """Sends a pause request.

        Args:
            args: Arguments for the pause request.

        Returns:
            The response model.
        """
        resp = await self._send_request("pause", args)
        return Response.model_validate(resp)

    async def step_out(self, args: StepOutArguments) -> Response:
        """Sends a stepOut request.

        Args:
            args: Arguments for the stepOut request.

        Returns:
            The response model.
        """
        resp = await self._send_request("stepOut", args)
        return Response.model_validate(resp)

    async def next(self, args: NextArguments) -> Response:
        """Sends a next (step over) request.

        Args:
            args: Arguments for the next request.

        Returns:
            The response model.
        """
        resp = await self._send_request("next", args)
        return Response.model_validate(resp)

    async def step_in(self, args: StepInArguments) -> Response:
        """Sends a stepIn request.

        Args:
            args: Arguments for the stepIn request.

        Returns:
            The response model.
        """
        resp = await self._send_request("stepIn", args)
        return Response.model_validate(resp)

    async def threads(self) -> ThreadsResponse:
        """Sends a threads request.

        Returns:
            The threads response model.
        """
        resp = await self._send_request("threads")
        return ThreadsResponse.model_validate(resp)

    async def attach(self, args: AttachRequestArguments) -> Response:
        """Sends an attach request.

        Args:
            args: Arguments for the attach request.

        Returns:
            The response model.
        """
        resp = await self._send_request("attach", args)
        return Response.model_validate(resp)

    async def launch(self, args: LaunchArguments) -> Response:
        """Sends a launch request.

        Args:
            args: Arguments for the launch request.

        Returns:
            The response model.
        """
        resp = await self._send_request("launch", args)
        return Response.model_validate(resp)

    async def evaluate(self, args: EvaluateArguments) -> EvaluateResponse:
        """Sends an evaluate request.

        Args:
            args: Arguments for the evaluate request.

        Returns:
            The response model.
        """
        resp = await self._send_request("evaluate", args)
        return EvaluateResponse.model_validate(resp)

    async def scopes(self, args: ScopesArguments) -> ScopesResponse:
        """Sends a scopes request.

        Args:
            args: Arguments for the scopes request.

        Returns:
            The scopes response model.
        """
        resp = await self._send_request("scopes", args)
        return ScopesResponse.model_validate(resp)

    async def variables(self, args: VariablesArguments) -> VariablesResponse:
        """Sends a variables request.

        Args:
            args: Arguments for the variables request.

        Returns:
            The variables response model.
        """
        resp = await self._send_request("variables", args)
        return VariablesResponse.model_validate(resp)

    async def set_breakpoints(
        self, args: SetBreakpointsArguments
    ) -> SetBreakpointsResponse:
        """Sends a setBreakpoints request.

        Args:
            args: Arguments for the setBreakpoints request.

        Returns:
            The setBreakpoints response model.
        """
        resp = await self._send_request("setBreakpoints", args)
        return SetBreakpointsResponse.model_validate(resp)

    async def _read_message(
        self, reader: asyncio.StreamReader
    ) -> dict[str, Any] | None:
        """Reads a single message from the reader, handling protocol framing.

        Args:
            reader: Stream reader to read from.

        Returns:
            The parsed message dictionary, or None on EOF.

        Raises:
            DapError: If framing headers are invalid or missing.
        """
        content_length = None
        while True:
            line = await reader.readline()
            if not line:
                return None  # EOF
            trimmed = line.decode("utf-8").strip()
            if not trimmed:
                break  # End of headers

            if trimmed.startswith("Content-Length:"):
                parts = trimmed.split(":")
                if len(parts) >= 2:
                    try:
                        content_length = int(parts[1].strip())
                    except ValueError:
                        raise DapError(f"Invalid Content-Length: {trimmed}")

        if content_length is None:
            raise DapError("Missing Content-Length header")

        body = await reader.readexactly(content_length)
        return json.loads(body.decode("utf-8"))

    async def _write_message(
        self, writer: StreamWriterProtocol, value: dict[str, Any]
    ) -> None:
        """Writes a message to the writer, handling protocol framing.

        Args:
            writer: Stream writer to write to.
            value: The message dictionary to serialize and send.
        """
        content = json.dumps(value, separators=(",", ":")).encode("utf-8")
        header = f"Content-Length: {len(content)}\r\n\r\n".encode("utf-8")
        writer.write(header)
        writer.write(content)
        await writer.drain()
