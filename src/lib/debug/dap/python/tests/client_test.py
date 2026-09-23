# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import io
import json
import unittest
from typing import Any

from pydap.client import DapClient, DapError
from pydap.dap_types import Source, SourceBreakpoint
from pydap.models import (
    AttachRequestArguments,
    ContinueArguments,
    ContinueResponse,
    DisconnectArguments,
    EvaluateArguments,
    InitializeArguments,
    LaunchArguments,
    NextArguments,
    PauseArguments,
    ScopesArguments,
    SetBreakpointsArguments,
    StackTraceArguments,
    StepInArguments,
    StepOutArguments,
    VariablesArguments,
)


class MockWriter:
    def __init__(self) -> None:
        self.buffer = io.BytesIO()

    def write(self, data: bytes) -> None:
        self.buffer.write(data)

    async def drain(self) -> None:
        pass


class FailingWriter:
    def write(self, data: bytes) -> None:
        raise OSError("Connection reset by peer")

    async def drain(self) -> None:
        pass


def feed_dap_response(
    reader: asyncio.StreamReader, response: dict[str, Any]
) -> None:
    body = json.dumps(response, separators=(",", ":")).encode("utf-8")
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8")
    reader.feed_data(header + body)


class TestDapClient(unittest.IsolatedAsyncioTestCase):
    def _start_client(
        self, client: DapClient
    ) -> tuple[asyncio.StreamReader, MockWriter]:
        reader = asyncio.StreamReader()
        writer = MockWriter()
        event_queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        client.run(reader, writer, event_queue)
        return reader, writer

    # Should not send request before running the client.
    async def test_not_running_error(self) -> None:
        client = DapClient()
        with self.assertRaises(DapError) as cm:
            client._send_request_future("initialize")
        self.assertIn("DapClient is not running", str(cm.exception))

    async def test_run(self) -> None:
        client = DapClient()
        reader = asyncio.StreamReader()
        writer = MockWriter()
        event_queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue()

        client.run(reader, writer, event_queue)
        self.assertTrue(client.is_running)

        reader.feed_eof()
        await client.close()
        self.assertFalse(client.is_running)

    async def test_already_running_error(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        with self.assertRaises(DapError) as cm:
            client.run(reader, writer, asyncio.Queue())
        self.assertIn("DapClient is already running", str(cm.exception))

    async def test_read_message(self) -> None:
        data = b'Content-Length: 26\r\n\r\n{"seq":1,"type":"request"}'
        reader = asyncio.StreamReader()
        reader.feed_data(data)
        reader.feed_eof()

        client = DapClient()
        msg = await client._read_message(reader)
        self.assertIsNotNone(msg)
        assert msg is not None
        self.assertEqual(msg["seq"], 1)

        self.assertEqual(msg["type"], "request")

    async def test_write_message(self) -> None:
        client = DapClient()
        value = {"seq": 1, "type": "request"}

        writer = MockWriter()
        await client._write_message(writer, value)

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        self.assertEqual(headers, b"Content-Length: 26")
        self.assertEqual(json.loads(body.decode("utf-8")), value)

    async def test__send_request(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        send_task = asyncio.create_task(client._send_request("initialize"))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "initialize",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp["success"])
        self.assertEqual(resp["request_seq"], seq)

    async def test_initialize(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = InitializeArguments(adapter_id="test")
        send_task = asyncio.create_task(client.initialize(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "initialize",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)

    async def test_disconnect(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = DisconnectArguments(terminate_debuggee=True)
        send_task = asyncio.create_task(client.disconnect(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "disconnect",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)

    async def test_stack_trace(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = StackTraceArguments(thread_id=1)
        send_task = asyncio.create_task(client.stack_trace(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "stackTrace",
            "body": {"stackFrames": []},
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertEqual(resp.body.stack_frames, [])

    async def test_stack_trace_presentation_hint(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = StackTraceArguments(thread_id=1)
        send_task = asyncio.create_task(client.stack_trace(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "stackTrace",
            "body": {
                "stackFrames": [
                    {
                        "id": 1,
                        "name": "frame1",
                        "line": 10,
                        "column": 1,
                        "presentationHint": "subtle",
                        "source": {
                            "origin": "Rust panic",
                        },
                    }
                ]
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertEqual(len(resp.body.stack_frames), 1)
        frame = resp.body.stack_frames[0]
        self.assertEqual(frame.presentation_hint, "subtle")
        self.assertIsNotNone(frame.source)
        assert frame.source is not None
        self.assertEqual(frame.source.origin, "Rust panic")

    async def test_continue_thread(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = ContinueArguments(thread_id=1)
        send_task = asyncio.create_task(client.continue_thread(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "continue",
            "body": {"allThreadsContinued": True},
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertIsInstance(resp, ContinueResponse)
        self.assertTrue(resp.success)
        self.assertIsNotNone(resp.body)
        self.assertTrue(resp.body.all_threads_continued)

    def test_continue_response_empty_body(self) -> None:
        response = {
            "seq": 10,
            "type": "response",
            "request_seq": 1,
            "success": True,
            "command": "continue",
            "body": {},
        }
        resp = ContinueResponse.model_validate(response)
        self.assertIsInstance(resp, ContinueResponse)
        self.assertIsNotNone(resp.body)
        self.assertTrue(resp.body.all_threads_continued)

    def test_continue_response_missing_body(self) -> None:
        response = {
            "seq": 10,
            "type": "response",
            "request_seq": 1,
            "success": True,
            "command": "continue",
        }
        resp = ContinueResponse.model_validate(response)
        self.assertIsInstance(resp, ContinueResponse)
        self.assertIsNotNone(resp.body)
        self.assertTrue(resp.body.all_threads_continued)

    async def test_pause_thread(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = PauseArguments(thread_id=1)
        send_task = asyncio.create_task(client.pause_thread(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "pause",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)

    async def test_step_out(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = StepOutArguments(
            thread_id=1, single_thread=True, granularity="line"
        )
        send_task = asyncio.create_task(client.step_out(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "stepOut",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["threadId"], 1)
        self.assertTrue(req_val["arguments"]["singleThread"])
        self.assertEqual(req_val["arguments"]["granularity"], "line")

    async def test_next(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = NextArguments(
            thread_id=1, single_thread=True, granularity="line"
        )
        send_task = asyncio.create_task(client.next(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "next",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["threadId"], 1)
        self.assertTrue(req_val["arguments"]["singleThread"])
        self.assertEqual(req_val["arguments"]["granularity"], "line")

    async def test_next_minimal(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = NextArguments(thread_id=2)
        send_task = asyncio.create_task(client.next(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "next",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["threadId"], 2)
        self.assertNotIn("singleThread", req_val["arguments"])
        self.assertNotIn("granularity", req_val["arguments"])

    async def test_step_in(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = StepInArguments(
            thread_id=1, single_thread=True, target_id=0, granularity="line"
        )
        send_task = asyncio.create_task(client.step_in(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "stepIn",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["threadId"], 1)
        self.assertTrue(req_val["arguments"]["singleThread"])
        self.assertEqual(req_val["arguments"]["targetId"], 0)
        self.assertEqual(req_val["arguments"]["granularity"], "line")

    async def test_step_in_minimal(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = StepInArguments(thread_id=2)
        send_task = asyncio.create_task(client.step_in(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 11,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "stepIn",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["threadId"], 2)
        self.assertNotIn("singleThread", req_val["arguments"])
        self.assertNotIn("targetId", req_val["arguments"])
        self.assertNotIn("granularity", req_val["arguments"])

    async def test_step_in_error(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = StepInArguments(thread_id=1)
        send_task = asyncio.create_task(client.step_in(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 12,
            "type": "response",
            "request_seq": seq,
            "success": False,
            "command": "stepIn",
            "message": "Thread not stopped",
        }

        feed_dap_response(reader, response)

        with self.assertRaises(DapError) as ctx:
            await send_task
        self.assertIn("Thread not stopped", str(ctx.exception))

    async def test_threads(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        send_task = asyncio.create_task(client.threads())

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "threads",
            "body": {
                "threads": [
                    {"id": 1234, "name": "main"},
                    {"id": 5678, "name": "worker"},
                ]
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertEqual(len(resp.body.threads), 2)
        self.assertEqual(resp.body.threads[0].id, 1234)
        self.assertEqual(resp.body.threads[0].name, "main")

    async def test_attach(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        args = AttachRequestArguments(
            restart=True, extra_fields={"process": "my_process"}
        )
        send_task = asyncio.create_task(client.attach(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "attach",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertTrue(req_val["arguments"]["__restart"])
        self.assertEqual(req_val["arguments"]["process"], "my_process")

    async def test_launch(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = LaunchArguments(process="my_process", launch_command="run")
        send_task = asyncio.create_task(client.launch(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "launch",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["process"], "my_process")
        self.assertEqual(req_val["arguments"]["launchCommand"], "run")

    async def test_evaluate(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = EvaluateArguments(
            expression="1 + 1", context="repl", frame_id=42
        )
        send_task = asyncio.create_task(client.evaluate(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "evaluate",
            "body": {
                "result": "2",
                "type": "int",
                "variablesReference": 0,
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(resp.body.result, "2")
        self.assertEqual(resp.body.type, "int")
        self.assertEqual(resp.body.variables_reference, 0)
        self.assertEqual(req_val["arguments"]["expression"], "1 + 1")
        self.assertEqual(req_val["arguments"]["context"], "repl")
        self.assertEqual(req_val["arguments"]["frameId"], 42)

    async def test_scopes(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = ScopesArguments(frame_id=42)
        send_task = asyncio.create_task(client.scopes(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "scopes",
            "body": {
                "scopes": [
                    {
                        "name": "Locals",
                        "variablesReference": 100,
                        "expensive": False,
                    }
                ]
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(len(resp.body.scopes), 1)
        self.assertEqual(resp.body.scopes[0].name, "Locals")
        self.assertEqual(resp.body.scopes[0].variables_reference, 100)
        self.assertFalse(resp.body.scopes[0].expensive)
        self.assertEqual(req_val["arguments"]["frameId"], 42)

    async def test_variables(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = VariablesArguments(variables_reference=100)
        send_task = asyncio.create_task(client.variables(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "variables",
            "body": {
                "variables": [
                    {
                        "name": "foo",
                        "value": "bar",
                        "variablesReference": 0,
                        "type": "str",
                    }
                ]
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(len(resp.body.variables), 1)
        self.assertEqual(resp.body.variables[0].name, "foo")
        self.assertEqual(resp.body.variables[0].value, "bar")
        self.assertEqual(resp.body.variables[0].variables_reference, 0)
        self.assertEqual(resp.body.variables[0].type, "str")
        self.assertEqual(req_val["arguments"]["variablesReference"], 100)
        self.assertNotIn("start", req_val["arguments"])
        self.assertNotIn("count", req_val["arguments"])

    async def test_variables_with_start_only(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = VariablesArguments(variables_reference=100, start=5)
        send_task = asyncio.create_task(client.variables(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "variables",
            "body": {"variables": []},
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["variablesReference"], 100)
        self.assertEqual(req_val["arguments"]["start"], 5)
        self.assertNotIn("count", req_val["arguments"])

    async def test_variables_with_count_only(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = VariablesArguments(variables_reference=100, count=10)
        send_task = asyncio.create_task(client.variables(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "variables",
            "body": {"variables": []},
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["variablesReference"], 100)
        self.assertNotIn("start", req_val["arguments"])
        self.assertEqual(req_val["arguments"]["count"], 10)

    async def test_variables_with_start_and_count(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = VariablesArguments(variables_reference=100, start=5, count=10)
        send_task = asyncio.create_task(client.variables(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "variables",
            "body": {"variables": []},
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["arguments"]["variablesReference"], 100)
        self.assertEqual(req_val["arguments"]["start"], 5)
        self.assertEqual(req_val["arguments"]["count"], 10)

    async def test_set_breakpoints(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = SetBreakpointsArguments(
            source=Source(path="/path/to/file.rs"),
            breakpoints=[SourceBreakpoint(line=12)],
        )
        send_task = asyncio.create_task(client.set_breakpoints(args))

        await asyncio.sleep(0)
        await client._write_queue.join()

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "setBreakpoints",
            "body": {
                "breakpoints": [
                    {
                        "id": 1,
                        "verified": True,
                        "source": {"path": "/path/to/file.rs"},
                        "line": 12,
                    }
                ]
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(len(resp.body.breakpoints), 1)
        self.assertEqual(resp.body.breakpoints[0].id, 1)
        self.assertTrue(resp.body.breakpoints[0].verified)
        self.assertEqual(resp.body.breakpoints[0].line, 12)
        self.assertEqual(
            req_val["arguments"]["source"]["path"], "/path/to/file.rs"
        )

    async def test_error_response(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)
        args = SetBreakpointsArguments(source=Source(path="relative.rs"))

        send_task = asyncio.create_task(client.set_breakpoints(args))
        await asyncio.sleep(0)
        await client._write_queue.join()
        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": False,
            "command": "setBreakpoints",
            "message": "SetBreakpointsRequest path must be absolute!",
        }

        feed_dap_response(reader, response)

        with self.assertRaises(DapError) as cm:
            await send_task
        self.assertIn(
            "SetBreakpointsRequest path must be absolute!", str(cm.exception)
        )

    async def test_reader_failure_fails_pending_futures(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        seq, sent_fut, data_fut = client._send_request_future("initialize")
        await asyncio.sleep(0)
        await client._write_queue.join()

        # Feed corrupted header length to cause _read_message to raise DapError
        reader.feed_data(b"Content-Length: invalid\r\n\r\n")

        with self.assertRaises(DapError):
            await data_fut

        self.assertEqual(len(client._pending_requests), 0)
        self.assertFalse(client.is_running)

    async def test_writer_failure_fails_pending_future(self) -> None:
        client = DapClient()
        reader = asyncio.StreamReader()
        writer = FailingWriter()
        event_queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        client.run(reader, writer, event_queue)

        seq, sent_fut, data_fut = client._send_request_future("initialize")

        with self.assertRaises(OSError) as cm_sent:
            await sent_fut
        self.assertIn("Connection reset by peer", str(cm_sent.exception))

        with self.assertRaises(OSError) as cm_data:
            await data_fut
        self.assertIn("Connection reset by peer", str(cm_data.exception))

    async def test_close_cancels_write_queue_futures(self) -> None:
        client = DapClient()
        reader, writer = self._start_client(client)

        seq, sent_fut, data_fut = client._send_request_future("initialize")

        await client.close()

        self.assertTrue(sent_fut.cancelled())
        self.assertTrue(data_fut.cancelled())
        self.assertFalse(client.is_running)

    async def test_caller_cancelled_data_fut_cancels_sent_fut(self) -> None:
        client = DapClient()
        reader = asyncio.StreamReader()
        writer = MockWriter()
        event_queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue()

        client.run(reader, writer, event_queue)
        seq, sent_fut, data_fut = client._send_request_future("initialize")
        # Cancel data_fut before client processes the write queue
        data_fut.cancel()

        await client._write_queue.join()

        self.assertTrue(sent_fut.cancelled())
        # Verify nothing was written to writer
        self.assertEqual(len(writer.buffer.getvalue()), 0)
        await client.close()
