# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.tracing.tracing_using_ffx.py."""

import os
import subprocess
import tempfile
import unittest
from collections.abc import Callable
from typing import Any, cast
from unittest import mock

import fidl_fuchsia_tracing as f_tracing
from honeydew import affordances_capable
from honeydew.affordances.tracing import tracing_using_ffx
from honeydew.affordances.tracing.errors import TracingError, TracingStateError
from honeydew.transports.ffx import ffx
from honeydew.transports.ffx.errors import FfxCommandError
from parameterized import param, parameterized


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = cast(Any, testcase_func).__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_{test_label}"


# pylint: disable=protected-access
class TracingFfxTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.affordances.tracing.tracing_using_ffx.py."""

    async def asyncSetUp(self) -> None:
        super().setUp()
        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice
        )
        self.ffx_obj = mock.MagicMock(spec=ffx.FFX)

        self.tracing_obj = tracing_using_ffx.TracingUsingFfx(
            device_name="fuchsia-emulator",
            ffx_inst=self.ffx_obj,
            reboot_affordance=self.reboot_affordance_obj,
        )

    async def test_verify_supported(self) -> None:
        """Test if verify_supported works."""
        self.tracing_obj.verify_supported()

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
            self.assertTrue(self.tracing_obj.is_session_initialized())
            self.assertTrue(self.tracing_obj._compression)

    async def test_initialize_with_compression_disabled(self) -> None:
        """Test for Tracing.initialize() with compression=False."""
        self.tracing_obj.initialize(compression=False)
        self.assertTrue(self.tracing_obj.is_session_initialized())
        self.assertFalse(self.tracing_obj._compression)

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
            self.ffx_obj.run.assert_called_with(
                [
                    "trace",
                    "start",
                    "--background",
                    "--categories",
                    ",".join(tracing_using_ffx.DEFAULT_CATEGORIES),
                ]
            )
            self.assertTrue(self.tracing_obj.is_active())

    async def test_start_with_args(self) -> None:
        """Test for Tracing.start() to verify FFX arguments."""
        self.tracing_obj.initialize(
            categories=["category1"],
            buffer_size=1024,
            buffering_mode=f_tracing.BufferingMode.ONESHOT,
        )
        await self.tracing_obj.start()
        self.ffx_obj.run.assert_called_with(
            [
                "trace",
                "start",
                "--background",
                "--categories",
                "category1",
                "--buffer-size",
                "1024",
                "--buffering-mode",
                "oneshot",
            ]
        )

    async def test_start_with_compression_disabled(self) -> None:
        """Test for Tracing.start() when compression is disabled."""
        self.tracing_obj.initialize(
            categories=["category1"],
            buffer_size=1024,
            buffering_mode=f_tracing.BufferingMode.ONESHOT,
            compression=False,
        )
        await self.tracing_obj.start()
        self.ffx_obj.run.assert_called_with(
            [
                "trace",
                "start",
                "--background",
                "--categories",
                "category1",
                "--buffer-size",
                "1024",
                "--buffering-mode",
                "oneshot",
                "--nocompress",
            ]
        )

    async def test_start_error_ffx_command(self) -> None:
        """Test for Tracing.start() when FFX raises FfxCommandError."""
        self.tracing_obj.initialize()
        self.ffx_obj.run.side_effect = FfxCommandError("ffx error")
        with self.assertRaises(TracingError):
            await self.tracing_obj.start()

    async def test_start_error_timeout(self) -> None:
        """Test for Tracing.start() when FFX raises subprocess.TimeoutExpired."""
        self.tracing_obj.initialize()
        self.ffx_obj.run.side_effect = subprocess.TimeoutExpired("ffx", 1)
        with self.assertRaises(subprocess.TimeoutExpired):
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

        await self.tracing_obj.stop()

        self.assertIsNotNone(self.tracing_obj._temp_trace_file)
        self.ffx_obj.run.assert_called_with(
            ["trace", "stop", "--output", self.tracing_obj._temp_trace_file]
        )
        self.assertFalse(self.tracing_obj.is_active())

        # Cleanup
        if self.tracing_obj._temp_trace_file and os.path.exists(
            self.tracing_obj._temp_trace_file
        ):
            os.remove(self.tracing_obj._temp_trace_file)

    async def test_stop_error_ffx_command(self) -> None:
        """Test for Tracing.stop() when FFX raises FfxCommandError."""
        self.tracing_obj.initialize()
        await self.tracing_obj.start()

        self.ffx_obj.run.side_effect = FfxCommandError("ffx error")
        with self.assertRaises(TracingError):
            await self.tracing_obj.stop()

        # Cleanup
        if self.tracing_obj._temp_trace_file and os.path.exists(
            self.tracing_obj._temp_trace_file
        ):
            os.remove(self.tracing_obj._temp_trace_file)

    async def test_stop_error_timeout(self) -> None:
        """Test for Tracing.stop() when FFX raises subprocess.TimeoutExpired."""
        self.tracing_obj.initialize()
        await self.tracing_obj.start()

        self.ffx_obj.run.side_effect = subprocess.TimeoutExpired("ffx", 1)
        with self.assertRaises(subprocess.TimeoutExpired):
            await self.tracing_obj.stop()

        # Cleanup
        if self.tracing_obj._temp_trace_file and os.path.exists(
            self.tracing_obj._temp_trace_file
        ):
            os.remove(self.tracing_obj._temp_trace_file)

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
        ],
        name_func=_custom_test_name_func,
    )
    async def test_terminate(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.terminate() method."""
        temp_file_path = None
        # Perform setup based on parameters.
        if parameterized_dict.get("session_initialized"):
            self.tracing_obj.initialize()
            fd, temp_file_path = tempfile.mkstemp(suffix=".fxt")
            os.close(fd)
            self.tracing_obj._temp_trace_file = temp_file_path

        await self.tracing_obj.terminate()

        if temp_file_path:
            self.assertFalse(os.path.exists(temp_file_path))

        self.assertFalse(self.tracing_obj.is_active())
        self.assertFalse(self.tracing_obj.is_session_initialized())

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
    async def test_terminate_and_download(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.terminate_and_download() method."""
        # Perform setup based on parameters.
        if parameterized_dict.get("session_initialized"):
            self.tracing_obj.initialize()
            await self.tracing_obj.start()

        with tempfile.TemporaryDirectory() as tmpdir:
            if not parameterized_dict.get("session_initialized"):
                with self.assertRaises(TracingStateError):
                    await self.tracing_obj.terminate_and_download(tmpdir)
                return

            trace_file: str = parameterized_dict.get("trace_file", "")
            trace_path: str = await self.tracing_obj.terminate_and_download(
                tmpdir, trace_file
            )
            self.assertFalse(self.tracing_obj.is_active())

            # Check the return value of the terminate method.
            if trace_file:
                self.assertEqual(trace_path, f"{tmpdir}/{trace_file}")
            else:
                self.assertRegex(trace_path, f"{tmpdir}/trace_.*.fxt")

            self.assertTrue(os.path.exists(trace_path))

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
                    "session_initialized": False,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_trace_session(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test for Tracing.trace_session() method."""
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
                self.assertTrue(self.tracing_obj._compression)
            self.assertFalse(self.tracing_obj.is_active())

            if download_trace:
                trace_path: str = os.path.join(tmpdir, trace_file)
                self.assertTrue(os.path.exists(trace_path))

    async def test_trace_session_with_args(self) -> None:
        """Test for Tracing.trace_session() forwarding initialization arguments."""
        self.assertFalse(self.tracing_obj.is_session_initialized())

        with tempfile.TemporaryDirectory() as tmpdir:
            async with self.tracing_obj.trace_session(
                categories=["category1"],
                buffer_size=1024,
                buffering_mode=f_tracing.BufferingMode.ONESHOT,
                compression=True,
                download=True,
                directory=tmpdir,
                trace_file="trace.fxt",
            ):
                self.assertTrue(self.tracing_obj.is_session_initialized())
                self.assertEqual(self.tracing_obj._categories, ["category1"])
                self.assertEqual(self.tracing_obj._buffer_size, 1024)
                self.assertEqual(
                    self.tracing_obj._buffering_mode,
                    f_tracing.BufferingMode.ONESHOT,
                )
                self.assertTrue(self.tracing_obj._compression)
                self.assertTrue(self.tracing_obj.is_active())

            self.assertFalse(self.tracing_obj.is_active())
            self.ffx_obj.run.assert_any_call(
                [
                    "trace",
                    "start",
                    "--background",
                    "--categories",
                    "category1",
                    "--buffer-size",
                    "1024",
                    "--buffering-mode",
                    "oneshot",
                ]
            )
            trace_path: str = os.path.join(tmpdir, "trace.fxt")
            self.assertTrue(os.path.exists(trace_path))

    async def test_trace_session_with_compression_disabled(self) -> None:
        """Test for Tracing.trace_session() when compression is disabled."""
        self.assertFalse(self.tracing_obj.is_session_initialized())

        with tempfile.TemporaryDirectory() as tmpdir:
            async with self.tracing_obj.trace_session(
                categories=["category1"],
                buffer_size=1024,
                buffering_mode=f_tracing.BufferingMode.ONESHOT,
                compression=False,
                download=True,
                directory=tmpdir,
                trace_file="trace.fxt",
            ):
                self.assertTrue(self.tracing_obj.is_session_initialized())
                self.assertEqual(self.tracing_obj._categories, ["category1"])
                self.assertEqual(self.tracing_obj._buffer_size, 1024)
                self.assertEqual(
                    self.tracing_obj._buffering_mode,
                    f_tracing.BufferingMode.ONESHOT,
                )
                self.assertFalse(self.tracing_obj._compression)
                self.assertTrue(self.tracing_obj.is_active())

            self.assertFalse(self.tracing_obj.is_active())
            self.ffx_obj.run.assert_any_call(
                [
                    "trace",
                    "start",
                    "--background",
                    "--categories",
                    "category1",
                    "--buffer-size",
                    "1024",
                    "--buffering-mode",
                    "oneshot",
                    "--nocompress",
                ]
            )
            trace_path: str = os.path.join(tmpdir, "trace.fxt")
            self.assertTrue(os.path.exists(trace_path))

    async def test_trace_session_cleanup_on_exception(self) -> None:
        """Verify that trace_session cleans up even if an exception is raised."""
        with self.assertRaises(RuntimeError):
            async with self.tracing_obj.trace_session():
                raise RuntimeError("Simulated test failure")

        self.assertFalse(self.tracing_obj.is_active())


if __name__ == "__main__":
    unittest.main()
