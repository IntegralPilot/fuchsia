# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import asyncio
import copy
import json
import os
import signal
import sys
import tempfile
import unittest
from pathlib import Path
from typing import (
    Any,
    Awaitable,
    Callable,
    Coroutine,
    Dict,
    List,
    Optional,
)

from async_utils.command import (
    AsyncCommand,
    StderrEvent,
    StdoutEvent,
    TerminationEvent,
)
from ffx_cmd.lib import FfxCmd
from portpicker import portpicker
from pydap.client import READER_STOPPED_EVENT
from pydap.dap_types import DapBaseModel
from pydap.models import (
    DisconnectArguments,
    EvaluateArguments,
    InitializeArguments,
    LaunchArguments,
    ScopesArguments,
    SetBreakpointsArguments,
    StackTraceArguments,
    VariablesArguments,
)
from zxdb_dap import ZxdbDapClient, ZxdbStackTraceArguments


class RequestFuture:
    """A future representing a pending DAP request and its expectations."""

    def __init__(
        self, framework: DapTestFramework, command: str, request_seq: int
    ) -> None:
        self.framework = framework
        self.command = command
        self.request_seq = request_seq
        self.fut = asyncio.get_running_loop().create_future()
        self.expectations: List[Dict[str, Any]] = []

    def expect(self, partial_check: Dict[str, Any]) -> RequestFuture:
        """Registers a partial check on the response."""
        self.expectations.append(partial_check)
        return self

    def __await__(self) -> Any:
        return self.fut.__await__()

    def set_result(self, result: Any) -> None:
        if self.fut.done():
            return
        try:
            for check in self.expectations:
                self.framework._verify_partial(result, check, raise_error=True)
            self.fut.set_result(result)
        except Exception as e:
            self.fut.set_exception(e)

    # asyncio.CancelledError inherits directly from BaseException.
    def set_exception(self, exc: BaseException) -> None:
        if not self.fut.done():
            self.fut.set_exception(exc)

    def cancel(self, msg: Optional[str] = None) -> bool:
        return self.fut.cancel(msg)

    def cancelled(self) -> bool:
        return self.fut.cancelled()


class EventFuture:
    """A future representing a pending DAP event and its expectations."""

    def __init__(
        self,
        framework: DapTestFramework,
        event_name: str,
        timeout: Optional[float] = None,
    ) -> None:
        self.framework = framework
        self.event_name = event_name
        self.timeout = timeout
        self.fut = asyncio.get_running_loop().create_future()
        self.checks: List[Dict[str, Any]] = []

    def expect(self, partial_check: Dict[str, Any]) -> EventFuture:
        """Registers a partial check on the event in O(1) time."""
        self.checks.append(partial_check)
        return self

    def __await__(self) -> Any:
        async def _wait() -> Dict[str, Any]:
            # Check if expectations are already satisfied by past history
            self.framework._check_event_expectations_against_history()

            if self.fut.done():
                return self.fut.result()

            try:
                return await asyncio.wait_for(self.fut, timeout=self.timeout)
            except asyncio.TimeoutError:
                raise asyncio.TimeoutError(
                    f"Timed out waiting for DAP event '{self.event_name}' after {self.timeout}s"
                )

        return _wait().__await__()


SentCallback = Callable[
    [asyncio.StreamWriter, Dict[str, Any]], Coroutine[Any, Any, None]
]


def get_build_root() -> Path:
    # //out/default/host_x64/obj/src/developer/debug/e2e_tests/dap_integration_tests/dap_integration_test/dap_integration_test.pyz
    curr = Path(sys.argv[0]).resolve()
    # //out/default
    return curr.parent.parent.parent.parent.parent.parent.parent.parent.parent


def get_dap_source_path(repo_relative_path: str) -> str:
    """Returns an absolute query_path for DAP breakpoint requests to match DWARF source files.

    Args:
        repo_relative_path: Path from the repository root (e.g., "src/developer/.../crasher.c").

    How query_path matches DWARF files in CQ / Swarming CAS without uploading source code:
    1. DAP Requirement: DAP strictly requires absolute paths for breakpoint query_paths.
    2. Build Dir Stripping: Our test framework configures zxdb's `.build_dir` to `get_build_root()`
       via a dynamic symbol-index.json. Setting this index file is required because when zxdb evaluates
       `query_path = get_build_root() / "../../<path>"`, it uses `.build_dir` to strip the prefix
       and convert query_path into relative path `../../<path>`. Without `symbol-index.json`, `.build_dir`
       is empty in CQ CAS, prefix stripping fails, and breakpoint resolution times out.
    3. Mandatory Exact Match: For the breakpoint to resolve, the stripped query_path must == `source file`
       (the exact source string recorded in the DWARF line table, such as `../../src/.../crasher.c`).
       Because zxdb matches this string directly in memory against the DWARF line table, physical
       source files do not need to exist on disk.
    """
    return str((get_build_root() / "../../" / repo_relative_path).resolve())


