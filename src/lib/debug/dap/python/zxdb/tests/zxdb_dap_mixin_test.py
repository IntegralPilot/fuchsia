# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import io
import json
import unittest
from typing import Any

from pydantic import ValidationError
from pydap.client import DapError
from zxdb_dap import (
    AsyncBacktraceUpdate,
    ThreadEvent,
    ZxdbDapClient,
    ZxdbDetachArguments,
    ZxdbPauseArguments,
    ZxdbProcessArguments,
    ZxdbProcessStoppedEvent,
    ZxdbStackTraceArguments,
    ZxdbThread,
    ZxdbThreadEvent,
    ZxdbThreadsResponse,
)


class MockWriter(asyncio.StreamWriter):
    def __init__(self) -> None:
        self.buffer = io.BytesIO()
        self.drained = asyncio.Event()

    def write(self, data: bytes) -> None:
        self.buffer.write(data)

    async def drain(self) -> None:
        self.drained.set()


def feed_dap_response(
    reader: asyncio.StreamReader, response: dict[str, Any]
) -> None:
    body = json.dumps(response, separators=(",", ":")).encode("utf-8")
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8")
    reader.feed_data(header + body)


class TestZxdbDapMixin(unittest.IsolatedAsyncioTestCase):
    def _start_client(
        self, client: ZxdbDapClient
    ) -> tuple[asyncio.StreamReader, MockWriter]:
        reader = asyncio.StreamReader()
        writer = MockWriter()
        event_queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        client.run(reader, writer, event_queue)
        return reader, writer

    async def test_zxdb_detach_pid(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        args = ZxdbDetachArguments(pid=1234)

        send_task = asyncio.create_task(client.zxdb_detach(args))

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "zxdb.Detach",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp["success"])
        self.assertEqual(req_val["command"], "zxdb.Detach")
        self.assertEqual(req_val["arguments"]["pid"], 1234)
        self.assertIsNone(req_val["arguments"].get("all"))

    async def test_zxdb_detach_all(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        args = ZxdbDetachArguments(detach_all=True)

        send_task = asyncio.create_task(client.zxdb_detach(args))

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "zxdb.Detach",
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp["success"])
        self.assertEqual(req_val["command"], "zxdb.Detach")
        self.assertIsNone(req_val["arguments"].get("pid"))
        self.assertTrue(req_val["arguments"]["all"])

    def test_zxdb_detach_invalid_args(self) -> None:
        with self.assertRaises(ValueError):
            ZxdbDetachArguments(pid=1234, detach_all=True)
        with self.assertRaises(ValueError):
            ZxdbDetachArguments(pid=None, detach_all=None)

    async def test_zxdb_process_all(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)

        send_task = asyncio.create_task(client.zxdb_process())

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "zxdb.Process",
            "body": {
                "processes": [
                    {
                        "id": 1001,
                        "name": "proc1",
                        "threads": [{"id": 2001, "name": "t1"}],
                    },
                    {
                        "id": 1002,
                        "name": "proc2",
                        "threads": [{"id": 2002, "name": "t2"}],
                    },
                ],
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["command"], "zxdb.Process")
        self.assertEqual(len(resp.body.processes), 2)
        self.assertEqual(resp.body.processes[0].id, 1001)
        self.assertEqual(resp.body.processes[0].name, "proc1")
        self.assertEqual(len(resp.body.processes[0].threads), 1)
        self.assertEqual(resp.body.processes[0].threads[0].id, 2001)

    async def test_zxdb_process_pid(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        args = ZxdbProcessArguments(pid=1001)

        send_task = asyncio.create_task(client.zxdb_process(args))

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": True,
            "command": "zxdb.Process",
            "body": {
                "processes": [
                    {
                        "id": 1001,
                        "name": "proc1",
                        "threads": [{"id": 2001, "name": "t1"}],
                    }
                ],
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["command"], "zxdb.Process")
        self.assertEqual(req_val["arguments"]["pid"], 1001)
        self.assertEqual(len(resp.body.processes), 1)
        self.assertEqual(resp.body.processes[0].id, 1001)

    async def test_zxdb_process_error(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        args = ZxdbProcessArguments(pid=9999)

        send_task = asyncio.create_task(client.zxdb_process(args))

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

        buffer_val = writer.buffer.getvalue()
        headers, body = buffer_val.split(b"\r\n\r\n", 1)
        req_val = json.loads(body.decode("utf-8"))
        seq = req_val["seq"]

        response = {
            "seq": 10,
            "type": "response",
            "request_seq": seq,
            "success": False,
            "command": "zxdb.Process",
            "message": "Process not found",
        }

        feed_dap_response(reader, response)

        with self.assertRaises(DapError) as ctx:
            await send_task
        self.assertIn("Process not found", str(ctx.exception))

    async def test_zxdb_stack_trace(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        args = ZxdbStackTraceArguments(thread_id=5678, remote_unwind=True)

        send_task = asyncio.create_task(client.zxdb_stack_trace(args))

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

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
                "stackFrames": [],
                "totalFrames": 0,
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertEqual(req_val["command"], "stackTrace")
        self.assertEqual(req_val["arguments"]["threadId"], 5678)
        self.assertTrue(req_val["arguments"]["remoteUnwind"])

    async def test_threads_response_process_id(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        send_task = asyncio.create_task(client.zxdb_threads())

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

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
                    {"id": 1234, "name": "main", "processId": 5678},
                ]
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertTrue(isinstance(resp, ZxdbThreadsResponse))
        self.assertEqual(len(resp.body.threads), 1)
        self.assertEqual(resp.body.threads[0].id, 1234)
        self.assertEqual(resp.body.threads[0].name, "main")
        self.assertEqual(resp.body.threads[0].process_id, 5678)

    async def test_threads_response_process_id_optional(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        send_task = asyncio.create_task(client.zxdb_threads())

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

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
                ]
            },
        }

        feed_dap_response(reader, response)

        resp = await send_task
        self.assertTrue(resp.success)
        self.assertTrue(isinstance(resp, ZxdbThreadsResponse))
        self.assertEqual(len(resp.body.threads), 1)
        self.assertEqual(resp.body.threads[0].id, 1234)
        self.assertEqual(resp.body.threads[0].name, "main")
        self.assertIsNone(resp.body.threads[0].process_id)

    def test_thread_event_process_id(self) -> None:
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "thread",
            "body": {
                "reason": "started",
                "threadId": 1234,
                "processId": 5678,
            },
        }
        event = ZxdbThreadEvent.model_validate(event_dict)
        self.assertEqual(event.event, "thread")
        self.assertEqual(event.body.reason, "started")
        self.assertEqual(event.body.thread_id, 1234)
        self.assertEqual(event.body.process_id, 5678)

    def test_standard_thread_event_without_process_id(self) -> None:
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "thread",
            "body": {
                "reason": "started",
                "threadId": 1234,
            },
        }
        event = ThreadEvent.model_validate(event_dict)
        self.assertEqual(event.event, "thread")
        self.assertEqual(event.body.reason, "started")
        self.assertEqual(event.body.thread_id, 1234)

    def test_zxdb_thread_dump_dap_process_id(self) -> None:
        thread = ZxdbThread(id=1, name="test", process_id=1234)
        dap_dict = thread.dump_dap()
        self.assertEqual(dap_dict["id"], 1)
        self.assertEqual(dap_dict["name"], "test")
        self.assertEqual(dap_dict["processId"], 1234)

    def test_thread_event_validation_invalid_event(self) -> None:
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "stopped",
            "body": {
                "reason": "started",
                "threadId": 1234,
                "processId": 5678,
            },
        }
        with self.assertRaises(ValidationError):
            ThreadEvent.model_validate(event_dict)

    def test_thread_event_validation_custom_reason(self) -> None:
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "thread",
            "body": {
                "reason": "unknown_reason",
                "threadId": 1234,
            },
        }
        event = ThreadEvent.model_validate(event_dict)
        self.assertEqual(event.body.reason, "unknown_reason")
        self.assertEqual(event.body.thread_id, 1234)

    def test_zxdb_thread_event_validation_custom_reason(self) -> None:
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "thread",
            "body": {
                "reason": "custom_reason",
                "threadId": 1234,
                "processId": 5678,
            },
        }
        event = ZxdbThreadEvent.model_validate(event_dict)
        self.assertEqual(event.body.reason, "custom_reason")
        self.assertEqual(event.body.thread_id, 1234)
        self.assertEqual(event.body.process_id, 5678)

    async def test_zxdb_pause_process(self) -> None:
        client = ZxdbDapClient()
        reader, writer = self._start_client(client)
        args = ZxdbPauseArguments(process_id=12345)

        send_task = asyncio.create_task(client.zxdb_pause_process(args))

        await asyncio.wait_for(writer.drained.wait(), timeout=2.0)

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
        self.assertEqual(req_val["command"], "pause")
        self.assertEqual(req_val["arguments"]["processId"], 12345)

    def test_zxdb_pause_invalid_args(self) -> None:
        with self.assertRaises(ValueError):
            ZxdbPauseArguments(process_id=None)

    def test_zxdb_process_stopped_event(self) -> None:
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "processStopped",
            "body": {
                "processId": 999,
                "name": "my_process",
                "threads": [1234, 5678],
            },
        }
        event = ZxdbProcessStoppedEvent.model_validate(event_dict)
        self.assertEqual(event.event, "processStopped")
        self.assertEqual(event.body.process_id, 999)
        self.assertEqual(event.body.name, "my_process")
        self.assertEqual(event.body.threads, [1234, 5678])

    def test_async_backtrace_update_event(self) -> None:
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "zxdb.updateAsyncBacktrace",
            "body": {
                "id": 1234,
                "name": "main",
                "processId": 5678,
                "tasks": [
                    {
                        "id": "0x1",
                        "name": "Task 1",
                        "file": "foo.cc",
                        "line": 42,
                        "children": [
                            {
                                "id": "0x2",
                                "name": "Task 2",
                                "file": "bar.cc",
                                "line": 24,
                                "children": [],
                            }
                        ],
                    }
                ],
            },
        }
        event = AsyncBacktraceUpdate.model_validate(event_dict)
        self.assertEqual(event.event, "zxdb.updateAsyncBacktrace")
        self.assertEqual(event.body.thread_id, 1234)
        self.assertEqual(event.body.name, "main")
        self.assertEqual(event.body.process_id, 5678)
        self.assertEqual(len(event.body.tasks), 1)

        task1 = event.body.tasks[0]
        self.assertEqual(task1.id, "0x1")
        self.assertEqual(task1.name, "Task 1")
        self.assertEqual(task1.file, "foo.cc")
        self.assertEqual(task1.line, 42)
        self.assertEqual(len(task1.children), 1)

        task2 = task1.children[0]
        self.assertEqual(task2.id, "0x2")
        self.assertEqual(task2.name, "Task 2")
        self.assertEqual(task2.file, "bar.cc")
        self.assertEqual(task2.line, 24)
        self.assertEqual(len(task2.children), 0)

    def test_async_backtrace_update_event_optional_defaults(self) -> None:
        """Tests deserializing a payload with omitted optional fields."""
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "zxdb.updateAsyncBacktrace",
            "body": {
                "id": 1234,
                "name": "main",
                "processId": 5678,
                "tasks": [
                    {
                        "name": "Task Minimal",
                    }
                ],
            },
        }
        event = AsyncBacktraceUpdate.model_validate(event_dict)
        self.assertEqual(event.body.thread_id, 1234)
        self.assertEqual(event.body.process_id, 5678)
        self.assertEqual(len(event.body.tasks), 1)

        task = event.body.tasks[0]
        self.assertIsNone(task.id)
        self.assertEqual(task.name, "Task Minimal")
        self.assertIsNone(task.file)
        self.assertIsNone(task.line)
        self.assertEqual(task.children, [])

    def test_async_backtrace_update_event_invalid_event(self) -> None:
        """Tests that mismatched event discriminators fail validation."""
        event_dict = {
            "seq": 1,
            "type": "event",
            "event": "invalid.event",
            "body": {
                "id": 1234,
                "name": "main",
                "tasks": [],
            },
        }
        with self.assertRaises(ValidationError):
            AsyncBacktraceUpdate.model_validate(event_dict)


if __name__ == "__main__":
    unittest.main()
