# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import unittest
from unittest.mock import AsyncMock, Mock, patch

from daemon.daemon import Daemon
from pydap.client import READER_STOPPED_EVENT
from shared.protocol import Response
from shared.protocol.wait_for_event import WaitForEventRequest
from zxdb_dap import AsyncTaskNode


class TestDaemonEvents(unittest.IsolatedAsyncioTestCase):
    async def test_event_sequencing(self) -> None:
        # Port is unused in these tests.
        daemon = Daemon(port=None)

        # Put events into queue
        await daemon.event_queue.put(
            {"event": "thread", "body": {"threadId": 1}}
        )
        await daemon.event_queue.put(
            {"event": "stopped", "body": {"threadId": 1}}
        )

        # Run process events in background
        task = asyncio.create_task(daemon._process_events())

        # Wait for events to be processed (avoiding fixed sleep)
        for _ in range(100):
            if len(daemon.all_events) == 2:
                break
            await asyncio.sleep(0.01)

        task.cancel()

        self.assertEqual(len(daemon.all_events), 2)
        self.assertEqual(daemon.all_events[1]["seq"], 1)
        self.assertEqual(daemon.all_events[2]["seq"], 2)

    async def test_wait_for_event_immediate(self) -> None:
        daemon = Daemon(port=None)

        daemon.all_events = {
            1: {"seq": 1, "event": "thread"},
            2: {"seq": 2, "event": "stopped"},
        }
        daemon.latest_seq = 2

        resp = await daemon.registry.handle(
            "wait-for-event", WaitForEventRequest(last_seen_seq=1)
        )
        self.assertTrue(resp.success)
        assert resp.events is not None
        self.assertEqual(len(resp.events), 1)
        self.assertEqual(resp.events[0]["seq"], 2)

    async def test_wait_for_event_blocking(self) -> None:
        daemon = Daemon(port=None)

        async def add_event() -> None:
            # Give the waiter time to start waiting
            await asyncio.sleep(0.05)
            daemon.latest_seq = 1
            daemon.all_events[1] = {"seq": 1, "event": "stopped"}
            async with daemon.new_event_condition:
                daemon.new_event_condition.notify_all()

        asyncio.create_task(add_event())

        resp = await daemon.registry.handle(
            "wait-for-event", WaitForEventRequest(last_seen_seq=0)
        )
        self.assertTrue(resp.success)
        assert resp.events is not None
        self.assertEqual(len(resp.events), 1)
        self.assertEqual(resp.events[0]["seq"], 1)

    async def test_event_filtering(self) -> None:
        daemon = Daemon(port=None)

        # Put allowed and disallowed events into queue
        await daemon.event_queue.put(
            {"event": "stopped", "body": {"threadId": 1}}
        )
        await daemon.event_queue.put(
            {"event": "output", "body": {"output": "stdout"}}
        )
        await daemon.event_queue.put(
            {"event": "exited", "body": {"exitCode": 0}}
        )

        # Run process events in background
        task = asyncio.create_task(daemon._process_events())

        # Wait for events to be processed
        for _ in range(100):
            if len(daemon.all_events) == 2:
                break
            await asyncio.sleep(0.01)

        task.cancel()

        self.assertEqual(len(daemon.all_events), 2)
        self.assertEqual(daemon.all_events[1]["event"], "stopped")
        self.assertEqual(daemon.all_events[2]["event"], "exited")

    async def test_event_acknowledgment(self) -> None:
        daemon = Daemon(port=None)

        daemon.all_events = {
            1: {"seq": 1, "event": "thread"},
            2: {"seq": 2, "event": "stopped"},
            3: {"seq": 3, "event": "exited"},
        }
        daemon.latest_seq = 3

        mock_reader = AsyncMock()
        mock_reader.readline.return_value = (
            b'{"command": "threads", "ack_seq": 2}\n'
        )

        mock_writer = Mock()
        mock_writer.write = Mock()
        mock_writer.drain = AsyncMock()
        mock_writer.wait_closed = AsyncMock()

        with patch.object(
            daemon.registry, "handle", new_callable=AsyncMock
        ) as mock_handle:
            mock_handle.return_value = Response(success=True)
            await daemon.handle_uds_client(mock_reader, mock_writer)

        self.assertEqual(len(daemon.all_events), 1)
        self.assertEqual(daemon.all_events[3]["seq"], 3)

    async def test_process_event_handling(self) -> None:
        daemon = Daemon(port=None)

        # Put process event into queue
        await daemon.event_queue.put(
            {
                "event": "process",
                "body": {"systemProcessId": 1234, "name": "test_process"},
            }
        )

        # Run process events in background
        task = asyncio.create_task(daemon._process_events())

        # Wait for events to be processed
        for _ in range(100):
            if 1234 in daemon.processes:
                break
            await asyncio.sleep(0.01)

        task.cancel()

        self.assertIn(1234, daemon.processes)
        self.assertEqual(daemon.processes[1234].name, "test_process")

    async def test_reader_stopped_event_logs_warning(self) -> None:
        daemon = Daemon(port=None)
        err = RuntimeError("Connection forcibly closed by peer")
        await daemon.event_queue.put(
            {"type": READER_STOPPED_EVENT, "exception": err}
        )

        with self.assertLogs("daemon.daemon", level="WARNING") as cm:
            await daemon._process_events()

        self.assertTrue(
            any(
                "DAP reader background task stopped with error" in log
                for log in cm.output
            )
        )

    async def test_thread_event_handling_populates_thread_and_process(
        self,
    ) -> None:
        daemon = Daemon(port=None)
        await daemon.event_queue.put(
            {
                "event": "thread",
                "body": {"reason": "started", "threadId": 1, "processId": 1234},
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertIn(1, daemon.threads)
        assert daemon.threads[1].process is not None
        self.assertEqual(daemon.threads[1].process.id, 1234)

    async def test_thread_event_exited_evicts_thread(self) -> None:
        daemon = Daemon(port=None)
        daemon.get_or_create_thread(1, process_id=1234).is_stopped = True
        daemon.get_or_create_thread(2, process_id=1234).is_stopped = True
        await daemon.event_queue.put(
            {
                "event": "thread",
                "body": {"reason": "exited", "threadId": 1},
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertNotIn(1, daemon.threads)
        self.assertIn(2, daemon.threads)
        self.assertTrue(daemon.threads[2].is_stopped)
        assert daemon.threads[2].process is not None
        self.assertEqual(daemon.threads[2].process.id, 1234)

    async def test_detached_event_clears_process_threads(self) -> None:
        daemon = Daemon(port=None)
        daemon.get_or_create_process(1234, name="p1")
        daemon.get_or_create_process(5678, name="p2")
        daemon.get_or_create_thread(1, process_id=1234).is_stopped = True
        daemon.get_or_create_thread(2, process_id=1234).is_stopped = True
        daemon.get_or_create_thread(3, process_id=5678).is_stopped = True
        await daemon.event_queue.put(
            {
                "event": "detached",
                "body": {"processId": 1234},
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertNotIn(1234, daemon.processes)
        self.assertIn(5678, daemon.processes)
        self.assertNotIn(1, daemon.threads)
        self.assertNotIn(2, daemon.threads)
        self.assertIn(3, daemon.threads)
        self.assertTrue(daemon.threads[3].is_stopped)
        assert daemon.threads[3].process is not None
        self.assertEqual(daemon.threads[3].process.id, 5678)

    async def test_process_stopped_event(self) -> None:
        daemon = Daemon(port=None)
        proc = daemon.get_or_create_process(1234, name="p1")
        daemon.get_or_create_thread(1, process_id=1234)
        daemon.get_or_create_thread(2, process_id=1234)
        self.assertFalse(proc.all_threads_stopped)
        self.assertFalse(daemon.threads[1].is_stopped)
        self.assertFalse(daemon.threads[2].is_stopped)

        with daemon.event_waiter.wait_for_process_stop(1234) as fut:
            await daemon.event_queue.put(
                {
                    "event": "processStopped",
                    "body": {
                        "processId": 1234,
                        "name": "p1",
                        "threads": [1, 2],
                    },
                }
            )
            await daemon.event_queue.put({"type": READER_STOPPED_EVENT})
            await daemon._process_events()
            self.assertTrue(fut.done())

        self.assertTrue(proc.all_threads_stopped)
        self.assertTrue(daemon.threads[1].is_stopped)
        self.assertTrue(daemon.threads[2].is_stopped)

    async def test_async_backtrace_update_event_caching(self) -> None:
        daemon = Daemon(port=None)
        daemon.get_or_create_thread(100, process_id=5678)

        await daemon.event_queue.put(
            {
                "event": "zxdb.updateAsyncBacktrace",
                "body": {
                    "id": 100,
                    "name": "main",
                    "processId": 5678,
                    "tasks": [{"name": "task1"}],
                },
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertIn(5678, daemon.processes)
        self.assertEqual(
            daemon.processes[5678].async_backtrace,
            [AsyncTaskNode(name="task1")],
        )

    async def test_empty_tasks_event_does_not_clobber_non_empty_cache(
        self,
    ) -> None:
        daemon = Daemon(port=None)
        proc = daemon.get_or_create_process(5678)
        daemon.get_or_create_thread(100, process_id=5678)
        daemon.get_or_create_thread(101, process_id=5678)
        proc.async_backtrace = [AsyncTaskNode(name="valid_task")]

        # Running threads that are unrelated to the async executor will emit an empty task list.
        await daemon.event_queue.put(
            {
                "event": "zxdb.updateAsyncBacktrace",
                "body": {
                    "id": 101,
                    "name": "unrelated_thread",
                    "processId": 5678,
                    "tasks": [],
                },
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        # Cache must retain executor_thread's valid task list.
        assert proc.async_backtrace is not None
        self.assertEqual(
            proc.async_backtrace,
            [AsyncTaskNode(name="valid_task")],
        )

    async def test_empty_tasks_event_caches_empty_list(self) -> None:
        daemon = Daemon(port=None)
        proc = daemon.get_or_create_process(5678)
        daemon.get_or_create_thread(100, process_id=5678)

        await daemon.event_queue.put(
            {
                "event": "zxdb.updateAsyncBacktrace",
                "body": {
                    "id": 100,
                    "name": "thread",
                    "processId": 5678,
                    "tasks": [],
                },
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertEqual(proc.async_backtrace, [])

    async def test_event_with_tasks_overwrites_empty_tasks_cache(self) -> None:
        daemon = Daemon(port=None)
        proc = daemon.get_or_create_process(5678)
        daemon.get_or_create_thread(100, process_id=5678)
        daemon.get_or_create_thread(101, process_id=5678)

        # An unrelated thread emits an empty task list first.
        await daemon.event_queue.put(
            {
                "event": "zxdb.updateAsyncBacktrace",
                "body": {
                    "id": 100,
                    "name": "unrelated_thread",
                    "processId": 5678,
                    "tasks": [],
                },
            }
        )
        # Followed immediately by an executor thread emitting valid tasks.
        await daemon.event_queue.put(
            {
                "event": "zxdb.updateAsyncBacktrace",
                "body": {
                    "id": 101,
                    "name": "executor_thread",
                    "processId": 5678,
                    "tasks": [{"name": "valid_task"}],
                },
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertEqual(
            proc.async_backtrace,
            [AsyncTaskNode(name="valid_task")],
        )

    async def test_async_backtrace_invalid_model_rejected(self) -> None:
        daemon = Daemon(port=None)
        proc = daemon.get_or_create_process(5678)
        daemon.get_or_create_thread(100, process_id=5678)

        await daemon.event_queue.put(
            {
                "event": "zxdb.updateAsyncBacktrace",
                "body": {
                    "id": 100,
                    "name": "thread",
                    "processId": 5678,
                    "tasks": [{"line": "invalid_not_an_int"}],
                },
            }
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertIsNone(proc.async_backtrace)

    async def test_detached_event_clears_async_backtrace_cache(self) -> None:
        daemon = Daemon(port=None)
        proc = daemon.get_or_create_process(5678, name="p1")
        proc.async_backtrace = [AsyncTaskNode(name="main")]

        await daemon.event_queue.put(
            {"event": "detached", "body": {"processId": 5678}}
        )
        await daemon.event_queue.put({"type": READER_STOPPED_EVENT})

        await daemon._process_events()

        self.assertNotIn(5678, daemon.processes)


if __name__ == "__main__":
    unittest.main()
