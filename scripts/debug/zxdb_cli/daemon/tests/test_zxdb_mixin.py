# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from unittest.mock import AsyncMock, Mock, patch

from daemon.daemon import Daemon
from shared.protocol.detach import DetachRequest
from zxdb_dap import ZxdbDapMixin, ZxdbDetachArguments


class TestZxdbDapMixin(unittest.IsolatedAsyncioTestCase):
    async def test_zxdb_detach(self) -> None:
        class MockClient(ZxdbDapMixin):
            def __init__(self) -> None:
                self._send_request = AsyncMock()

        client = MockClient()
        args = ZxdbDetachArguments(pid=1234)

        await client.zxdb_detach(args)

        client._send_request.assert_called_once_with("zxdb.Detach", args)


class TestDaemonDetach(unittest.IsolatedAsyncioTestCase):
    async def test_handle_detach_all(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1234, name="proc1")
        daemon.get_or_create_process(5678, name="proc2")

        with patch.object(
            daemon.dap_client, "zxdb_detach", new_callable=AsyncMock
        ) as mock_detach:
            mock_detach.return_value = {"success": True}

            req = DetachRequest(all=True)
            resp = await daemon.registry.handle("detach", req)

            self.assertTrue(resp.success)
            mock_detach.assert_called_once()
            self.assertIsNotNone(mock_detach.call_args)
            assert mock_detach.call_args is not None
            args = mock_detach.call_args[0][0]
            self.assertTrue(args.detach_all)

            # Verify state side-effects
            self.assertEqual(daemon.processes, {})

            # Verify synthesized event
            self.assertEqual(daemon.event_queue.qsize(), 1)
            event = daemon.event_queue.get_nowait()
            self.assertEqual(event["event"], "detached")
            self.assertEqual(event["body"]["pid"], None)
            self.assertTrue(event["body"]["all"])

    async def test_handle_detach_pid(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1234, name="proc1")
        daemon.get_or_create_process(5678, name="proc2")

        with patch.object(
            daemon.dap_client, "zxdb_detach", new_callable=AsyncMock
        ) as mock_detach:
            mock_detach.return_value = {"success": True}

            req = DetachRequest(pid=1234)
            resp = await daemon.registry.handle("detach", req)

            self.assertTrue(resp.success)
            mock_detach.assert_called_once()
            self.assertIsNotNone(mock_detach.call_args)
            assert mock_detach.call_args is not None
            args = mock_detach.call_args[0][0]
            self.assertEqual(args.pid, 1234)

            # Verify state side-effects
            self.assertEqual(list(daemon.processes.keys()), [5678])
            self.assertEqual(daemon.processes[5678].name, "proc2")

            # Verify synthesized event
            self.assertEqual(daemon.event_queue.qsize(), 1)
            event = daemon.event_queue.get_nowait()
            self.assertEqual(event["event"], "detached")
            self.assertEqual(event["body"]["pid"], 1234)
            self.assertFalse(event["body"]["all"])

    async def test_handle_detach_not_connected(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = None

        req = DetachRequest(pid=1234)
        resp = await daemon.registry.handle("detach", req)

        self.assertFalse(resp.success)
        self.assertIn("Not connected", resp.message or "")

    async def test_handle_detach_dap_failure(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1234, name="proc1")

        with patch.object(
            daemon.dap_client, "zxdb_detach", new_callable=AsyncMock
        ) as mock_detach:
            mock_detach.return_value = {
                "success": False,
                "message": "Process not found",
            }

            req = DetachRequest(pid=1234)
            resp = await daemon.registry.handle("detach", req)

            self.assertFalse(resp.success)
            self.assertIn("Process not found", resp.message or "")
            mock_detach.assert_called_once()

            # Verify state NOT modified
            self.assertEqual(list(daemon.processes.keys()), [1234])
            self.assertEqual(daemon.processes[1234].name, "proc1")

            # Verify NO event synthesized
            self.assertEqual(daemon.event_queue.qsize(), 0)

    async def test_handle_detach_dap_exception(self) -> None:
        daemon = Daemon(port=15678)
        daemon.zxdb_writer = Mock()
        daemon.get_or_create_process(1234, name="proc1")

        with patch.object(
            daemon.dap_client, "zxdb_detach", new_callable=AsyncMock
        ) as mock_detach:
            mock_detach.side_effect = Exception("Connection lost")

            req = DetachRequest(pid=1234)
            resp = await daemon.registry.handle("detach", req)

            self.assertFalse(resp.success)
            self.assertIn("Connection lost", resp.message or "")
            mock_detach.assert_called_once()

            # Verify state NOT modified
            self.assertEqual(list(daemon.processes.keys()), [1234])
            self.assertEqual(daemon.processes[1234].name, "proc1")

            # Verify NO event synthesized
            self.assertEqual(daemon.event_queue.qsize(), 0)


if __name__ == "__main__":
    unittest.main()
