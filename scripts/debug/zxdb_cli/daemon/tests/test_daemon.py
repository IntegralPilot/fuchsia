# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import unittest
from typing import Any
from unittest.mock import AsyncMock, Mock, patch

from daemon.daemon import CommandHandlerRegistry, Daemon
from daemon.state import Process, Thread
from pydap.client import DapClient, DapError
from pydap.dap_types import Source, StackFrame
from pydap.models import PauseArguments
from pydap.models import Response as DapResponse
from shared.protocol import (
    BaseRequest,
    BreakRequest,
    GetStateResponse,
    Response,
)
from shared.protocol.async_backtrace import (
    AsyncBacktraceRequest,
    AsyncBacktraceResponse,
)
from shared.protocol.attach import AttachRequest
from shared.protocol.continue_request import ContinueRequest
from shared.protocol.evaluate import EvaluateRequest, EvaluateResponse
from shared.protocol.finish import FinishRequest
from shared.protocol.get_state import GetStateRequest
from shared.protocol.next_request import NextRequest
from shared.protocol.stack_trace import (
    ProcessStackTraceResponse,
    StackTraceRequest,
    ThreadStackTraceResponse,
)
from shared.protocol.step_in import StepInRequest
from shared.protocol.threads import ThreadsRequest
from shared.protocol.variables import VariablesRequest
from zxdb_dap import AsyncTaskNode, ZxdbPauseArguments


