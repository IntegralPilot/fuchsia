# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import contextlib
import json
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from io import StringIO
from typing import Any

from cli.cli import main
from daemon.constants import UDS_PATH
from portpicker import portpicker
from shared.protocol import PROTOCOL_VERSION, serialize
from shared.protocol.hello import HelloRequest
from shared.protocol.start import StartRequest

DAEMON_CLEANUP_TIMEOUT = 5.0


async def _cleanup_process_group(proc: subprocess.Popen[Any] | None) -> None:
    """Kills the process group of the given process."""
    if proc is None:
        return

    if getattr(proc, "stdout_file", None) and not proc.stdout_file.closed:  # type: ignore
        proc.stdout_file.close()  # type: ignore
    if getattr(proc, "stderr_file", None) and not proc.stderr_file.closed:  # type: ignore
        proc.stderr_file.close()  # type: ignore

    if proc.poll() is None:
        try:
            pgid = os.getpgid(proc.pid)
            os.killpg(pgid, signal.SIGTERM)
            try:
                await asyncio.wait_for(
                    asyncio.to_thread(proc.wait), timeout=DAEMON_CLEANUP_TIMEOUT
                )
            except asyncio.TimeoutError:
                os.killpg(pgid, signal.SIGKILL)
                await asyncio.to_thread(proc.wait)
        except ProcessLookupError:
            pass