def get_ffx_bin() -> str:
    build_root = get_build_root()
    DAP_E2E_TESTS_FFX_TEST_DATA = os.environ.get("DAP_E2E_TESTS_FFX_TEST_DATA")
    if DAP_E2E_TESTS_FFX_TEST_DATA is None:
        raise RuntimeError(
            "DAP_E2E_TESTS_FFX_TEST_DATA environment variable not set"
        )
    # The DAP_E2E_TESTS_FFX_TEST_DATA is calculated by rebase_path(ffx_test_host_tools_out_dir, root_build_dir).
    # root_build_dir is //out/default.
    ffx_bin = str((build_root / DAP_E2E_TESTS_FFX_TEST_DATA / "ffx").resolve())

    if not Path(ffx_bin).exists():
        raise RuntimeError(f"ffx binary not found at: {ffx_bin}")

    return ffx_bin


class DapTestFramework:
    """Base class for DAP integration tests."""

    def __init__(self) -> None:
        self.client = ZxdbDapClient()
        self._split_requests: Dict[int, float] = {}
        self._sent_callbacks: Dict[int, SentCallback] = {}
        self._original_write = self.client._write_message
        setattr(self.client, "_write_message", self._patched_write_message)
        self.event_queue: asyncio.Queue[Dict[str, Any]] = asyncio.Queue()
        self.traffic_history: List[Dict[str, Any]] = []
        self.unmatched_events: List[Dict[str, Any]] = []
        self._event_processor_task: Optional[asyncio.Task[None]] = None
        self.request_timeout: float = 5.0
        self._request_tasks: set[asyncio.Task[Any]] = set()
        self.proc: Optional[AsyncCommand] = None
        self._server_log_task: Optional[asyncio.Task[None]] = None
        self._writer: Optional[asyncio.StreamWriter] = None
        self.pending_futures: List[RequestFuture] = []
        self.event_expectations: List[EventFuture] = []
        self.initialized = False
        self.disconnected = False
        self.return_code: Optional[int] = None
        self._isolate_dir: Optional[tempfile.TemporaryDirectory[str]] = None
        self._server_logs: List[str] = []

    async def start_server(self, port: int) -> None:
        """Starts the DAP server via FfxCmd and waits for it to be ready."""
        # Instantiate FfxCmd using create_test_inner() to bypass the default
        # FfxCmd constructor's build directory lookup, which fails in isolated
        # test execution environments like CQ.
        # Ensure FUCHSIA_SSH_KEY is absolute, matching GetFfxEnv() in ffx_debug_agent_bridge.cc
        ssh_key = os.environ.get("FUCHSIA_SSH_KEY")
        if ssh_key:
            os.environ["FUCHSIA_SSH_KEY"] = str(Path(ssh_key).resolve())

        build_root = get_build_root()
        extra_args = []
        self._isolate_dir = tempfile.TemporaryDirectory(
            prefix="ffx_isolate_", dir=os.environ.get("FUCHSIA_TEST_OUTDIR")
        )
        extra_args.extend(["--isolate-dir", self._isolate_dir.name])
        # Replicate GetFfxArgV() config construction
        DAP_E2E_TESTS_FFX_TEST_DATA = os.environ.get(
            "DAP_E2E_TESTS_FFX_TEST_DATA", ""
        )
        ffx_test_data_dir = (build_root / DAP_E2E_TESTS_FFX_TEST_DATA).resolve()

        ffx_config = (
            "log.level=debug,"
            "ffx.isolated=true,"
            "fastboot.usb.disabled=true,"
            "discovery.mdns.enabled=false,"
            f"ffx.subtool-search-paths={ffx_test_data_dir}"
        )

        test_outdir = os.environ.get("FUCHSIA_TEST_OUTDIR")
        if test_outdir:
            ffx_config += f",log.dir={test_outdir}"

        extra_args.extend(["--config", ffx_config])

        device_addr = os.environ.get("FUCHSIA_DEVICE_ADDR", "")
        if device_addr:
            extra_args.append("--target")
            ssh_port = os.environ.get("FUCHSIA_SSH_PORT")
            if ssh_port:
                device_addr = f"{device_addr}:{ssh_port}"
            extra_args.append(device_addr)

        ffx_cmd = FfxCmd(FfxCmd.create_test_inner(get_ffx_bin(), *extra_args))
        symbol_dir = os.environ.get("DAP_E2E_TESTS_SYMBOL_DIR", "")
        zxdb_sym_args = []
        if symbol_dir:
            # Resolve relative to runtime execution CAS build_root.
            resolved_sym_dir = (build_root / symbol_dir).resolve()
            if not resolved_sym_dir.exists() or not any(
                resolved_sym_dir.iterdir()
            ):
                raise RuntimeError(
                    f"Inferior symbol directory missing or empty: {resolved_sym_dir}"
                )
            sym_index_path = Path(self._isolate_dir.name) / "symbol-index.json"
            with open(sym_index_path, "w") as f:
                json.dump(
                    {
                        "build_id_dirs": [
                            {
                                "path": str(resolved_sym_dir),
                                "build_dir": str(build_root),
                            }
                        ]
                    },
                    f,
                )
            zxdb_sym_args.append(f"--symbol-index={sym_index_path}")

        args = [
            "debug",
            "connect",
            "--new-agent",
            "--",
            *zxdb_sym_args,
            "--enable-debug-adapter",
            "--debug-mode",
            f"--debug-adapter-port={port}",
            f"--signal-when-ready={os.getpid()}",
        ]
        self.proc = await ffx_cmd.start(*args)
        self._server_log_task = asyncio.create_task(
            self._run_to_completion_collect_log()
        )
        # Setup signal handler to wait for zxdb to be ready
        loop = asyncio.get_running_loop()
        signal_fut = loop.create_future()

        def handle_sigusr1() -> None:
            if not signal_fut.done():
                signal_fut.set_result(True)

        loop.add_signal_handler(signal.SIGUSR1, handle_sigusr1)

        print("Waiting for SIGUSR1 from zxdb...")
        try:
            await asyncio.wait_for(signal_fut, timeout=30.0)
            print("Received SIGUSR1 from zxdb. Server is ready.")
        except asyncio.TimeoutError:
            print("Timed out waiting for SIGUSR1 from zxdb.")
            if self.proc:
                self.proc.terminate()
            raise RuntimeError("Timed out waiting for DAP server to start")
        finally:
            loop.remove_signal_handler(signal.SIGUSR1)
        print(f"Connecting to DAP server on port {port}...")
        reader, writer = await asyncio.open_connection("localhost", port)
        self._writer = writer
        # Start background tasks for protocol and event handling.
        self.client.run(reader, writer, self.event_queue)
        self._event_processor_task = asyncio.create_task(
            self._event_processor_loop()
        )

    def _send_wrapper(
        self,
        command: str,
        arguments: Optional[DapBaseModel] = None,
    ) -> RequestFuture:
        """Writes a DAP request directly to the socket and returns a RequestFuture.

        Args:
            command: DAP command string.
            arguments: Optional command arguments model.
        """
        seq, sent_fut, data_fut = self.client._send_request_future(
            command, arguments
        )

        req_fut = RequestFuture(self, command, seq)
        self.pending_futures.append(req_fut)

        async def _wait_and_resolve() -> None:
            try:
                resp_dict = await self.client._await_request_response(
                    command, seq, sent_fut, data_fut, self.request_timeout
                )
                self.traffic_history.append(resp_dict)
                if command == "initialize":
                    self.initialized = True
                req_fut.set_result(resp_dict)
            except BaseException as e:
                if not req_fut.fut.done():
                    req_fut.set_exception(e)
                if not isinstance(e, Exception):
                    raise

        task = asyncio.create_task(_wait_and_resolve())
        self._request_tasks.add(task)
        task.add_done_callback(self._request_tasks.discard)
        return req_fut

    def on_event(
        self, event_name: str, timeout: Optional[float] = 120.0
    ) -> EventFuture:
        """Returns an EventFuture to track expectations on events."""
        event_fut = EventFuture(self, event_name, timeout=timeout)
        self.event_expectations.append(event_fut)
        return event_fut

    # This function is only used for testing socket related behaviors now.
    async def _patched_write_message(
        self, writer: asyncio.StreamWriter, value: dict[str, Any]
    ) -> None:
        """Intercepts all outbound DAP messages to execute dynamic callbacks on matched sequence numbers."""
        self.traffic_history.append(value)
        seq = value.get("seq")
        if seq is not None and seq in self._split_requests:
            delay = self._split_requests.pop(seq)
            content = json.dumps(value, separators=(",", ":")).encode("utf-8")
            header = f"Content-Length: {len(content)}\r\n\r\n".encode("utf-8")
            full_payload = header + content
            # Split in half to test arbitrary fragmentation boundaries (e.g. mid-body).
            split_idx = len(full_payload) // 2
            writer.write(full_payload[:split_idx])
            await writer.drain()
            await asyncio.sleep(delay)
            writer.write(full_payload[split_idx:])
            await writer.drain()
        else:
            await self._original_write(writer, value)
        if seq is not None and seq in self._sent_callbacks:
            callback = self._sent_callbacks.pop(seq)
            await callback(writer, value)

    def split_request(self, seq: int, delay: float = 0.1) -> None:
        """Configures the client to split the request with the given sequence number."""
        self._split_requests[seq] = delay

    def set_sent_callback(
        self,
        seq: int,
        callback: SentCallback,
    ) -> None:
        """Registers a callback to be executed when the request with the given seq is sent and drained."""
        self._sent_callbacks[seq] = callback

    async def _event_processor_loop(self) -> None:
        """Continuously reads events from queue and processes them against expectations."""
        while True:
            try:
                event = await self.event_queue.get()
                # Handle synthetic reader termination event sent by DapClient._run_reader_task
                if event.get("type") == READER_STOPPED_EVENT:
                    exc = event.get("exception")
                    if exc is None:
                        exc = RuntimeError(
                            "DAP client reader background task was cancelled"
                        )
                    # Fail all pending event expectations immediately with the underlying exception
                    for exp in list(self.event_expectations):
                        if not exp.fut.done():
                            exp.fut.set_exception(exc)
                    self.event_expectations.clear()
                    break

                self.traffic_history.append(event)

                matched = False
                for exp in list(self.event_expectations):
                    if exp.event_name == event.get("event"):
                        all_passed = True
                        for check in exp.checks:
                            if not self._verify_partial(
                                event, check, raise_error=False
                            ):
                                all_passed = False
                                break
                        if all_passed:
                            if not exp.fut.done():
                                exp.fut.set_result(event)
                            self.event_expectations.remove(exp)
                            matched = True
                            break
                if not matched:
                    self.unmatched_events.append(event)
                self.event_queue.task_done()
            except asyncio.CancelledError:
                break
            except Exception as e:
                print(f"Error in event processor loop: {e}")
                raise

    def _verify_partial(
        self,
        data: Dict[str, Any],
        check: Dict[str, Any],
        raise_error: bool = True,
    ) -> bool:
        """Verifies that data matches partial check recursively."""
        for k, v in check.items():
            if k not in data:
                if raise_error:
                    raise AssertionError(
                        f"Expectation failed: key {k} not found in data"
                    )
                return False
            if isinstance(v, dict) and isinstance(data[k], dict):
                if not self._verify_partial(data[k], v, raise_error):
                    return False
            elif isinstance(v, list) and isinstance(data[k], list):
                if not self._verify_list_partial(data[k], v, raise_error):
                    return False
            elif data[k] != v:
                if raise_error:
                    raise AssertionError(
                        f"Expectation failed: expected {k}={v}, got {data[k]}"
                    )
                return False
        return True

    def _verify_list_partial(
        self,
        data_list: List[Any],
        check_list: List[Any],
        raise_error: bool = True,
    ) -> bool:
        """Verifies that each item in check_list partially matches an item in data_list."""
        # Special case: [...] means we expect a list but don't care about its content
        if check_list == [...]:
            return True

        # For each item we expect, search for a matching item in the actual list.
        for check_item in check_list:
            found = False
            for data_item in data_list:
                # If both are dicts, do a recursive partial match.
                if isinstance(check_item, dict) and isinstance(data_item, dict):
                    if self._verify_partial(
                        data_item, check_item, raise_error=False
                    ):
                        found = True
                        break
                # Otherwise, do an exact match.
                elif data_item == check_item:
                    found = True
                    break
            # If any expected item was not found, the expectation fails.
            if not found:
                if raise_error:
                    raise AssertionError(
                        f"Expectation failed: expected item {check_item} not found in list {data_list}"
                    )
                return False
        return True

    def evaluate_and_compare(
        self, golden_file: str, ignore_list: Optional[List[str]] = None
    ) -> None:
        """Compares history against golden file."""
        history = self._get_clean_history(ignore_list)
        with open(golden_file, "r") as f:
            golden = json.load(f)

        if history != golden:
            raise AssertionError(
                f"Snapshot mismatch. History: {history}, Golden: {golden}"
            )

    def evaluate_and_save(
        self, golden_file: str, ignore_list: Optional[List[str]] = None
    ) -> None:
        """Saves history as golden file."""
        history = self._get_clean_history(ignore_list)
        abs_path = os.path.abspath(golden_file)
        print(f"Saving golden file to: {abs_path}")
        os.makedirs(os.path.dirname(abs_path), exist_ok=True)
        with open(abs_path, "w") as f:
            json.dump(history, f, indent=2)

    def _get_clean_history(
        self, ignore_list: Optional[List[str]] = None
    ) -> List[Dict[str, Any]]:
        """Strips volatile fields from history."""
        clean_history = []
        for msg in self.traffic_history:
            clean_msg = copy.deepcopy(msg)
            if ignore_list:
                for path in ignore_list:
                    self._strip_path(clean_msg, path)
            clean_history.append(clean_msg)
        return clean_history

    def _strip_path(self, data: Any, path: str) -> None:
        """Strips a simple dotted path (e.g., 'body.threadId' or '$.seq') from data."""
        # Normalize path (strip leading $ and .)
        if path.startswith("$"):
            path = path[1:]
        if path.startswith("."):
            path = path[1:]

        parts = path.split(".")
        self._recursive_strip_path(data, parts)

    def _recursive_strip_path(self, data: Any, parts: List[str]) -> None:
        if not parts:
            return

        key = parts[0]
        if len(parts) == 1:
            if isinstance(data, dict):
                data.pop(key, None)
            elif isinstance(data, list):
                for item in data:
                    if isinstance(item, dict):
                        item.pop(key, None)
            return

        # More parts remaining
        if isinstance(data, dict):
            if key in data:
                self._recursive_strip_path(data[key], parts[1:])
        elif isinstance(data, list):
            for item in data:
                if isinstance(item, dict) and key in item:
                    self._recursive_strip_path(item[key], parts[1:])

    def initialize(self, args: InitializeArguments) -> RequestFuture:
        return self._send_wrapper("initialize", args)

    def launch(self, args: LaunchArguments) -> RequestFuture:
        return self._send_wrapper("launch", args)

    def evaluate(self, args: EvaluateArguments) -> RequestFuture:
        return self._send_wrapper("evaluate", args)

    def threads(self) -> RequestFuture:
        return self._send_wrapper("threads")

    def stack_trace(self, args: StackTraceArguments) -> RequestFuture:
        return self._send_wrapper("stackTrace", args)

    def zxdb_stack_trace(self, args: ZxdbStackTraceArguments) -> RequestFuture:
        return self._send_wrapper("stackTrace", args)

    def set_breakpoints(self, args: SetBreakpointsArguments) -> RequestFuture:
        return self._send_wrapper("setBreakpoints", args)

    def scopes(self, args: ScopesArguments) -> RequestFuture:
        return self._send_wrapper("scopes", args)

    def variables(self, args: VariablesArguments) -> RequestFuture:
        return self._send_wrapper("variables", args)

    def disconnect(
        self, args: Optional[DisconnectArguments] = None
    ) -> RequestFuture:
        self.disconnected = True
        if args is None:
            args = DisconnectArguments()
        return self._send_wrapper("disconnect", args)

    async def verify_all_expectations(self) -> None:
        """Awaits all pending futures and verifies event expectations."""
        # 1. Check if pending expectations are already satisfied by unmatched history
        self._check_event_expectations_against_history()

        # 2. Verify request futures (Fail-fast before waiting for events)
        for fut in self.pending_futures:
            try:
                await fut
            except Exception as e:
                print(f"Expectation failure in background request: {e}")
                raise
        self.pending_futures.clear()

        # 3. If there are still pending expectations, wait for up to 2 seconds for them to arrive
        if self.event_expectations:
            print(
                f"Pending event expectations remain. Waiting up to 2 seconds for them to arrive..."
            )
            try:
                async with asyncio.timeout(2.0):
                    futs = [exp.fut for exp in self.event_expectations]
                    await asyncio.gather(*futs)
            except asyncio.TimeoutError:
                print("Timed out waiting for pending event expectations.")

        # 4. Verify event expectations
        if self.event_expectations:
            raise AssertionError(
                f"Pending event expectations were not met: {[e.event_name for e in self.event_expectations]}"
            )

    def dispose_response(self, fut: RequestFuture) -> None:
        """Disposes of a pending request future that we do not expect a response for.

        This differs from simply ignoring a future. If we close a socket or abort a connection,
        the background task for the request will eventually fail (e.g., timeout or socket error)
        and set an exception on the future. If the future is GC'ed without being awaited,
        Python will log a 'Future exception was never retrieved' warning.

        This method removes the future from pending expectations and registers a callback
        to silently retrieve and discard any exception when it completes, preventing warnings.
        """
        if fut in self.pending_futures:
            self.pending_futures.remove(fut)

        def silence_future(f: asyncio.Future[Any]) -> None:
            try:
                if not f.cancelled():
                    f.exception()
            except BaseException:
                pass

        fut.fut.add_done_callback(silence_future)

    def _check_event_expectations_against_history(self) -> None:
        """Helper to check pending event expectations against unmatched history."""
        for exp in list(self.event_expectations):
            for msg in list(self.unmatched_events):
                if (
                    msg.get("type") == "event"
                    and msg.get("event") == exp.event_name
                ):
                    all_passed = True
                    for check in exp.checks:
                        if not self._verify_partial(
                            msg, check, raise_error=False
                        ):
                            all_passed = False
                            break
                    if all_passed:
                        if not exp.fut.done():
                            exp.fut.set_result(msg)
                        self.event_expectations.remove(exp)
                        self.unmatched_events.remove(msg)
                        break

    async def wait_for_shutdown(self, timeout: float = 10.0) -> None:
        """Waits for the server process to exit by waiting for the log reader task.

        This is the safe way to wait for the process to exit because the background
        log reader task (`_run_to_completion_collect_log`) is the sole consumer
        of the process event queue.
        """
        if self._server_log_task:
            await asyncio.wait_for(self._server_log_task, timeout=timeout)

    async def _drain_and_cleanup_server(self) -> None:
        """Drains server logs and ensures the process is terminated."""
        if not self._server_log_task:
            return

        try:
            # 1. Wait for normal exit
            try:
                await asyncio.wait_for(self._server_log_task, timeout=5.0)
                return
            except asyncio.TimeoutError:
                print(
                    "Teardown: Timeout waiting for server log reader to finish, terminating..."
                )

            # 2. Try SIGTERM
            if self.proc:
                try:
                    self.proc.terminate()
                except ProcessLookupError:
                    pass

            # 3. Wait again after SIGTERM
            try:
                await asyncio.wait_for(self._server_log_task, timeout=5.0)
                return
            except asyncio.TimeoutError:
                print(
                    "Teardown: Timeout waiting for server log reader to finish after terminate. Killing..."
                )

            # 4. Try SIGKILL
            if self.proc:
                try:
                    self.proc.kill()
                except ProcessLookupError:
                    pass

            # 5. Final cancellation fallback if still not done
            if not self._server_log_task.done():
                self._server_log_task.cancel()
            try:
                await self._server_log_task
            except asyncio.CancelledError:
                pass
        except Exception as e:
            print(f"Teardown: error draining server process: {e}")

        if self.initialized:
            assert (
                self.return_code == 0
            ), f"DAP server exited unexpectedly with code: {self.return_code}"

    async def teardown(self) -> None:
        """Cleans up connections and processes."""
        for task in list(self._request_tasks):
            if not task.done():
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
        self._request_tasks.clear()

        await self.client.close()
        if self._event_processor_task:
            self._event_processor_task.cancel()
            try:
                await self._event_processor_task
            except asyncio.CancelledError:
                pass

        if self._writer:
            self._writer.close()
            await self._writer.wait_closed()

        # Drain logs and terminate process
        await self._drain_and_cleanup_server()

        if self._isolate_dir:
            self._isolate_dir.cleanup()
            self._isolate_dir = None

    async def _run_to_completion_collect_log(self) -> None:
        """Runs the process to completion while collecting stdout/stderr logs.

        CRITICAL: This is the sole consumer of the `AsyncCommand` event queue
        (`self.proc`). Since `AsyncCommand` only supports a single iterator,
        no other methods (such as `self.proc.run_to_completion()`) should be
        called concurrently on `self.proc` while this task is running. Doing
        so will cause a race condition where events are missed by one of the
        consumers, leading to hangs.
        """
        if self.proc is None:
            raise RuntimeError("Cannot read server log: process is not started")
        try:
            async for event in self.proc:
                if isinstance(event, StdoutEvent):
                    self._server_logs.append(
                        f"[zxdb stdout] {event.text.decode('utf-8', errors='replace')}"
                    )
                elif isinstance(event, StderrEvent):
                    self._server_logs.append(
                        f"[zxdb stderr] {event.text.decode('utf-8', errors='replace')}"
                    )
                elif isinstance(event, TerminationEvent):
                    self.return_code = event.return_code
                    self._server_logs.append(
                        f"[zxdb terminated] exit code: {event.return_code}\n"
                    )
        except asyncio.CancelledError:
            pass
        except Exception as e:
            self._server_logs.append(f"Error reading server log: {e}\n")
            raise

    def dump_server_logs(self, test_id: Optional[str] = None) -> None:
        """Dumps captured DAP server stdout/stderr logs to host output."""
        if not self._server_logs:
            return
        if test_id:
            print(
                f"\n--- Captured DAP Server Logs for {test_id} ---", flush=True
            )
        else:
            print("\n--- Captured DAP Server Logs ---", flush=True)
        for line in self._server_logs:
            print(line, end="", flush=True)
        print("---------------------------------\n", flush=True)

    def dump_traffic_history(self, test_id: Optional[str] = None) -> None:
        """Dumps all recorded DAP traffic (requests, responses, events) to host output."""
        if not self.traffic_history:
            return
        if test_id:
            print(
                f"\n--- Captured DAP Traffic History for {test_id} ---",
                flush=True,
            )
        else:
            print("\n--- Captured DAP Traffic History ---", flush=True)
        for item in self.traffic_history:
            direction = "Send" if item.get("type") == "request" else "Recv"
            print(f"[{direction}] {json.dumps(item, indent=2)}", flush=True)
        print("-------------------------------------------\n", flush=True)

    async def flush_and_dump_traffic_history(
        self, test_id: Optional[str] = None, drain_timeout: float = 1.0
    ) -> None:
        """Briefly awaits pending background request coroutines before dumping traffic history."""
        pending_tasks = [t for t in self._request_tasks if not t.done()]
        if pending_tasks:
            await asyncio.wait(pending_tasks, timeout=drain_timeout)
        await asyncio.sleep(0.1)
        self.dump_traffic_history(test_id)