class TestCommandHandlerRegistry(unittest.IsolatedAsyncioTestCase):
    async def test_register_and_handle(self) -> None:
        registry = CommandHandlerRegistry()

        async def mock_handler(_req: BaseRequest[Any]) -> Response[Any]:
            return Response(success=True, body={"data": "handled"})

        registry.register("test_cmd", mock_handler)

        resp = await registry.handle(
            "test_cmd", BaseRequest[Any](command="test_cmd")
        )

        self.assertTrue(resp.success)
        self.assertEqual(resp.body, {"data": "handled"})

    async def test_unknown_command(self) -> None:
        registry = CommandHandlerRegistry()
        resp = await registry.handle(
            "unknown", BaseRequest[Any](command="unknown")
        )

        self.assertFalse(resp.success)
        self.assertIsNotNone(resp.message)
        self.assertIn("Unknown command", resp.message or "")

    async def test_attach_registration(self) -> None:
        daemon = Daemon(port=15678)
        self.assertIn("attach", daemon.registry.handlers)

    async def test_handle_attach_success(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon.dap_client, "attach", new_callable=AsyncMock
        ) as mock_attach:
            mock_attach_resp = Mock()
            mock_attach_resp.dump_dap.return_value = {"success": True}
            mock_attach.return_value = mock_attach_resp

            req = AttachRequest(filter="my_process")
            resp = await daemon.registry.handle("attach", req)

            self.assertTrue(resp.success)
            self.assertEqual(resp.body, {"success": True})
            mock_attach.assert_called_once()

    async def test_handle_attach_failure(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon.dap_client, "attach", new_callable=AsyncMock
        ) as mock_attach:
            mock_attach.side_effect = Exception("Failed to attach")

            req = AttachRequest(filter="my_process")
            resp = await daemon.registry.handle("attach", req)

            self.assertFalse(resp.success)
            self.assertIn("Failed to attach", resp.message or "")

    async def test_handle_attach_not_connected(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = None

        req = AttachRequest(filter="my_process")
        resp = await daemon.registry.handle("attach", req)

        self.assertFalse(resp.success)
        self.assertIn("Not connected", resp.message or "")

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_request_waits_for_startup_lock_instead_of_failing(
        self, mock_dap_client_class: Mock
    ) -> None:
        """Verifies requests arriving while start is executing wait on _command_lock.

        Otherwise, it would show "Not connected to zxdb DAP server" because of an uninitialized zxdb_writer.
        """
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = None  # DAP server not yet connected

        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_client.attach = AsyncMock(
            return_value=Mock(dump_dap=lambda: {"success": True})
        )

        # 1. Simulate 'start' acquiring the command lock during initialization
        await daemon._command_lock.acquire()

        # 2. Background task simulates 'start' completing DAP connection after 50ms
        async def complete_dap_startup_after_delay() -> None:
            await asyncio.sleep(0.05)
            daemon.zxdb_writer = Mock()
            daemon._command_lock.release()

        startup_task = asyncio.create_task(complete_dap_startup_after_delay())

        # 3. Dispatch an operational command while startup is still locked.
        # It should block for 50ms, acquire the lock after startup completes, and succeed.
        req = AttachRequest(filter="my_process")
        async with daemon._command_lock:
            resp = await daemon.registry.handle("attach", req)

        self.assertTrue(resp.success)
        mock_dap_client.attach.assert_called_once()
        await startup_task

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_commands_execute_in_strict_fifo_order(
        self, mock_dap_client_class: Mock
    ) -> None:
        """Verifies multiple commands arriving concurrently execute in FIFO order without interleaving."""
        execution_log: list[str] = []

        async def delayed_attach_side_effect(*args: Any, **kwargs: Any) -> Mock:
            execution_log.append("attach_start")
            await asyncio.sleep(0.05)
            execution_log.append("attach_end")
            return Mock(dump_dap=lambda: {"success": True})

        async def continue_side_effect(*args: Any, **kwargs: Any) -> Mock:
            execution_log.append("continue_start")
            await asyncio.sleep(0.01)
            execution_log.append("continue_end")
            return Mock(dump_dap=lambda: {"success": True})

        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_client.attach = AsyncMock(
            side_effect=delayed_attach_side_effect
        )
        mock_dap_client.continue_thread = AsyncMock(
            side_effect=continue_side_effect
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        async def dispatch(cmd: str, req: Any) -> Response[Any]:
            async with daemon._command_lock:
                return await daemon.registry.handle(cmd, req)

        # Launch attach first (takes 50ms)
        task1 = asyncio.create_task(
            dispatch("attach", AttachRequest(filter="proc1"))
        )
        # Ensure task1 acquires the lock first
        await asyncio.sleep(0.005)
        # Launch continue second (takes 10ms); should wait for attach to finish
        task2 = asyncio.create_task(
            dispatch("continue", ContinueRequest(thread_id=1))
        )

        resp1, resp2 = await asyncio.gather(task1, task2)

        self.assertTrue(resp1.success)
        self.assertTrue(resp2.success)
        self.assertEqual(
            execution_log,
            ["attach_start", "attach_end", "continue_start", "continue_end"],
        )

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_continue(self, mock_dap_client_class: Mock) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_continue_resp = Mock()
        mock_continue_resp.body = Mock()
        mock_continue_resp.body.all_threads_continued = True
        mock_continue_resp.dump_dap.return_value = {"allThreadsContinued": True}
        mock_dap_client.continue_thread = AsyncMock(
            return_value=mock_continue_resp
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread = daemon.get_or_create_thread(1, process_id=1234)
        thread.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "continue", ContinueRequest(thread_id=1)
        )

        self.assertTrue(resp.success)
        mock_dap_client.continue_thread.assert_called_once()
        self.assertFalse(thread.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_continue_single_thread(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_continue_resp = Mock()
        mock_continue_resp.body = Mock()
        mock_continue_resp.body.all_threads_continued = False
        mock_continue_resp.dump_dap.return_value = {
            "allThreadsContinued": False
        }
        mock_dap_client.continue_thread = AsyncMock(
            return_value=mock_continue_resp
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "continue", ContinueRequest(thread_id=1, single_thread=True)
        )

        self.assertTrue(resp.success)
        mock_dap_client.continue_thread.assert_called_once()
        self.assertFalse(thread1.is_stopped)
        self.assertTrue(thread2.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    def test_update_resumed_threads_single_thread_none_raises(self) -> None:
        daemon = Daemon(port=15678)
        with self.assertRaises(ValueError) as ctx:
            daemon.update_resumed_threads(None, single_thread=True)
        self.assertIn(
            "Single thread resumed without providing thread_id!",
            str(ctx.exception),
        )

    def test_process_resume(self) -> None:
        daemon = Daemon(port=15678)
        proc = daemon.get_or_create_process(1234)
        proc.async_backtrace = [AsyncTaskNode(name="foo")]
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        self.assertTrue(proc.all_threads_stopped)

        proc.resume()
        self.assertFalse(thread1.is_stopped)
        self.assertFalse(thread2.is_stopped)
        self.assertFalse(proc.all_threads_stopped)
        self.assertIsNone(proc.async_backtrace)

    def test_thread_resume_clears_process_cache(self) -> None:
        daemon = Daemon(port=15678)
        proc = daemon.get_or_create_process(1234)
        proc.async_backtrace = [AsyncTaskNode(name="foo")]
        thread = daemon.get_or_create_thread(1, process_id=1234)
        thread.is_stopped = True

        thread.resume()
        self.assertFalse(thread.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    def test_thread_resume_no_clear_cache(self) -> None:
        daemon = Daemon(port=15678)
        proc = daemon.get_or_create_process(1234)
        proc.async_backtrace = [AsyncTaskNode(name="foo")]
        thread = daemon.get_or_create_thread(1, process_id=1234)
        thread.is_stopped = True

        thread._resume_internal(clear_process_cache=False)
        self.assertFalse(thread.is_stopped)
        self.assertIsNotNone(proc.async_backtrace)

    def test_thread_resume_without_process(self) -> None:
        daemon = Daemon(port=15678)
        thread = daemon.get_or_create_thread(1)
        thread.is_stopped = True

        thread.resume()
        self.assertFalse(thread.is_stopped)

    def test_process_clear_cache(self) -> None:
        daemon = Daemon(port=15678)
        proc = daemon.get_or_create_process(1234)
        proc.async_backtrace = [AsyncTaskNode(name="foo")]

        proc.clear_cache()
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_finish(self, mock_dap_client_class: Mock) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_finish_resp = Mock()
        mock_finish_resp.dump_dap.return_value = {"success": True}
        mock_dap_client.step_out = AsyncMock(return_value=mock_finish_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "finish",
            FinishRequest(command="finish", thread_id=1),
        )
        self.assertTrue(resp.success, resp.message)
        mock_dap_client.step_out.assert_called_once()
        args = mock_dap_client.step_out.call_args[0][0]
        self.assertEqual(args.thread_id, 1)
        self.assertTrue(args.single_thread)
        self.assertFalse(thread1.is_stopped)
        self.assertTrue(thread2.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_finish_all_threads(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_finish_resp = Mock()
        mock_finish_resp.dump_dap.return_value = {"success": True}
        mock_dap_client.step_out = AsyncMock(return_value=mock_finish_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "finish",
            FinishRequest(command="finish", thread_id=1, single_thread=False),
        )
        self.assertTrue(resp.success, resp.message)
        mock_dap_client.step_out.assert_called_once()
        args = mock_dap_client.step_out.call_args[0][0]
        self.assertEqual(args.thread_id, 1)
        self.assertFalse(args.single_thread)
        self.assertFalse(thread1.is_stopped)
        self.assertFalse(thread2.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_next(self, mock_dap_client_class: Mock) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_next_resp = Mock()
        mock_next_resp.success = True
        mock_next_resp.dump_dap.return_value = {"success": True}
        mock_dap_client.next = AsyncMock(return_value=mock_next_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "next",
            NextRequest(
                command="next",
                thread_id=1,
                granularity="line",
            ),
        )
        self.assertTrue(resp.success, resp.message)
        mock_dap_client.next.assert_called_once()
        args = mock_dap_client.next.call_args[0][0]
        self.assertEqual(args.thread_id, 1)
        self.assertTrue(args.single_thread)
        self.assertFalse(thread1.is_stopped)
        self.assertTrue(thread2.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_next_all_threads(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_next_resp = Mock()
        mock_next_resp.success = True
        mock_next_resp.dump_dap.return_value = {"success": True}
        mock_dap_client.next = AsyncMock(return_value=mock_next_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "next",
            NextRequest(
                command="next",
                thread_id=1,
                single_thread=False,
            ),
        )
        self.assertTrue(resp.success, resp.message)
        mock_dap_client.next.assert_called_once()
        args = mock_dap_client.next.call_args[0][0]
        self.assertEqual(args.thread_id, 1)
        self.assertFalse(args.single_thread)
        self.assertFalse(thread1.is_stopped)
        self.assertFalse(thread2.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_next_dap_error(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_client.next = AsyncMock(
            side_effect=DapError("Thread not stopped")
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        resp = await daemon.registry.handle(
            "next", NextRequest(command="next", thread_id=1)
        )
        self.assertFalse(resp.success)
        self.assertIn("Thread not stopped", resp.message or "")

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_step_in(self, mock_dap_client_class: Mock) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_response = Mock()
        mock_dap_response.success = True
        mock_dap_response.dump_dap.return_value = {"success": True}
        mock_dap_client.step_in = AsyncMock(return_value=mock_dap_response)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "step-in",
            StepInRequest(command="step-in", thread_id=1),
        )

        self.assertTrue(resp.success)
        mock_dap_client.step_in.assert_called_once()
        args = mock_dap_client.step_in.call_args[0][0]
        self.assertEqual(args.thread_id, 1)
        self.assertTrue(args.single_thread)
        self.assertFalse(thread1.is_stopped)
        self.assertTrue(thread2.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_step_in_all_threads(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_response = Mock()
        mock_dap_response.success = True
        mock_dap_response.dump_dap.return_value = {"success": True}
        mock_dap_client.step_in = AsyncMock(return_value=mock_dap_response)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread1 = daemon.get_or_create_thread(1, process_id=1234)
        thread2 = daemon.get_or_create_thread(2, process_id=1234)
        thread1.is_stopped = True
        thread2.is_stopped = True
        proc = daemon.processes[1234]
        proc.async_backtrace = [AsyncTaskNode(name="task")]

        resp = await daemon.registry.handle(
            "step-in",
            StepInRequest(command="step-in", thread_id=1, single_thread=False),
        )

        self.assertTrue(resp.success)
        mock_dap_client.step_in.assert_called_once()
        args = mock_dap_client.step_in.call_args[0][0]
        self.assertEqual(args.thread_id, 1)
        self.assertFalse(args.single_thread)
        self.assertFalse(thread1.is_stopped)
        self.assertFalse(thread2.is_stopped)
        self.assertIsNone(proc.async_backtrace)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_step_in_dap_error(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_client.step_in = AsyncMock(
            side_effect=DapError("Thread not stopped")
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        resp = await daemon.registry.handle(
            "step-in", StepInRequest(command="step-in", thread_id=1)
        )

        self.assertFalse(resp.success)
        self.assertIn("Thread not stopped", resp.message or "")

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_ensure_process_stopped_already_stopped(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        daemon = Daemon(port=15678)
        daemon.get_or_create_process(12345)
        daemon.get_or_create_thread(1, process_id=12345).is_stopped = True

        await daemon.ensure_process_stopped(12345)
        mock_dap_client.zxdb_pause_process.assert_not_called()

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_ensure_process_stopped_failure(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_dap_client.zxdb_pause_process = AsyncMock(
            return_value=DapResponse(
                seq=1,
                type="response",
                request_seq=1,
                success=False,
                message="Process not found",
            )
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with self.assertRaises(Exception) as ctx:
            await daemon.ensure_process_stopped(12345)
        self.assertIn("Process not found", str(ctx.exception))

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_ensure_process_stopped_when_not_stopped(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        proc = daemon.get_or_create_process(12345)
        thread = daemon.get_or_create_thread(1, process_id=12345)
        self.assertFalse(proc.all_threads_stopped)

        async def mock_pause_side_effect(
            args: ZxdbPauseArguments,
        ) -> DapResponse:
            assert args.process_id is not None
            thread.is_stopped = True
            daemon.event_waiter.notify_process_stop(
                args.process_id,
                {
                    "type": "event",
                    "event": "processStopped",
                    "body": {"processId": args.process_id, "name": "p1"},
                },
            )
            return DapResponse(
                seq=1, type="response", request_seq=1, success=True
            )

        mock_dap_client.zxdb_pause_process = AsyncMock(
            side_effect=mock_pause_side_effect
        )

        await daemon.ensure_process_stopped(12345)
        mock_dap_client.zxdb_pause_process.assert_called_once_with(
            ZxdbPauseArguments(process_id=12345)
        )
        self.assertTrue(proc.all_threads_stopped)

    async def test_ensure_process_stopped_not_connected(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = None
        with self.assertRaises(Exception) as ctx:
            await daemon.ensure_process_stopped(12345)
        self.assertIn("Not connected to zxdb DAP server", str(ctx.exception))

    @patch("daemon.daemon.ZxdbDapClient")
    @patch("daemon.daemon.asyncio.wait_for", side_effect=asyncio.TimeoutError)
    async def test_ensure_process_stopped_timeout(
        self, mock_wait_for: Mock, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_dap_client.zxdb_pause_process = AsyncMock(
            return_value=DapResponse(
                seq=1, type="response", request_seq=1, success=True
            )
        )
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with self.assertRaises(Exception) as ctx:
            await daemon.ensure_process_stopped(12345)
        self.assertIn(
            "Timed out waiting for process 12345 to stop", str(ctx.exception)
        )

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_ensure_stopped_already_stopped(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        daemon = Daemon(port=15678)
        daemon.get_or_create_thread(1).is_stopped = True

        await daemon.ensure_stopped(1)
        mock_dap_client.pause_thread.assert_not_called()

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_ensure_stopped_when_not_stopped(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        thread = daemon.get_or_create_thread(1)
        self.assertFalse(thread.is_stopped)

        async def mock_pause_thread_side_effect(
            args: PauseArguments,
        ) -> dict[str, Any]:
            assert args.thread_id is not None
            thread.is_stopped = True
            daemon.event_waiter.notify_thread_stop(
                args.thread_id,
                {
                    "type": "event",
                    "event": "stopped",
                    "body": {"reason": "pause", "threadId": args.thread_id},
                },
            )
            return {"success": True}

        mock_dap_client.pause_thread = AsyncMock(
            side_effect=mock_pause_thread_side_effect
        )

        await daemon.ensure_stopped(1)
        mock_dap_client.pause_thread.assert_called_once_with(
            PauseArguments(threadId=1)
        )
        self.assertTrue(thread.is_stopped)

    async def test_ensure_stopped_not_connected(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = None
        with self.assertRaises(Exception) as ctx:
            await daemon.ensure_stopped(1)
        self.assertIn("Not connected to zxdb DAP server", str(ctx.exception))

    @patch("daemon.daemon.ZxdbDapClient")
    @patch("daemon.daemon.asyncio.wait_for", side_effect=asyncio.TimeoutError)
    async def test_ensure_stopped_timeout(
        self, mock_wait_for: Mock, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_client.pause_thread = AsyncMock(return_value={"success": True})
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with self.assertRaises(Exception) as ctx:
            await daemon.ensure_stopped(1)
        self.assertIn(
            "Timed out waiting for thread 1 to stop", str(ctx.exception)
        )

    def test_threads_registration(self) -> None:
        daemon = Daemon(port=15678)
        self.assertIn("threads", daemon.registry.handlers)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_threads(self, mock_dap_client_class: Mock) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        mock_threads_resp = Mock()
        mock_body = Mock()
        mock_thread1 = Mock()
        mock_thread1.id = 1
        mock_thread1.name = "main"
        mock_thread1.process_id = 1234
        mock_thread2 = Mock()
        mock_thread2.id = 2
        mock_thread2.name = "worker"
        mock_thread2.process_id = 1234
        mock_body.threads = [mock_thread1, mock_thread2]
        mock_body.model_dump.return_value = {
            "threads": [
                {"id": 1, "name": "main", "processId": 1234},
                {"id": 2, "name": "worker", "processId": 1234},
            ]
        }
        mock_threads_resp.body = mock_body
        mock_dap_client.zxdb_threads = AsyncMock(return_value=mock_threads_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        resp = await daemon.registry.handle("threads", ThreadsRequest())

        if not resp.success:
            print(f"Test failed with message: {resp.message}")
        self.assertTrue(resp.success)
        assert resp.body is not None

        threads = resp.body["threads"]
        self.assertEqual(len(threads), 2)
        self.assertEqual(threads[0]["id"], 1)
        self.assertEqual(threads[0]["name"], "main")
        self.assertEqual(threads[1]["id"], 2)
        self.assertEqual(threads[1]["name"], "worker")

        assert daemon.threads[1].process is not None
        assert daemon.threads[2].process is not None
        self.assertEqual(daemon.threads[1].process.id, 1234)
        self.assertEqual(daemon.threads[2].process.id, 1234)

    async def test_update_thread_cache_evicts_stale_threads(self) -> None:
        daemon = Daemon(port=15678)
        daemon.get_or_create_thread(1, process_id=1234)
        daemon.get_or_create_thread(2, process_id=1234)
        daemon.get_or_create_thread(3, process_id=5678)

        mock_thread1 = Mock()
        mock_thread1.id = 1
        mock_thread1.process_id = 1234

        daemon.update_thread_cache([mock_thread1])

        self.assertEqual(list(daemon.threads.keys()), [1])
        assert daemon.threads[1].process is not None
        self.assertEqual(daemon.threads[1].process.id, 1234)

    def test_thread_and_process_equality(self) -> None:
        p1 = Process(id=123, name="p1")
        t1 = Thread(id=1, name="t1", is_stopped=False, process=p1)
        p1.threads[1] = t1

        p2 = Process(id=123, name="p2")
        t2 = Thread(id=1, name="t2", is_stopped=True, process=p2)
        p2.threads[1] = t2

        # Verify comparisons succeed without recursion error and match on ID only.
        self.assertEqual(t1, t2)
        self.assertEqual(p1, p2)

        t3 = Thread(id=2, process=p1)
        p3 = Process(id=456)
        self.assertNotEqual(t1, t3)
        self.assertNotEqual(p1, p3)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_get_state(self, mock_dap_client_class: Mock) -> None:
        """Verifies handle_get_state successfully queries threads and returns
        GetStateResponse.
        """
        mock_dap_client = mock_dap_client_class.return_value
        mock_threads_resp = Mock()
        mock_body = Mock()
        mock_thread1 = Mock()
        mock_thread1.id = 1
        mock_thread1.name = "main"
        mock_thread1.process_id = 1234
        mock_body.threads = [mock_thread1]
        mock_threads_resp.body = mock_body
        mock_dap_client.zxdb_threads = AsyncMock(return_value=mock_threads_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1234, name="test_process")
        daemon.active_breakpoints = {"/path/to/file.rs": {24, 12}}

        resp = await daemon.registry.handle("get-state", GetStateRequest())

        self.assertTrue(resp.success)
        state_resp = resp.body
        assert isinstance(state_resp, GetStateResponse)
        self.assertEqual(len(state_resp.threads), 1)
        self.assertEqual(state_resp.threads[0].id, 1)
        self.assertEqual(state_resp.threads[0].name, "main")
        self.assertEqual(state_resp.processes, {1234: "test_process"})
        self.assertEqual(state_resp.breakpoints, {"/path/to/file.rs": [12, 24]})

        assert daemon.threads[1].process is not None
        self.assertEqual(daemon.threads[1].process.id, 1234)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_get_state_defensive(
        self, mock_dap_client_class: Mock
    ) -> None:
        """Verifies handle_get_state gracefully handles None threads response
        body.
        """
        mock_dap_client = mock_dap_client_class.return_value
        mock_threads_resp = Mock()
        mock_threads_resp.body = None  # Simulate missing DAP body
        mock_dap_client.zxdb_threads = AsyncMock(return_value=mock_threads_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1234, name="test_process")

        resp = await daemon.registry.handle("get-state", GetStateRequest())

        self.assertTrue(resp.success)
        state_resp = resp.body
        assert isinstance(state_resp, GetStateResponse)
        self.assertEqual(
            len(state_resp.threads), 0
        )  # Successfully defaulted to empty list without crashing
        self.assertEqual(state_resp.processes, {1234: "test_process"})

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_get_state_empty_threads_prunes_cache(
        self, mock_dap_client_class: Mock
    ) -> None:
        """Verifies handle_get_state prunes cached threads when DAP returns an empty list."""
        mock_dap_client = mock_dap_client_class.return_value
        mock_threads_resp = Mock()
        mock_body = Mock()
        mock_body.threads = []
        mock_threads_resp.body = mock_body
        mock_dap_client.zxdb_threads = AsyncMock(return_value=mock_threads_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_thread(1, process_id=1234)
        daemon.get_or_create_process(1234, name="test_process")

        resp = await daemon.registry.handle("get-state", GetStateRequest())

        self.assertTrue(resp.success)
        state_resp = resp.body
        assert isinstance(state_resp, GetStateResponse)
        self.assertEqual(len(state_resp.threads), 0)
        self.assertEqual(daemon.threads, {})

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_threads_empty_prunes_cache(
        self, mock_dap_client_class: Mock
    ) -> None:
        """Verifies handle(threads) prunes cached threads when DAP returns an empty list."""
        mock_dap_client = mock_dap_client_class.return_value
        mock_threads_resp = Mock()
        mock_body = Mock()
        mock_body.threads = []
        mock_body.model_dump.return_value = {"threads": []}
        mock_threads_resp.body = mock_body
        mock_dap_client.zxdb_threads = AsyncMock(return_value=mock_threads_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_thread(1, process_id=1234)

        resp = await daemon.registry.handle("threads", ThreadsRequest())

        self.assertTrue(resp.success)
        self.assertEqual(daemon.threads, {})

    @patch("daemon.daemon.asyncio.start_unix_server")
    @patch("daemon.daemon.ZxdbDapClient")
    async def test_run_cleanup_detach_on_existing_session(
        self, mock_dap_client_class: Mock, mock_start_unix_server: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        mock_dap_client.zxdb_detach = AsyncMock()
        mock_dap_client.close = AsyncMock()

        daemon = Daemon(port=15678)
        daemon.connect_to_existing = True
        daemon.zxdb_writer = Mock()  # Simulate active connection

        # Mock the unix server
        mock_server = AsyncMock()
        mock_server.close = Mock()  # close is synchronous
        mock_start_unix_server.return_value = mock_server

        # Start run() in a task
        run_task = asyncio.create_task(daemon.run())

        # Let it run and reach the wait
        await asyncio.sleep(0.05)

        # Trigger stop
        daemon.stop_event.set()

        # Wait for run to complete
        await run_task

        # Verify zxdb_detach was called with all=True
        mock_dap_client.zxdb_detach.assert_called_once()
        args, kwargs = mock_dap_client.zxdb_detach.call_args
        self.assertTrue(args[0].detach_all)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_variables_success(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        # Mock stack trace
        mock_stack_resp = Mock()
        mock_frame = Mock()
        mock_frame.id = 42
        mock_stack_resp.body.stack_frames = [mock_frame]
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        # Mock scopes
        mock_scopes_resp = Mock()
        mock_scope1 = Mock()
        mock_scope1.name = "Locals"
        mock_scope1.variables_reference = 100
        mock_scope2 = Mock()
        mock_scope2.name = "Arguments"
        mock_scope2.variables_reference = 101
        mock_scope3 = Mock()
        mock_scope3.name = "Globals"
        mock_scope3.variables_reference = 102
        mock_scopes_resp.body.scopes = [mock_scope1, mock_scope2, mock_scope3]
        mock_dap_client.scopes = AsyncMock(return_value=mock_scopes_resp)

        # Mock variables for Locals (100)
        mock_vars_resp1 = Mock()
        mock_var1 = Mock()
        mock_var1.name = "x"
        mock_var1.value = "1"
        mock_var1.type = "int"
        mock_vars_resp1.body.variables = [mock_var1]

        # Mock variables for Arguments (101)
        mock_vars_resp2 = Mock()
        mock_var2 = Mock()
        mock_var2.name = "y"
        mock_var2.value = "2"
        mock_var2.type = "int"
        mock_vars_resp2.body.variables = [mock_var2]

        # variables is called twice, once for 100, once for 101
        mock_dap_client.variables = AsyncMock(
            side_effect=[mock_vars_resp1, mock_vars_resp2]
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon, "ensure_stopped", new_callable=AsyncMock
        ) as mock_ensure_stopped:
            resp = await daemon.registry.handle(
                "variables", VariablesRequest(thread_id=1, frame_index=0)
            )

            self.assertTrue(resp.success)
            self.assertEqual(
                resp.body,
                {
                    "variables": [
                        {"name": "x", "value": "1", "type": "int"},
                        {"name": "y", "value": "2", "type": "int"},
                    ]
                },
            )
            mock_ensure_stopped.assert_called_once_with(1)
            mock_dap_client.stack_trace.assert_called_once()
            mock_dap_client.scopes.assert_called_once()
            self.assertEqual(mock_dap_client.variables.call_count, 2)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_variables_no_frames(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_stack_resp = Mock()
        mock_stack_resp.body.stack_frames = []
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon, "ensure_stopped", new_callable=AsyncMock
        ) as mock_ensure_stopped:
            resp = await daemon.registry.handle(
                "variables", VariablesRequest(thread_id=1, frame_index=0)
            )

            self.assertFalse(resp.success)
            self.assertIn("No stack frames found", resp.message or "")

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_variables_frame_out_of_bounds(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_stack_resp = Mock()
        mock_frame = Mock()
        mock_stack_resp.body.stack_frames = [mock_frame]
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon, "ensure_stopped", new_callable=AsyncMock
        ) as mock_ensure_stopped:
            resp = await daemon.registry.handle(
                "variables", VariablesRequest(thread_id=1, frame_index=5)
            )

            self.assertFalse(resp.success)
            self.assertIn("Frame index 5 out of range", resp.message or "")

    async def test_handle_break_success(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon.dap_client, "set_breakpoints", new_callable=AsyncMock
        ) as mock_set_breakpoints:
            mock_resp = Mock()
            mock_resp.success = True
            mock_resp.body.dump_dap.return_value = {
                "breakpoints": [{"id": 1, "verified": True, "line": 12}]
            }
            mock_set_breakpoints.return_value = mock_resp

            req = BreakRequest(file="/path/to/file.rs", line=12)
            resp = await daemon.registry.handle("break", req)

            self.assertTrue(resp.success)
            self.assertEqual(
                resp.body,
                {"breakpoints": [{"id": 1, "verified": True, "line": 12}]},
            )
            self.assertEqual(
                daemon.active_breakpoints["/path/to/file.rs"], {12}
            )
            mock_set_breakpoints.assert_called_once()

    async def test_handle_break_additive(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.active_breakpoints["/path/to/file.rs"] = {12}

        with patch.object(
            daemon.dap_client, "set_breakpoints", new_callable=AsyncMock
        ) as mock_set_breakpoints:
            mock_resp = Mock()
            mock_resp.success = True
            mock_resp.body.dump_dap.return_value = {
                "breakpoints": [
                    {"id": 1, "verified": True, "line": 12},
                    {"id": 2, "verified": True, "line": 24},
                ]
            }
            mock_set_breakpoints.return_value = mock_resp

            req = BreakRequest(file="/path/to/file.rs", line=24)
            resp = await daemon.registry.handle("break", req)

            self.assertTrue(resp.success)
            self.assertEqual(
                daemon.active_breakpoints["/path/to/file.rs"], {12, 24}
            )
            mock_set_breakpoints.assert_called_once()
            args, _ = mock_set_breakpoints.call_args
            dap_args = args[0]
            self.assertEqual(len(dap_args.breakpoints), 2)
            self.assertEqual(dap_args.breakpoints[0].line, 12)
            self.assertEqual(dap_args.breakpoints[1].line, 24)

    async def test_handle_break_delete(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.active_breakpoints["/path/to/file.rs"] = {12, 24}

        with patch.object(
            daemon.dap_client, "set_breakpoints", new_callable=AsyncMock
        ) as mock_set_breakpoints:
            mock_resp = Mock()
            mock_resp.success = True
            mock_resp.body.dump_dap.return_value = {
                "breakpoints": [{"id": 1, "verified": True, "line": 24}]
            }
            mock_set_breakpoints.return_value = mock_resp

            req = BreakRequest(file="/path/to/file.rs", line=12, delete=True)
            resp = await daemon.registry.handle("break", req)

            self.assertTrue(resp.success)
            self.assertEqual(
                daemon.active_breakpoints["/path/to/file.rs"], {24}
            )
            mock_set_breakpoints.assert_called_once()
            args, _ = mock_set_breakpoints.call_args
            dap_args = args[0]
            self.assertEqual(len(dap_args.breakpoints), 1)
            self.assertEqual(dap_args.breakpoints[0].line, 24)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_evaluate_success_no_children(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        # Mock stack trace
        mock_stack_resp = Mock()
        mock_frame = Mock()
        mock_frame.id = 42
        mock_stack_resp.body.stack_frames = [mock_frame]
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        # Mock evaluate
        mock_eval_resp = Mock()
        mock_eval_resp.success = True
        mock_eval_resp.body.result = "10"
        mock_eval_resp.body.type = "int"
        mock_eval_resp.body.variables_reference = 0
        mock_dap_client.evaluate = AsyncMock(return_value=mock_eval_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_thread(1).is_stopped = True

        resp = await daemon.registry.handle(
            "evaluate",
            EvaluateRequest(thread_id=1, frame_index=0, expression="10"),
        )

        self.assertTrue(resp.success)
        self.assertIsNotNone(resp.body)
        assert isinstance(resp.body, EvaluateResponse)
        self.assertEqual(resp.body.result, "10")
        self.assertEqual(resp.body.type, "int")
        mock_dap_client.stack_trace.assert_called_once()
        mock_dap_client.evaluate.assert_called_once()

    # TODO(https://fxbug.dev/529329366): Add test_handle_evaluate_success_with_children
    # once variablesReference expansion is supported in EvaluateResponse.

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_evaluate_failure(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        # Mock stack trace
        mock_stack_resp = Mock()
        mock_stack_resp.body.stack_frames = []
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_thread(1).is_stopped = True

        resp = await daemon.registry.handle(
            "evaluate",
            EvaluateRequest(thread_id=1, frame_index=0, expression="10"),
        )

        self.assertFalse(resp.success)
        self.assertIn("No stack frames found", resp.message or "")

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_evaluate_thread_not_stopped(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_thread(1).is_stopped = False

        def on_pause(*args: Any, **kwargs: Any) -> dict[str, Any]:
            daemon.event_waiter.notify_thread_stop(
                1,
                {
                    "type": "event",
                    "event": "stopped",
                    "body": {"reason": "pause"},
                },
            )
            return {"success": True}

        mock_dap_client.pause_thread = AsyncMock(side_effect=on_pause)

        # Mock stack trace
        mock_stack_resp = Mock()
        mock_frame = Mock()
        mock_frame.id = 42
        mock_stack_resp.body.stack_frames = [mock_frame]
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        # Mock evaluate
        mock_eval_resp = Mock()
        mock_eval_resp.success = True
        mock_eval_resp.body.result = "10"
        mock_eval_resp.body.type = "int"
        mock_eval_resp.body.variables_reference = 0
        mock_dap_client.evaluate = AsyncMock(return_value=mock_eval_resp)

        resp = await daemon.registry.handle(
            "evaluate",
            EvaluateRequest(thread_id=1, frame_index=0, expression="10"),
        )

        self.assertTrue(resp.success)
        self.assertIsNotNone(resp.body)
        assert isinstance(resp.body, EvaluateResponse)
        self.assertEqual(resp.body.result, "10")
        self.assertEqual(resp.body.type, "int")
        mock_dap_client.pause_thread.assert_called_once()
        mock_dap_client.stack_trace.assert_called_once()
        mock_dap_client.evaluate.assert_called_once()

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_elides_subtle_frames(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_stack_resp = Mock()
        mock_stack_resp.body.stack_frames = [
            StackFrame(
                id=1,
                name="f1",
                line=1,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
            StackFrame(
                id=2,
                name="f2",
                line=2,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
        ]
        mock_stack_resp.body.total_frames = 2
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon, "ensure_stopped", new_callable=AsyncMock
        ) as mock_ensure_stopped:
            resp = await daemon.registry.handle(
                "stack-trace", StackTraceRequest(thread_id=1, raw=False)
            )

            self.assertTrue(resp.success)
            self.assertIsNotNone(resp.body)
            assert isinstance(resp.body, ThreadStackTraceResponse)
            self.assertEqual(resp.body.thread_id, 1)
            frames = resp.body.stack_frames
            self.assertEqual(len(frames), 1)
            self.assertIn("0…1 «Rust panic»", frames[0].name)
            self.assertEqual(frames[0].frame_index, 0)

            # Raw flag returns uncollapsing frames
            resp_raw = await daemon.registry.handle(
                "stack-trace", StackTraceRequest(thread_id=1, raw=True)
            )
            self.assertTrue(resp_raw.success)
            assert isinstance(resp_raw.body, ThreadStackTraceResponse)
            self.assertEqual(len(resp_raw.body.stack_frames), 2)
            self.assertEqual(resp_raw.body.stack_frames[0].frame_index, 0)
            self.assertEqual(resp_raw.body.stack_frames[1].frame_index, 1)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_subtle_non_subtle_subtle(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_stack_resp = Mock()
        mock_stack_resp.body.stack_frames = [
            StackFrame(
                id=1,
                name="f1",
                line=1,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
            StackFrame(
                id=2,
                name="f2",
                line=2,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
            StackFrame(
                id=3,
                name="f3",
                line=3,
                column=1,
                presentation_hint="normal",
                source=None,
            ),
            StackFrame(
                id=4,
                name="f4",
                line=4,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="C++ stdlib"),
            ),
            StackFrame(
                id=5,
                name="f5",
                line=5,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="C++ stdlib"),
            ),
        ]
        mock_stack_resp.body.total_frames = 5
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(daemon, "ensure_stopped", new_callable=AsyncMock):
            resp = await daemon.registry.handle(
                "stack-trace", StackTraceRequest(thread_id=1, raw=False)
            )

            self.assertTrue(resp.success)
            self.assertIsNotNone(resp.body)
            assert isinstance(resp.body, ThreadStackTraceResponse)
            frames = resp.body.stack_frames
            self.assertEqual(len(frames), 3)
            self.assertIn("0…1 «Rust panic»", frames[0].name)
            self.assertEqual(frames[0].frame_index, 0)
            self.assertEqual(frames[1].name, "f3")
            self.assertEqual(frames[1].frame_index, 2)
            self.assertIn("3…4 «C++ stdlib»", frames[2].name)
            self.assertEqual(frames[2].frame_index, 3)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_non_subtle_subtle_non_subtle(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_stack_resp = Mock()
        mock_stack_resp.body.stack_frames = [
            StackFrame(
                id=1,
                name="f1",
                line=1,
                column=1,
                presentation_hint="normal",
                source=None,
            ),
            StackFrame(
                id=2,
                name="f2",
                line=2,
                column=1,
                presentation_hint="normal",
                source=None,
            ),
            StackFrame(
                id=3,
                name="f3",
                line=3,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
            StackFrame(
                id=4,
                name="f4",
                line=4,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
            StackFrame(
                id=5,
                name="f5",
                line=5,
                column=1,
                presentation_hint="normal",
                source=None,
            ),
        ]
        mock_stack_resp.body.total_frames = 5
        mock_dap_client.stack_trace = AsyncMock(return_value=mock_stack_resp)

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(daemon, "ensure_stopped", new_callable=AsyncMock):
            resp = await daemon.registry.handle(
                "stack-trace", StackTraceRequest(thread_id=1, raw=False)
            )

            self.assertTrue(resp.success)
            self.assertIsNotNone(resp.body)
            assert isinstance(resp.body, ThreadStackTraceResponse)
            frames = resp.body.stack_frames
            self.assertEqual(len(frames), 4)
            self.assertEqual(frames[0].name, "f1")
            self.assertEqual(frames[0].frame_index, 0)
            self.assertEqual(frames[1].name, "f2")
            self.assertEqual(frames[1].frame_index, 1)
            self.assertIn("2…3 «Rust panic»", frames[2].name)
            self.assertEqual(frames[2].frame_index, 2)
            self.assertEqual(frames[3].name, "f5")
            self.assertEqual(frames[3].frame_index, 4)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_exception(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value
        # Mock _send_request returning invalid response structure that fails model_validate
        mock_dap_client._send_request = AsyncMock(
            return_value={"invalid": "response_structure"}
        )
        # Real stack_trace calls _send_request and model_validate
        mock_dap_client.stack_trace = lambda args: DapClient.stack_trace(
            mock_dap_client, args
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(daemon, "ensure_stopped", new_callable=AsyncMock):
            resp = await daemon.registry.handle(
                "stack-trace", StackTraceRequest(thread_id=1)
            )

            self.assertFalse(resp.success)
            self.assertIsNotNone(resp.message)
            self.assertIn("Failed to get stack trace", resp.message or "")

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_pid_success(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_stack_resp_1 = Mock()
        mock_stack_resp_1.body.stack_frames = [
            StackFrame(
                id=1,
                name="f1",
                line=1,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
            StackFrame(
                id=2,
                name="f2",
                line=2,
                column=1,
                presentation_hint="subtle",
                source=Source(origin="Rust panic"),
            ),
        ]
        mock_stack_resp_1.body.total_frames = 2

        mock_stack_resp_2 = Mock()
        mock_stack_resp_2.body.stack_frames = [
            StackFrame(
                id=3,
                name="worker_func",
                line=3,
                column=1,
                presentation_hint="normal",
                source=None,
            ),
        ]
        mock_stack_resp_2.body.total_frames = 1

        mock_dap_client.stack_trace = AsyncMock(
            side_effect=[mock_stack_resp_1, mock_stack_resp_2]
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(12345, name="test_proc")
        daemon.get_or_create_thread(
            101, name="initial-thread", process_id=12345
        ).is_stopped = True
        daemon.get_or_create_thread(
            102, name="worker-thread", process_id=12345
        ).is_stopped = True

        resp = await daemon.registry.handle(
            "stack-trace", StackTraceRequest(pid=12345, raw=False)
        )

        self.assertTrue(resp.success)
        self.assertIsNotNone(resp.body)

        assert isinstance(resp.body, ProcessStackTraceResponse)
        self.assertEqual(resp.body.process_id, 12345)
        threads = resp.body.stacks
        self.assertEqual(len(threads), 2)
        self.assertEqual(threads[0].thread_id, 101)
        self.assertEqual(len(threads[0].stack_frames), 1)
        self.assertIn("0…1 «Rust panic»", threads[0].stack_frames[0].name)
        self.assertEqual(threads[0].stack_frames[0].frame_index, 0)
        self.assertEqual(threads[1].thread_id, 102)
        self.assertEqual(len(threads[1].stack_frames), 1)
        self.assertEqual(threads[1].stack_frames[0].name, "worker_func")
        self.assertEqual(threads[1].stack_frames[0].frame_index, 0)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_pid_not_found(
        self, mock_dap_client_class: Mock
    ) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()

        with patch.object(
            daemon, "ensure_process_stopped", new_callable=AsyncMock
        ) as mock_ensure_stopped:
            resp = await daemon.registry.handle(
                "stack-trace", StackTraceRequest(pid=99999, raw=False)
            )
            self.assertFalse(resp.success)
            self.assertIn(
                "No threads found for process 99999", resp.message or ""
            )
            mock_ensure_stopped.assert_called_once_with(99999)

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_pid_no_threads(
        self, mock_dap_client_class: Mock
    ) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(12345, name="test_proc")

        with patch.object(
            daemon, "ensure_process_stopped", new_callable=AsyncMock
        ):
            resp = await daemon.registry.handle(
                "stack-trace", StackTraceRequest(pid=12345)
            )
            self.assertFalse(resp.success)
            self.assertIn(
                "No threads found for process 12345", resp.message or ""
            )

    @patch("daemon.daemon.ZxdbDapClient")
    async def test_handle_stack_trace_pid_partial_thread_failure(
        self, mock_dap_client_class: Mock
    ) -> None:
        mock_dap_client = mock_dap_client_class.return_value

        mock_stack_resp_1 = Mock()
        mock_stack_resp_1.body.stack_frames = [
            StackFrame(
                id=1,
                name="func_ok",
                line=1,
                column=1,
                presentation_hint="normal",
                source=None,
            ),
        ]
        mock_stack_resp_1.body.total_frames = 1

        mock_dap_client.stack_trace = AsyncMock(
            side_effect=[mock_stack_resp_1, Exception("Thread exited")]
        )

        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(12345, name="test_proc")
        daemon.get_or_create_thread(
            101, name="thread-ok", process_id=12345
        ).is_stopped = True
        daemon.get_or_create_thread(
            102, name="thread-exited", process_id=12345
        ).is_stopped = True

        resp = await daemon.registry.handle(
            "stack-trace", StackTraceRequest(pid=12345, raw=False)
        )

        self.assertTrue(resp.success)
        self.assertIsNotNone(resp.body)

        assert isinstance(resp.body, ProcessStackTraceResponse)
        self.assertEqual(len(resp.body.stacks), 2)
        self.assertEqual(resp.body.stacks[0].thread_id, 101)
        self.assertEqual(len(resp.body.stacks[0].stack_frames), 1)
        self.assertEqual(resp.body.stacks[1].thread_id, 102)
        self.assertEqual(len(resp.body.stacks[1].stack_frames), 0)
        self.assertEqual(resp.body.stacks[1].total_frames, 0)

    def test_async_backtrace_registration(self) -> None:
        daemon = Daemon(port=15678)
        self.assertIn("async-backtrace", daemon.registry.handlers)

    async def test_handle_async_backtrace_not_connected(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = None
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest()
        )
        self.assertFalse(resp.success)
        self.assertIn("Not connected to zxdb DAP server", resp.message or "")

    async def test_handle_async_backtrace_no_processes(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest()
        )
        self.assertFalse(resp.success)
        self.assertIn("No active processes found", resp.message or "")

    async def test_handle_async_backtrace_ambiguous_processes(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1001, name="proc1")
        daemon.get_or_create_process(1002, name="proc2")
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest()
        )
        self.assertFalse(resp.success)
        self.assertIn(
            "Multiple processes active; please specify a PID with --pid / -p",
            resp.message or "",
        )

    async def test_handle_async_backtrace_pid_not_found(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1001, name="proc1")
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest(pid=9999)
        )
        self.assertFalse(resp.success)
        self.assertIn("Process 9999 not found", resp.message or "")

    async def test_handle_async_backtrace_no_cache(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1001, name="proc1")
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest(pid=1001)
        )
        self.assertFalse(resp.success)
        self.assertIn(
            "No async backtrace cached for process 1001", resp.message or ""
        )

    async def test_handle_async_backtrace_success_explicit_pid(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        proc = daemon.get_or_create_process(1001, name="proc1")
        proc.async_backtrace = [
            AsyncTaskNode(
                id="t1",
                name="root_task",
                file="main.rs",
                line=42,
                children=[],
            )
        ]
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest(pid=1001)
        )
        self.assertTrue(resp.success)
        self.assertIsNotNone(resp.body)
        assert isinstance(resp.body, AsyncBacktraceResponse)
        self.assertEqual(resp.body.process_id, 1001)
        self.assertEqual(len(resp.body.tasks), 1)
        self.assertEqual(resp.body.tasks[0].id, "t1")
        self.assertEqual(resp.body.tasks[0].name, "root_task")

    async def test_handle_async_backtrace_success_default_single_process(
        self,
    ) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        proc = daemon.get_or_create_process(1001, name="proc1")
        proc.async_backtrace = []
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest()
        )
        self.assertTrue(resp.success)
        assert isinstance(resp.body, AsyncBacktraceResponse)
        self.assertEqual(resp.body.process_id, 1001)
        self.assertEqual(resp.body.tasks, [])

    async def test_handle_async_backtrace_nested_nodes(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        proc = daemon.get_or_create_process(1001, name="proc1")
        proc.async_backtrace = [
            AsyncTaskNode(
                id="t1",
                name="parent_task",
                file="foo.rs",
                line=10,
                children=[
                    AsyncTaskNode(
                        id="t2",
                        name="child_task",
                        file="bar.rs",
                        line=20,
                        children=[],
                    )
                ],
            )
        ]
        resp = await daemon.registry.handle(
            "async-backtrace", AsyncBacktraceRequest(pid=1001)
        )
        self.assertTrue(resp.success)
        assert isinstance(resp.body, AsyncBacktraceResponse)
        self.assertEqual(len(resp.body.tasks), 1)
        parent = resp.body.tasks[0]
        self.assertIsInstance(parent, AsyncTaskNode)
        self.assertEqual(parent.name, "parent_task")
        self.assertEqual(len(parent.children), 1)
        child = parent.children[0]
        self.assertIsInstance(child, AsyncTaskNode)
        self.assertEqual(child.name, "child_task")


if __name__ == "__main__":
    unittest.main()