class TestCLIIntegration(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        if UDS_PATH.exists():
            UDS_PATH.unlink()
        self.fake_dap_server: asyncio.AbstractServer | None = None
        self.received_dap_requests: list[dict[str, Any]] = []

    async def asyncTearDown(self) -> None:
        if self.fake_dap_server:
            self.fake_dap_server.close()
            await self.fake_dap_server.wait_closed()
        if UDS_PATH.exists():
            UDS_PATH.unlink()

    async def start_fake_dap_server(self, port: int) -> asyncio.AbstractServer:
        async def handle_client(
            reader: asyncio.StreamReader, writer: asyncio.StreamWriter
        ) -> None:
            try:
                while True:
                    header = await reader.readuntil(b"\r\n\r\n")
                    content_length = 0
                    for line in header.decode("utf-8").split("\r\n"):
                        if line.startswith("Content-Length:"):
                            content_length = int(line.split(":")[1].strip())
                    body = await reader.readexactly(content_length)
                    req = json.loads(body.decode("utf-8"))
                    self.received_dap_requests.append(req)

                    if req.get("command") == "initialize":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "initialize",
                            "body": {},
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()

                        # Send initialized event
                        event = {
                            "seq": req["seq"] + 1,
                            "type": "event",
                            "event": "initialized",
                        }
                        event_body = json.dumps(event).encode("utf-8")
                        event_header = (
                            f"Content-Length: {len(event_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(event_header + event_body)
                        await writer.drain()
                    elif req.get("command") == "pause":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "pause",
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()

                        # Simulate stopped event
                        event = {
                            "seq": req["seq"] + 1,
                            "type": "event",
                            "event": "stopped",
                            "body": {
                                "reason": "pause",
                                "threadId": req["arguments"]["threadId"],
                            },
                        }
                        event_body = json.dumps(event).encode("utf-8")
                        event_header = (
                            f"Content-Length: {len(event_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(event_header + event_body)
                        await writer.drain()
                    elif req.get("command") == "continue":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "continue",
                            "body": {"allThreadsContinued": True},
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()
                    elif req.get("command") == "stackTrace":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "stackTrace",
                            "body": {
                                "stackFrames": [
                                    {
                                        "id": 1,
                                        "name": "main",
                                        "source": {
                                            "name": "main.cc",
                                            "path": "/path/to/main.cc",
                                        },
                                        "line": 10,
                                        "column": 1,
                                    }
                                ],
                                "totalFrames": 1,
                            },
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()
                    elif req.get("command") == "attach":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "attach",
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()

                        # Simulate process event
                        event = {
                            "seq": req["seq"] + 1,
                            "type": "event",
                            "event": "process",
                            "body": {
                                "name": "test_process",
                                "systemProcessId": 1234,
                                "isLocalProcess": False,
                                "startMethod": "attach",
                            },
                        }
                        event_body = json.dumps(event).encode("utf-8")
                        event_header = (
                            f"Content-Length: {len(event_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(event_header + event_body)
                        await writer.drain()
                    elif req.get("command") == "zxdb.Detach":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "zxdb.Detach",
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()
                    elif req.get("command") == "threads":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "threads",
                            "body": {"threads": []},
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()
                    elif req.get("command") == "evaluate":
                        resp = {
                            "seq": req["seq"],
                            "type": "response",
                            "request_seq": req["seq"],
                            "success": True,
                            "command": "evaluate",
                            "body": {
                                "result": "42",
                                "type": "int",
                                "variablesReference": 0,
                            },
                        }
                        resp_body = json.dumps(resp).encode("utf-8")
                        resp_header = (
                            f"Content-Length: {len(resp_body)}\r\n\r\n".encode(
                                "utf-8"
                            )
                        )
                        writer.write(resp_header + resp_body)
                        await writer.drain()
            except (asyncio.IncompleteReadError, ConnectionResetError):
                pass
            finally:
                writer.close()
                try:
                    await writer.wait_closed()
                except:
                    pass

        server = await asyncio.start_server(handle_client, "127.0.0.1", port)
        return server

    async def _setup_daemon_and_server(
        self,
    ) -> tuple[subprocess.Popen[Any], int]:
        port = portpicker.pick_unused_port()
        proc: subprocess.Popen[Any] | None = None
        read_fd: int | None = None
        write_fd: int | None = None
        stdout_file: Any = None
        stderr_file: Any = None

        # Start fake DAP server
        self.fake_dap_server = await self.start_fake_dap_server(port)

        try:
            read_fd, write_fd = os.pipe()
            os.set_inheritable(write_fd, True)

            # Start Daemon manually
            # Tests are always run as a .pyz file, so sys.path[0] is the path to the .pyz archive.
            current_dir = os.path.dirname(sys.path[0])
            daemon_path = os.path.join(current_dir, "zxdb-daemon")
            args = [
                daemon_path,
                "--port",
                str(port),
                f"--ready-fd={write_fd}",
            ]

            # zxdb-daemon expects to be executed from the context of fuchsia-vendored-python, which is
            # also how this test is always invoked. So it's safe for us to use our own interpreter to
            # invoke the zxdb-daemon as well.
            cmd = [sys.executable] + args
            stdout_file = tempfile.TemporaryFile()
            stderr_file = tempfile.TemporaryFile()

            proc = subprocess.Popen(
                cmd,
                stdout=stdout_file,
                stderr=stderr_file,
                start_new_session=True,
                pass_fds=[write_fd],
            )
            setattr(proc, "stdout_file", stdout_file)
            setattr(proc, "stderr_file", stderr_file)

            # Close write end in parent
            os.close(write_fd)
            write_fd = None

            # Wait for signal on the pipe
            loop = asyncio.get_running_loop()
            future = loop.create_future()

            def on_read() -> None:
                assert read_fd is not None
                try:
                    data = os.read(read_fd, 1)
                    if not future.done():
                        future.set_result(data)
                except BlockingIOError:
                    pass
                except Exception as e:
                    if not future.done():
                        future.set_exception(e)

            os.set_blocking(read_fd, False)
            loop.add_reader(read_fd, on_read)
            try:
                await asyncio.wait_for(future, timeout=10.0)
            finally:
                loop.remove_reader(read_fd)
            os.close(read_fd)
            read_fd = None

            # Perform handshake to ensure daemon is connected to DAP server
            reader, writer = await asyncio.open_unix_connection(UDS_PATH)
            req = HelloRequest(version=PROTOCOL_VERSION)
            writer.write(serialize(req).encode("utf-8"))
            await writer.drain()
            response = await asyncio.wait_for(reader.readline(), timeout=5.0)
            writer.close()
            await writer.wait_closed()

            resp_dict = json.loads(response.decode("utf-8"))
            self.assertTrue(resp_dict.get("success"))

            # Send StartRequest to initialize DAP session
            reader, writer = await asyncio.open_unix_connection(UDS_PATH)
            start_req = StartRequest(port=port, connect=True)
            writer.write(serialize(start_req).encode("utf-8"))
            await writer.drain()
            response = await asyncio.wait_for(reader.readline(), timeout=5.0)
            writer.close()
            await writer.wait_closed()

            resp_dict = json.loads(response.decode("utf-8"))
            self.assertTrue(resp_dict.get("success"))

            return proc, port
        except Exception:
            stderr_output = ""
            if (
                proc
                and getattr(proc, "stderr_file", None)
                and not getattr(proc, "stderr_file").closed
            ):
                getattr(proc, "stderr_file").seek(0)
                stderr_output = (
                    getattr(proc, "stderr_file")
                    .read()
                    .decode("utf-8", errors="replace")
                )
            elif stderr_file and not stderr_file.closed:
                stderr_file.seek(0)
                stderr_output = stderr_file.read().decode(
                    "utf-8", errors="replace"
                )

            await _cleanup_process_group(proc)
            if read_fd is not None:
                try:
                    os.close(read_fd)
                except OSError:
                    pass
            if write_fd is not None:
                try:
                    os.close(write_fd)
                except OSError:
                    pass
            if stdout_file and not stdout_file.closed:
                try:
                    stdout_file.close()
                except OSError:
                    pass
            if stderr_file and not stderr_file.closed:
                try:
                    stderr_file.close()
                except OSError:
                    pass

            if stderr_output:
                print(
                    f"[DAEMON CRASH STDERR]\n{stderr_output}", file=sys.stderr
                )
            raise

    async def test_daemon_lifecycle(self) -> None:
        """Tests that the daemon starts and stops correctly."""
        proc, port = await self._setup_daemon_and_server()

        try:
            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

            # Wait for process to exit
            try:
                await asyncio.wait_for(
                    asyncio.to_thread(proc.wait), timeout=5.0
                )
            except asyncio.TimeoutError:
                self.fail("Daemon process did not exit after stop")

            # Verify socket is deleted
            self.assertFalse(UDS_PATH.exists(), "Socket file was not deleted")

        finally:
            await _cleanup_process_group(proc)

    async def test_daemon_idempotent_start(self) -> None:
        """Tests that sending StartRequest twice is handled cleanly."""
        # _setup_daemon_and_server() already sends an initial StartRequest.
        proc, port = await self._setup_daemon_and_server()

        try:
            # Send StartRequest a second time
            reader, writer = await asyncio.open_unix_connection(UDS_PATH)
            start_req = StartRequest(port=port, connect=True)
            writer.write(serialize(start_req).encode("utf-8"))
            await writer.drain()
            response = await asyncio.wait_for(reader.readline(), timeout=5.0)
            writer.close()
            await writer.wait_closed()

            resp_dict = json.loads(response.decode("utf-8"))
            self.assertTrue(resp_dict.get("success"))
            self.assertEqual(resp_dict.get("message"), "Daemon already started")

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_daemon_start_different_port(self) -> None:
        """Tests that sending StartRequest with a different port fails if already running."""
        # _setup_daemon_and_server() already sends an initial StartRequest.
        proc, port = await self._setup_daemon_and_server()

        try:
            reader, writer = await asyncio.open_unix_connection(UDS_PATH)
            # Use a different port
            different_port = port + 1
            start_req = StartRequest(port=different_port, connect=True)
            writer.write(serialize(start_req).encode("utf-8"))
            await writer.drain()
            response = await asyncio.wait_for(reader.readline(), timeout=5.0)
            writer.close()
            await writer.wait_closed()

            resp_dict = json.loads(response.decode("utf-8"))
            self.assertFalse(resp_dict.get("success"))
            self.assertIn("cannot switch to", resp_dict.get("message", ""))

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_daemon_start_different_connect(self) -> None:
        """Tests that sending StartRequest with a different connect value fails if already running."""
        # _setup_daemon_and_server() already sends an initial StartRequest.
        proc, port = await self._setup_daemon_and_server()

        try:
            reader, writer = await asyncio.open_unix_connection(UDS_PATH)
            # Use a different connect value (already running with connect=True, so use False)
            start_req = StartRequest(port=port, connect=False)
            writer.write(serialize(start_req).encode("utf-8"))
            await writer.drain()
            response = await asyncio.wait_for(reader.readline(), timeout=5.0)
            writer.close()
            await writer.wait_closed()

            resp_dict = json.loads(response.decode("utf-8"))
            self.assertFalse(resp_dict.get("success"))
            self.assertIn("cannot switch to", resp_dict.get("message", ""))

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_daemon_hello(self) -> None:
        """Tests the versioned hello handshake."""

        async def run_test() -> None:
            proc, port = await self._setup_daemon_and_server()

            try:
                # Test valid version
                reader, writer = await asyncio.open_unix_connection(UDS_PATH)
                req = HelloRequest(version=PROTOCOL_VERSION)
                writer.write(serialize(req).encode("utf-8"))
                await writer.drain()

                try:
                    # TODO(https://fxbug.dev/509581944): this is intentionally kept longer than the
                    # daemon's internal "handle_hello" timeout, which doesn't feel very good. This
                    # should be improved along with the rest of the general timeout error handling.
                    response = await asyncio.wait_for(
                        reader.readline(), timeout=15.0
                    )
                except asyncio.TimeoutError:
                    self.fail("Timed out waiting for response from daemon")
                writer.close()
                await writer.wait_closed()

                resp_dict = json.loads(response.decode("utf-8"))
                self.assertTrue(resp_dict.get("success"))
                self.assertEqual(
                    resp_dict.get("body", {}).get("protocol_version"),
                    PROTOCOL_VERSION,
                )

                # Test invalid version
                reader, writer = await asyncio.open_unix_connection(UDS_PATH)
                req = HelloRequest(version=PROTOCOL_VERSION + 1)
                writer.write(serialize(req).encode("utf-8"))
                await writer.drain()

                try:
                    response = await asyncio.wait_for(
                        reader.readline(), timeout=5.0
                    )
                except asyncio.TimeoutError:
                    self.fail("Timed out waiting for response from daemon")
                writer.close()
                await writer.wait_closed()

                resp_dict = json.loads(response.decode("utf-8"))
                self.assertFalse(resp_dict.get("success"))
                self.assertIn("version mismatch", resp_dict.get("message", ""))

            finally:
                await _cleanup_process_group(proc)
                if self.fake_dap_server:
                    self.fake_dap_server.close()
                    await self.fake_dap_server.wait_closed()
                    self.fake_dap_server = None

        # Run the test twice to ensure that the daemon can be started and stopped
        # repeatedly without leaving stale state or leaking resources.
        try:
            await asyncio.wait_for(run_test(), timeout=30.0)
        except asyncio.TimeoutError:
            self.fail("Test timed out")

        try:
            await asyncio.wait_for(run_test(), timeout=30.0)
        except asyncio.TimeoutError:
            self.fail("Test timed out")

    async def test_pause_continue(self) -> None:
        proc, port = await self._setup_daemon_and_server()

        try:
            # Test Pause
            exit_code = await main(["pause", "-t", "1"])
            self.assertEqual(exit_code, 0)

            # Test Continue
            exit_code = await main(["continue", "1"])
            self.assertEqual(exit_code, 0)

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_stack_trace(self) -> None:
        proc, port = await self._setup_daemon_and_server()

        try:
            f = StringIO()
            with contextlib.redirect_stdout(f):
                exit_code = await main(["stack-trace", "-t", "1"])
            self.assertEqual(exit_code, 0)

            output = f.getvalue()
            output_json = json.loads(output)
            self.assertTrue(output_json.get("success"))

            # Verify stack frame
            body = output_json.get("body")
            self.assertIsNotNone(body)
            frames = body.get("stack_frames")
            self.assertEqual(len(frames), 1)
            self.assertEqual(frames[0]["frame_index"], 0)
            self.assertEqual(frames[0]["name"], "main")
            self.assertEqual(frames[0]["source"]["name"], "main.cc")

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_wait_for_event(self) -> None:
        proc, _port = await self._setup_daemon_and_server()

        try:
            # Trigger an event by sending pause command
            exit_code = await main(["pause", "-t", "1"])
            self.assertEqual(exit_code, 0)

            # Capture stdout to read the JSON response from wait-for-event
            out = StringIO()
            with contextlib.redirect_stdout(out):
                exit_code = await main(
                    ["wait-for-event", "--last-seen-seq", "0"]
                )
            self.assertEqual(exit_code, 0)
            output = out.getvalue().strip()

            # Parse output
            resp = json.loads(output)
            self.assertTrue(resp.get("success"))
            events = resp.get("events", [])
            self.assertGreater(len(events), 0)
            self.assertEqual(events[0]["event"], "stopped")

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_process_event(self) -> None:
        proc, _port = await self._setup_daemon_and_server()

        try:
            # Trigger a process event by sending attach command
            exit_code = await main(["attach", "test_process"])
            self.assertEqual(exit_code, 0)

            # Capture stdout to read the JSON response from wait-for-event
            out = StringIO()
            with contextlib.redirect_stdout(out):
                exit_code = await main(
                    ["wait-for-event", "--last-seen-seq", "0"]
                )
            self.assertEqual(exit_code, 0)
            output = out.getvalue().strip()

            # Parse output
            resp = json.loads(output)
            self.assertTrue(resp.get("success"))
            events = resp.get("events", [])
            self.assertGreater(len(events), 0)

            # Find process event
            process_event = None
            for event in events:
                if event.get("event") == "process":
                    process_event = event
                    break

            self.assertIsNotNone(
                process_event, "Process event was not received"
            )
            assert process_event is not None
            self.assertEqual(process_event["body"]["systemProcessId"], 1234)
            self.assertEqual(process_event["body"]["name"], "test_process")

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_detach_command(self) -> None:
        proc, _port = await self._setup_daemon_and_server()

        try:
            # Attach to trigger process event
            exit_code = await main(["attach", "test_process"])
            self.assertEqual(exit_code, 0)

            # Wait for the process event (seq 1) instead of sleeping
            out = StringIO()
            with contextlib.redirect_stdout(out):
                exit_code = await main(
                    ["wait-for-event", "--last-seen-seq", "0", "--timeout", "5"]
                )
            self.assertEqual(exit_code, 0)
            output = out.getvalue().strip()

            resp = json.loads(output)
            self.assertTrue(resp.get("success"))
            events = resp.get("events", [])
            self.assertGreater(len(events), 0)
            self.assertEqual(events[0]["event"], "process")
            self.assertEqual(events[0]["body"]["systemProcessId"], 1234)

            # Detach via CLI
            exit_code = await main(["detach", "1234"])
            self.assertEqual(exit_code, 0)

            # Wait for synthesized detached event (seq 2) instead of sleeping
            out = StringIO()
            with contextlib.redirect_stdout(out):
                exit_code = await main(
                    ["wait-for-event", "--last-seen-seq", "1", "--timeout", "5"]
                )
            self.assertEqual(exit_code, 0)
            output = out.getvalue().strip()

            resp = json.loads(output)
            self.assertTrue(resp.get("success"))
            events = resp.get("events", [])
            self.assertGreater(len(events), 0)
            self.assertEqual(events[0]["event"], "detached")
            self.assertEqual(events[0]["body"]["pid"], 1234)

            # Verify zxdb.Detach was received by fake server (should be immediate now)
            detach_req = next(
                (
                    r
                    for r in self.received_dap_requests
                    if r.get("command") == "zxdb.Detach"
                ),
                None,
            )
            self.assertIsNotNone(
                detach_req,
                "zxdb.Detach command was not received by fake server",
            )
            # This assertion is redundant with the testing assertion made directly above, but is
            # actually necessary for mypy to be able to deduce that |detach_req| is definitely a
            # DetachRequest type and not None.
            assert detach_req is not None
            self.assertEqual(detach_req.get("arguments", {}).get("pid"), 1234)

            # Verify active processes cleaned up in state
            f = StringIO()
            with contextlib.redirect_stdout(f):
                exit_code = await main(["get-state"])
            self.assertEqual(exit_code, 0)
            state_resp = json.loads(f.getvalue())
            self.assertTrue(state_resp.get("success"))
            self.assertEqual(state_resp.get("body", {}).get("processes"), {})

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_detach_all_command(self) -> None:
        proc, _port = await self._setup_daemon_and_server()

        try:
            # Attach to trigger process event
            exit_code = await main(["attach", "test_process"])
            self.assertEqual(exit_code, 0)

            # Wait for the process event (seq 1)
            out = StringIO()
            with contextlib.redirect_stdout(out):
                exit_code = await main(
                    ["wait-for-event", "--last-seen-seq", "0", "--timeout", "5"]
                )
            self.assertEqual(exit_code, 0)
            output = out.getvalue().strip()

            resp = json.loads(output)
            self.assertTrue(resp.get("success"))
            events = resp.get("events", [])
            self.assertGreater(len(events), 0)
            self.assertEqual(events[0]["event"], "process")
            self.assertEqual(events[0]["body"]["systemProcessId"], 1234)

            # Detach all via CLI
            exit_code = await main(["detach", "--all"])
            self.assertEqual(exit_code, 0)

            # Wait for synthesized detached event (seq 2)
            out = StringIO()
            with contextlib.redirect_stdout(out):
                exit_code = await main(
                    ["wait-for-event", "--last-seen-seq", "1", "--timeout", "5"]
                )
            self.assertEqual(exit_code, 0)
            output = out.getvalue().strip()

            resp = json.loads(output)
            self.assertTrue(resp.get("success"))
            events = resp.get("events", [])
            self.assertGreater(len(events), 0)
            self.assertEqual(events[0]["event"], "detached")
            self.assertTrue(events[0]["body"]["all"])

            # Verify zxdb.Detach (all: True) was received by fake server
            detach_req = next(
                (
                    r
                    for r in self.received_dap_requests
                    if r.get("command") == "zxdb.Detach"
                ),
                None,
            )
            self.assertIsNotNone(
                detach_req,
                "zxdb.Detach command was not received by fake server",
            )
            # This assertion is redundant with the testing assertion made directly above, but is
            # actually necessary for mypy to be able to deduce that |detach_req| is definitely a
            # DetachRequest type and not None.
            assert detach_req is not None
            self.assertTrue(detach_req.get("arguments", {}).get("all"))

            # Verify active processes cleaned up in state
            f = StringIO()
            with contextlib.redirect_stdout(f):
                exit_code = await main(["get-state"])
            self.assertEqual(exit_code, 0)
            state_resp = json.loads(f.getvalue())
            self.assertTrue(state_resp.get("success"))
            self.assertEqual(state_resp.get("body", {}).get("processes"), {})

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)

    async def test_stop_detaches_existing_session(self) -> None:
        """Tests that stopping the daemon detaches from all processes if connected to existing."""
        # _setup_daemon_and_server starts fake DAP, starts daemon, and connects
        proc, port = await self._setup_daemon_and_server()

        try:
            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

            # Wait for daemon to exit
            await asyncio.wait_for(asyncio.to_thread(proc.wait), timeout=5.0)

            # Verify that the fake DAP server received "zxdb.Detach" request
            detach_request = None
            for req in self.received_dap_requests:
                if req.get("command") == "zxdb.Detach":
                    detach_request = req
                    break

            self.assertIsNotNone(
                detach_request, "zxdb.Detach was not received by DAP server"
            )
            assert detach_request is not None
            self.assertTrue(
                detach_request["arguments"].get("all"),
                "zxdb.Detach did not specify 'all'",
            )

        finally:
            await _cleanup_process_group(proc)

    async def test_evaluate_command(self) -> None:
        proc, port = await self._setup_daemon_and_server()

        try:
            exit_code = await main(["pause", "-t", "1"])
            self.assertEqual(exit_code, 0)

            f = StringIO()
            with contextlib.redirect_stdout(f):
                exit_code = await main(
                    ["evaluate", "--thread-id", "1", "1", "+", "1"]
                )
            self.assertEqual(exit_code, 0)

            output = f.getvalue()
            output_json = json.loads(output)
            self.assertTrue(output_json.get("success"))
            body = output_json.get("body")
            self.assertIsNotNone(body)
            self.assertEqual(body.get("result"), "42")
            self.assertEqual(body.get("type"), "int")

            # Verify that the fake DAP server received the evaluate command
            eval_req = next(
                (
                    r
                    for r in self.received_dap_requests
                    if r.get("command") == "evaluate"
                ),
                None,
            )
            self.assertIsNotNone(eval_req)
            assert eval_req is not None
            self.assertEqual(eval_req["arguments"]["expression"], "1 + 1")
            self.assertEqual(eval_req["arguments"]["frameId"], 1)

            # Stop via CLI
            exit_code = await main(["stop"])
            self.assertEqual(exit_code, 0)

        finally:
            await _cleanup_process_group(proc)


if __name__ == "__main__":
    unittest.main()