class DapTestCase(unittest.IsolatedAsyncioTestCase):
    """Base class for DAP integration tests, handling server lifecycle fixtures."""

    auto_initialize: bool = True
    require_build_type: Optional[List[str]] = None

    def _get_build_type_map(self) -> Dict[str, str]:
        """Returns build settings in key,value pair.
        raw_build_type example:
        optimize=size:target_cpu=arm64:is_lto=false:is_thinlto=false:is_asan=false:is_coverage=false
        """
        raw_build_type = os.environ.get("DAP_E2E_TESTS_BUILD_TYPE", "")

        result: Dict[str, str] = {}
        for item in raw_build_type.split(":"):
            if "=" in item:
                k, v = item.split("=", 1)
                result[k.strip()] = v.strip()
        return result

    def require(self, requirement: str) -> None:
        """Skips the test if the current DAP_E2E_TESTS_BUILD_TYPE key does not satisfy requirement (= or !=)."""
        build_type_map = self._get_build_type_map()
        if "!=" in requirement:
            key, unexpected_val = requirement.split("!=", 1)
            key = key.strip()
            unexpected_val = unexpected_val.strip()
            actual_val = build_type_map.get(key)
            if actual_val == unexpected_val:
                self.skipTest(
                    f"Skipped because of unmet requirement '{requirement}' (current {key}='{actual_val}')"
                )
        elif "=" in requirement:
            key, expected_val = requirement.split("=", 1)
            key = key.strip()
            expected_val = expected_val.strip()
            actual_val = build_type_map.get(key)
            if actual_val != expected_val:
                self.skipTest(
                    f"Skipped because of unmet requirement '{requirement}' (current {key}='{actual_val}')"
                )
        else:
            raise ValueError(
                f"Requirement must be in 'key=value' or 'key!=value' format, got '{requirement}'"
            )

    async def asyncSetUp(self) -> None:
        if self.require_build_type:
            for req in self.require_build_type:
                self.require(req)

        print(f"\n[TEST START] {self.id()}", flush=True)
        self.framework = DapTestFramework()
        self.port = portpicker.pick_unused_port()

        # Start server and connect
        try:
            await self.framework.start_server(self.port)
            if getattr(self, "auto_initialize", True):
                await self.initialize(InitializeArguments(adapterID="zxdb"))
                await self.on_event("initialized", 10.0)
        except Exception:
            await self.framework._drain_and_cleanup_server()
            self.framework.dump_server_logs(self.id())
            print(f"[TEST END By Setup] {self.id()}\n", flush=True)
            raise

    # any exception that is not caught during the execution of unittest will update the outcome
    def _is_test_failed(self) -> bool:
        if getattr(self, "_teardown_failed", False):
            return True
        outcome = getattr(self, "_outcome", None)
        if outcome:
            if hasattr(outcome, "result"):
                # Python 3.11+
                result = outcome.result
                return any(
                    test == self
                    for test, text in result.errors + result.failures
                )
        return False

    async def _run_keep_previous_fail(
        self, task_name: str, coro: Awaitable[Any]
    ) -> None:
        is_failed = self._is_test_failed()
        try:
            await coro
        except Exception as e:
            self._teardown_failed = True
            if is_failed:
                print(
                    f"[{task_name}] Check failed (ignored due to prior failure): {e}",
                    flush=True,
                )
            else:
                print(f"[{task_name}] Check failed: {e}", flush=True)
                raise

    async def asyncTearDown(self) -> None:
        try:
            # some failure might have already happened before entering this function
            # use _run_keep_previous_fail to prevent failure from being overwritten
            await self._run_keep_previous_fail(
                "VERIFY_EXPECT", self.framework.verify_all_expectations()
            )

            # safely close the connection if it was initialized and not already disconnected.
            # the framework.teardown will terminate forcefully if server is still alive
            if self.framework.initialized and not self.framework.disconnected:
                await self._run_keep_previous_fail(
                    "VERIFY_DISCONNECT", self.disconnect()
                )
        finally:
            # Whenever failure occurs mid-test, or traffic dump requested, drain telemetry.
            is_failed = self._is_test_failed()
            if is_failed or os.environ.get("DAP_DUMP_LOG_ALWAYS") == "1":
                await self.framework.flush_and_dump_traffic_history(self.id())
                self.framework.dump_server_logs(self.id())

            await self._run_keep_previous_fail(
                "CLEAN_FRAMEWORK", self.framework.teardown()
            )
            print(f"[TEST END] {self.id()}\n", flush=True)

    # Delegation methods for cleaner test syntax
    def initialize(self, args: InitializeArguments) -> RequestFuture:
        return self.framework.initialize(args)

    def launch(self, args: LaunchArguments) -> RequestFuture:
        return self.framework.launch(args)

    def evaluate(self, args: EvaluateArguments) -> RequestFuture:
        return self.framework.evaluate(args)

    def threads(self) -> RequestFuture:
        return self.framework.threads()

    def stack_trace(self, args: StackTraceArguments) -> RequestFuture:
        return self.framework.stack_trace(args)

    def zxdb_stack_trace(self, args: ZxdbStackTraceArguments) -> RequestFuture:
        return self.framework.zxdb_stack_trace(args)

    def set_breakpoints(self, args: SetBreakpointsArguments) -> RequestFuture:
        return self.framework.set_breakpoints(args)

    def scopes(self, args: ScopesArguments) -> RequestFuture:
        return self.framework.scopes(args)

    def variables(self, args: VariablesArguments) -> RequestFuture:
        return self.framework.variables(args)

    def split_request(self, seq: int, delay: float = 0.1) -> None:
        self.framework.split_request(seq, delay)

    def disconnect(
        self, args: Optional[DisconnectArguments] = None
    ) -> RequestFuture:
        return self.framework.disconnect(args)

    def dispose_response(self, fut: RequestFuture) -> None:
        self.framework.dispose_response(fut)

    def set_sent_callback(
        self,
        seq: int,
        callback: SentCallback,
    ) -> None:
        self.framework.set_sent_callback(seq, callback)

    def on_event(
        self, event_name: str, timeout: Optional[float] = 120.0
    ) -> EventFuture:
        return self.framework.on_event(event_name, timeout=timeout)

    def evaluate_and_compare(
        self, golden_file: str, ignore_list: Optional[List[str]] = None
    ) -> None:
        self.framework.evaluate_and_compare(golden_file, ignore_list)

    def evaluate_and_save(
        self, golden_file: str, ignore_list: Optional[List[str]] = None
    ) -> None:
        self.framework.evaluate_and_save(golden_file, ignore_list)

    async def verify_all_expectations(self) -> None:
        await self.framework.verify_all_expectations()
