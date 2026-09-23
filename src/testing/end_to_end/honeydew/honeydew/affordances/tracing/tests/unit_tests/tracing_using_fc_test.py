# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.fuchsia_controller.tracing.py."""

import os
import tempfile
import types
import unittest
from collections.abc import Callable
from typing import Any, cast
from unittest import mock

import fidl_fuchsia_tracing as f_tracing
import fidl_fuchsia_tracing_controller as f_tracingcontroller
import fuchsia_controller_py as fc
from fidl import AsyncSocket
from honeydew import affordances_capable
from honeydew.affordances.tracing import tracing_using_fc
from honeydew.affordances.tracing.errors import TracingError, TracingStateError
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from parameterized import param, parameterized


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = cast(Any, testcase_func).__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_{test_label}"


def _initialize_tracing_fake(
    *,
    # pylint: disable-next=unused-argument
    controller: int,
    # pylint: disable-next=unused-argument
    config: f_tracingcontroller.TraceConfig,
    output: int,
) -> None:
    """Must have the same signature as TraceProvisioner.initialize_tracing()."""
    fc.Socket(output).close()


# pylint: disable=protected-access
class TracingFCTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.affordances.fuchsia_controller.tracing.py."""

    async def asyncSetUp(self) -> None:
        super().setUp()
        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice
        )
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController
        )
        self.fc_transport_obj.ctx = fc.Context()

        def channel_create(
            self: fc_transport.FuchsiaController,
        ) -> tuple[fc.Channel, fc.Channel]:
            return self.ctx.channel_create()

        self.fc_transport_obj.channel_create = types.MethodType(
            channel_create, self.fc_transport_obj
        )

        self.mock_provisioner_client = mock.create_autospec(
            f_tracingcontroller.ProvisionerClient, spec_set=True
        )
        self.enterContext(
            mock.patch.object(
                f_tracingcontroller,
                "ProvisionerClient",
                return_value=self.mock_provisioner_client,
            )
        )

        self.mock_session_client = mock.create_autospec(
            f_tracingcontroller.SessionClient, spec_set=True
        )
        self.mock_session_client.start_tracing = mock.AsyncMock()
        self.mock_session_client.stop_tracing = mock.AsyncMock(
            return_value=f_tracingcontroller.SessionStopTracingResult(
                response=f_tracingcontroller.StopResult(provider_stats=[])
            )
        )

        self.enterContext(
            mock.patch.object(
                f_tracingcontroller,
                "SessionClient",
                return_value=self.mock_session_client,
            )
        )

        self.tracing_obj = tracing_using_fc.TracingUsingFc(
            device_name="fuchsia-emulator",
            fuchsia_controller=self.fc_transport_obj,
            reboot_affordance=self.reboot_affordance_obj,
        )

    async def test_verify_supported(self) -> None:
        """Test if verify_supported works."""
        # TODO(http://b/409625325): Implement the test method logic

    @parameterized.expand(
        [
            (
                {
                    "label": "with_no_categories_and_no_buffer_size",
                },
            ),
            (
                {
                    "label": "with_categories_and_buffer_size",
                    "categories": ["category1", "category2"],
                    "buffer_size": 1024,
                },
            ),
            (
                {
                    "label": "when_session_already_initialized",
                    "session_initialized": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_initialize(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.initialize() method."""
        # Perform setup based on parameters.
        if parameterized_dict.get("session_initialized"):
            self.tracing_obj.initialize()

        # Check whether an `TracingStateError` exception is raised when
        # calling `initialize()` on a session that is already initialized.
        if parameterized_dict.get("session_initialized"):
            with self.assertRaises(TracingStateError):
                self.tracing_obj.initialize()
        else:
            self.tracing_obj.initialize(
                categories=parameterized_dict.get("categories"),
                buffer_size=parameterized_dict.get("buffer_size"),
            )
            self.mock_provisioner_client.initialize_tracing.assert_called()

    async def test_initialize_error(self) -> None:
        """Test for Tracing.initialize() when the FIDL transport raises an error.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        self.mock_provisioner_client.initialize_tracing.side_effect = (
            fc.FcTransportStatus(fc.FcTransportStatus.FC_ERR_INVALID_ARGS)
        )
        with self.assertRaises(TracingError):
            self.tracing_obj.initialize()

    @parameterized.expand(
        [
            (
                {
                    "label": "when_session_is_not_initialized",
                    "session_initialized": False,
                    "tracing_active": False,
                },
            ),
            (
                {
                    "label": "when_session_is_initialized",
                    "session_initialized": True,
                    "tracing_active": False,
                },
            ),
            (
                {
                    "label": "when_tracing_already_started",
                    "session_initialized": True,
                    "tracing_active": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_start(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.start() method."""
        # Perform setup based on parameters.
        if parameterized_dict.get("session_initialized"):
            self.tracing_obj.initialize()
        if parameterized_dict.get("tracing_active"):
            await self.tracing_obj.start()

        # Check whether an `TracingStateError` exception is raised when
        # state is not valid.
        if not parameterized_dict.get(
            "session_initialized"
        ) or parameterized_dict.get("tracing_active"):
            with self.assertRaises(TracingStateError):
                await self.tracing_obj.start()
        else:
            await self.tracing_obj.start()
            self.mock_session_client.start_tracing.assert_called()

    async def test_start_error(self) -> None:
        """Test for Tracing.start() when the FIDL transport raises an error.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        self.tracing_obj.initialize()

        self.mock_session_client.start_tracing.side_effect = (
            fc.FcTransportStatus(fc.FcTransportStatus.FC_ERR_INVALID_ARGS)
        )
        with self.assertRaises(TracingError):
            await self.tracing_obj.start()

    @parameterized.expand(
        [
            (
                {
                    "label": "when_session_is_not_initialized",
                    "session_initialized": False,
                    "tracing_active": False,
                },
            ),
            (
                {
                    "label": "when_session_is_initialized",
                    "session_initialized": True,
                    "tracing_active": False,
                },
            ),
            (
                {
                    "label": "when_tracing_already_started",
                    "session_initialized": True,
                    "tracing_active": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_stop(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.stop() method."""
        # Perform setup based on parameters.
        self.mock_provisioner_client.initialize_tracing.side_effect = (
            _initialize_tracing_fake
        )
        session_initialized = parameterized_dict.get("session_initialized")
        tracing_active = parameterized_dict.get("tracing_active")
        if session_initialized:
            self.tracing_obj.initialize()
        if tracing_active:
            await self.tracing_obj.start()

        # Check whether an `TracingStateError` exception is raised when
        # state is not valid.
        if not session_initialized or not tracing_active:
            with self.assertRaises(TracingStateError):
                await self.tracing_obj.stop()
            return

        try:
            await self.tracing_obj.stop()
            self.mock_session_client.stop_tracing.assert_called()
        finally:
            await self.tracing_obj._reset_state()

    async def test_stop_error(self) -> None:
        """Test for Tracing.stop() when the FIDL call raises an error.
        ZX_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        self.mock_provisioner_client.initialize_tracing.side_effect = (
            _initialize_tracing_fake
        )

        self.tracing_obj.initialize()
        await self.tracing_obj.start()

        try:
            self.mock_session_client.stop_tracing.side_effect = fc.ZxStatus(
                fc.ZxStatus.ZX_ERR_INVALID_ARGS
            )
            with self.assertRaises(TracingError):
                await self.tracing_obj.stop()
        finally:
            await self.tracing_obj._reset_state()

    @parameterized.expand(
        [
            (
                {
                    "label": "when_session_is_not_initialized",
                    "session_initialized": False,
                },
            ),
            (
                {
                    "label": "with_no_download",
                    "session_initialized": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_terminate(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.terminate() method."""
        # Perform setup based on parameters.
        if parameterized_dict.get("session_initialized"):
            self.tracing_obj.initialize()

        await self.tracing_obj.terminate()

        if parameterized_dict.get("session_initialized"):
            self.mock_provisioner_client.initialize_tracing.assert_called_once()

        self.assertFalse(self.tracing_obj.is_active())
        self.assertFalse(self.tracing_obj.is_session_initialized())

    async def test_terminate_error(self) -> None:
        """Test for Tracing.terminate() when an error occurs during wait."""
        self.tracing_obj.initialize()

        with mock.patch.object(
            self.mock_session_client,
            "close_cleanly",
            new_callable=mock.Mock,
        ) as mock_wait:
            mock_wait.side_effect = TracingError("test error")
            with self.assertLogs(level="WARNING") as cm:
                await self.tracing_obj.terminate()
            self.assertIn(
                "Could not cleanly wait for trace termination", cm.output[0]
            )
            self.assertIn("test error", cm.output[0])

        self.mock_provisioner_client.initialize_tracing.assert_called_once()
        self.assertFalse(self.tracing_obj.is_session_initialized())
        self.assertFalse(self.tracing_obj.is_active())

    @parameterized.expand(
        [
            (
                {
                    "label": "with_unset_record_dropped",
                    "dropped": None,
                    "assert_warning": False,
                },
            ),
            (
                {
                    "label": "with_record_dropped",
                    "dropped": 10,
                    "assert_warning": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_stop_with_warning(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.stop() method with Warning."""
        # Perform setup based on parameters.
        self.mock_provisioner_client.initialize_tracing.side_effect = (
            _initialize_tracing_fake
        )
        records_dropped = parameterized_dict.get("dropped")
        self.mock_session_client.stop_tracing.return_value = (
            f_tracingcontroller.SessionStopTracingResult(
                response=f_tracingcontroller.StopResult(
                    provider_stats=[
                        f_tracingcontroller.ProviderStats(
                            name="virtual-console.cm",
                            pid=4566,
                            buffering_mode=1,
                            buffer_wrapped_count=0,
                            records_dropped=records_dropped,
                            percentage_durable_buffer_used=0.0,
                            non_durable_bytes_written=16,
                        )
                    ]
                )
            )
        )

        self.tracing_obj.initialize()
        await self.tracing_obj.start()
        try:
            if parameterized_dict.get("assert_warning"):
                with self.assertLogs(level="WARNING") as lc:
                    await self.tracing_obj.stop()
                    self.mock_session_client.stop_tracing.assert_called()
                    self.assertIn(
                        f"{records_dropped} records were dropped for virtual-console.cm!",
                        lc.output[0],
                    )
            else:
                with self.assertNoLogs(level="WARNING") as lc:
                    await self.tracing_obj.stop()
        finally:
            await self.tracing_obj._reset_state()

    @parameterized.expand(
        [
            (
                {
                    "label": "without_session_initialized",
                    "session_initialized": False,
                },
            ),
            (
                {
                    "label": "with_tracing_download_default_file_name",
                    "return_value": "samp_trace_data",
                    "session_initialized": True,
                },
            ),
            (
                {
                    "label": "with_tracing_download_given_file_name",
                    "trace_file": "trace.fxt",
                    "return_value": "samp_trace_data",
                    "session_initialized": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(fc, "Context")
    @mock.patch.object(AsyncSocket, "read_all", new_callable=mock.AsyncMock)
    async def test_terminate_and_download(
        self,
        parameterized_dict: dict[str, Any],
        mock_async_socket_read_all: mock.AsyncMock,
        mock_fc_context: mock.Mock,
    ) -> None:
        """Test for Tracing.terminate_and_download() method."""
        # Mock out the tracing Socket.
        return_value: str = parameterized_dict.get("return_value", "")
        mock_async_socket_read_all.return_value = bytes(
            return_value, encoding="utf-8"
        )

        mock_fc_context.socket_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )

        mock_fc_context.channel_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )

        # Perform setup based on parameters.
        if parameterized_dict.get("session_initialized"):
            self.tracing_obj.initialize()
            await self.tracing_obj.start()

        with tempfile.TemporaryDirectory() as tmpdir:
            if not parameterized_dict.get("session_initialized"):
                with self.assertRaises(TracingStateError):
                    await self.tracing_obj.terminate_and_download(
                        directory=tmpdir
                    )
                return

            trace_file: str = parameterized_dict.get("trace_file", "")
            trace_path: str = await self.tracing_obj.terminate_and_download(
                directory=tmpdir, trace_file=trace_file
            )
            self.assertFalse(self.tracing_obj.is_active())

            # Check the return value of the terminate method.
            if trace_file:
                self.assertEqual(trace_path, f"{tmpdir}/{trace_file}")
            else:
                self.assertRegex(trace_path, f"{tmpdir}/trace_.*.fxt")

            # Check the contents of the file.
            with open(trace_path, "r", encoding="utf-8") as file:
                data: str = file.read()
                self.assertEqual(data, return_value)

    @parameterized.expand(
        [
            (
                {
                    "label": "when_session_is_not_initialized",
                    "session_initialized": False,
                },
            ),
            (
                {
                    "label": "when_session_is_initialized",
                    "session_initialized": True,
                },
            ),
            (
                {
                    "label": "with_tracing_download",
                    "download_trace": True,
                    "trace_file": "trace.fxt",
                    "return_value": "samp_trace_data",
                    "session_initialized": False,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(fc, "Context")
    @mock.patch.object(AsyncSocket, "read_all", new_callable=mock.AsyncMock)
    async def test_trace_session(
        self,
        parameterized_dict: dict[str, Any],
        mock_async_socket_read_all: mock.AsyncMock,
        mock_fc_context: mock.Mock,
    ) -> None:
        """Test for Tracing.trace_session() method."""
        # Mock out the tracing Socket.
        return_value: str = parameterized_dict.get("return_value", "")
        mock_async_socket_read_all.return_value = bytes(
            return_value, encoding="utf-8"
        )
        mock_fc_context.socket_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )
        mock_fc_context.channel_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            trace_file: str = parameterized_dict.get("trace_file", "")
            download_trace: bool = parameterized_dict.get(
                "download_trace", False
            )

            if parameterized_dict.get("session_initialized"):
                self.tracing_obj.initialize()
            async with self.tracing_obj.trace_session(
                download=download_trace, directory=tmpdir, trace_file=trace_file
            ):
                self.assertTrue(self.tracing_obj.is_active())
            self.assertFalse(self.tracing_obj.is_active())

            if download_trace:
                trace_path: str = os.path.join(tmpdir, trace_file)
                self.assertTrue(os.path.exists(trace_path))

                # Check the contents of the file.
                with open(trace_path, "r", encoding="utf-8") as file:
                    data: str = file.read()
                    self.assertEqual(data, return_value)

    @mock.patch.object(fc, "Context")
    @mock.patch.object(AsyncSocket, "read_all", new_callable=mock.AsyncMock)
    async def test_trace_session_with_args(
        self,
        mock_async_socket_read_all: mock.AsyncMock,
        mock_fc_context: mock.Mock,
    ) -> None:
        """Test for Tracing.trace_session() method forwarding arguments."""
        mock_fc_context.socket_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )
        mock_fc_context.channel_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )

        self.assertFalse(self.tracing_obj.is_session_initialized())

        with self.assertLogs(level="WARNING") as log_context:
            async with self.tracing_obj.trace_session(
                categories=["category1"],
                buffer_size=1024,
                start_timeout_milliseconds=1000,
                buffering_mode=f_tracing.BufferingMode.ONESHOT,
                defer_transfer=True,
                compression=True,
            ):
                self.assertTrue(self.tracing_obj.is_session_initialized())
                self.assertTrue(self.tracing_obj.is_active())

        self.assertFalse(self.tracing_obj.is_active())

        # Verify FIDL config received all forwarded options
        self.mock_provisioner_client.initialize_tracing.assert_called_once()
        _, kwargs = self.mock_provisioner_client.initialize_tracing.call_args
        config = kwargs["config"]
        self.assertEqual(config.categories, ["category1"])
        self.assertEqual(config.buffer_size_megabytes_hint, 1024)
        self.assertEqual(config.start_timeout_milliseconds, 1000)
        self.assertEqual(config.buffering_mode, f_tracing.BufferingMode.ONESHOT)
        self.assertTrue(config.defer_transfer)

        # Verify fallback warning was emitted for compression
        self.assertTrue(
            any(
                "Compression is not supported in TracingUsingFc" in line
                for line in log_context.output
            )
        )

    @mock.patch.object(fc, "Context")
    @mock.patch.object(AsyncSocket, "read_all", new_callable=mock.AsyncMock)
    async def test_trace_session_cleanup_on_exception(
        self,
        mock_async_socket_read_all: mock.AsyncMock,
        mock_fc_context: mock.Mock,
    ) -> None:
        """Verify that trace_session cleans up even if an exception is raised."""
        mock_fc_context.socket_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )
        mock_fc_context.channel_create.return_value = (
            mock.MagicMock(),
            mock.MagicMock(),
        )

        with self.assertRaises(RuntimeError):
            async with self.tracing_obj.trace_session():
                raise RuntimeError("Simulated test failure")

        self.assertFalse(self.tracing_obj.is_active())


if __name__ == "__main__":
    unittest.main()
