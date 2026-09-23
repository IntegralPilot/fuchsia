# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import pathlib
import tempfile
import typing
import unittest
import unittest.mock as mock

import fx_cmd
from async_utils.command import (
    CommandEvent,
    CommandOutput,
    StderrEvent,
    StdoutEvent,
)

real_subprocess_exec = asyncio.create_subprocess_exec


class TestFxCmd(unittest.IsolatedAsyncioTestCase):
    @mock.patch.dict(
        "os.environ", {"FUCHSIA_BUILD_DIR_FROM_FX": "/tmp/fuchsia/global"}
    )
    def test_command_line(self) -> None:
        """command line respects build directory specification"""
        cmd = fx_cmd.FxCmd()
        self.assertEqual(
            cmd.command_line("format-code"),
            ["fx", "--dir", "/tmp/fuchsia/global", "format-code"],
        )
        cmd = fx_cmd.FxCmd(build_directory=pathlib.Path("/fuchsia"))
        self.assertEqual(
            cmd.command_line("format-code"),
            ["fx", "--dir", "/fuchsia", "format-code"],
        )

    # This mock captures the --dir argument to an `fx` command and lists
    # that directory rather than running the original command.
    @mock.patch(
        "asyncio.subprocess.create_subprocess_exec",
        mock.Mock(
            side_effect=lambda *args, **kwargs: real_subprocess_exec(
                "ls", args[2], **kwargs
            ),
        ),
    )
    async def test_sync_stdout(self) -> None:
        """sync execution returns stdout"""
        with tempfile.TemporaryDirectory() as td:
            path = pathlib.Path(td)
            f1 = path / "foo.txt"
            f2 = path / "bar.txt"
            f1.touch()
            f2.touch()

            lines: list[str] = []

            def callback(x: StdoutEvent) -> None:
                lines.append(x.text.decode(errors="ignore").strip())

            result = await fx_cmd.FxCmd(build_directory=path).sync(
                "build", stdout_callback=callback
            )
            self.assertEqual(result.return_code, 0)

            self.assertTrue(
                any(["bar.txt" in l for l in lines]),
                f"Missing bar.txt in {lines}",
            )
            self.assertTrue(
                any(["foo.txt" in l for l in lines]),
                f"Missing foo.txt in {lines}",
            )

    @mock.patch(
        "asyncio.subprocess.create_subprocess_exec",
        mock.Mock(
            side_effect=lambda *args, **kwargs: real_subprocess_exec(
                "ls", "/tmp/does-not-exist12345", **kwargs
            ),
        ),
    )
    async def test_sync_stderr(self) -> None:
        """sync execution returns stderr"""
        lines: list[str] = []

        def callback(x: StderrEvent) -> None:
            lines.append(x.text.decode(errors="ignore").strip())

        result = await fx_cmd.FxCmd(
            build_directory=pathlib.Path("/missing")
        ).sync("build", stderr_callback=callback)
        self.assertNotEqual(result.return_code, 0)

        self.assertNotEqual(lines, [])


class TestCommandTransformers(unittest.IsolatedAsyncioTestCase):
    class OutputCountTransformer(fx_cmd.CommandTransformer[str, int]):
        """Simple transformer that counts the number of lines in a test execution."""

        def _handle_event(
            self, event: CommandEvent, callback: typing.Callable[[str], None]
        ) -> None:
            if isinstance(event, StdoutEvent):
                callback(event.text.decode(errors="ignore").strip())

        def _to_output(self, output: CommandOutput) -> int:
            return output.stdout.count("\n")

    @mock.patch(
        "asyncio.subprocess.create_subprocess_exec",
        mock.Mock(
            side_effect=lambda *args, **kwargs: real_subprocess_exec(
                "ls", args[2], **kwargs
            ),
        ),
    )
    async def test_counting_transformer(self) -> None:
        """A basic transformer works correctly"""
        with tempfile.TemporaryDirectory() as td:
            path = pathlib.Path(td)
            f1 = path / "foo.txt"
            f2 = path / "bar.txt"
            f1.touch()
            f2.touch()

            lines: list[str] = []

            result = await self.OutputCountTransformer(
                "ignored", inner=fx_cmd.FxCmd(build_directory=path)
            ).sync(event_callback=lambda x: lines.append(x))
            self.assertEqual(result, 2)

            self.assertSetEqual(set(lines), {"bar.txt", "foo.txt"})

            # Test that we get an error reading a directory that doesn't exist.
            with self.assertRaises(fx_cmd.CommandFailed):
                await self.OutputCountTransformer(
                    "ignored",
                    inner=fx_cmd.FxCmd(build_directory=path / "does-not-exist"),
                ).sync()

    class FailingOutputTransformer(fx_cmd.CommandTransformer[None, int]):
        def _to_output(self, output: CommandOutput) -> int:
            raise RuntimeError("failed to convert")

    class FailingEventTransformer(fx_cmd.CommandTransformer[str, int]):
        def _handle_event(
            self,
            event: CommandEvent,
            callback: typing.Callable[[str], None],
        ) -> None:
            raise RuntimeError("failed to handle event")

        def _to_output(self, output: CommandOutput) -> int:
            return 0

    @mock.patch(
        "asyncio.subprocess.create_subprocess_exec",
        mock.Mock(
            side_effect=lambda *_args, **kwargs: real_subprocess_exec(
                "echo", "hello world\n", **kwargs
            ),
        ),
    )
    async def test_transformer_errors(self) -> None:
        """Exceptions in the transformer are caught"""
        with self.assertRaises(fx_cmd.CommandTransformFailed):
            await self.FailingOutputTransformer(
                "ignored", inner=fx_cmd.FxCmd(build_directory="/fuchsia")
            ).sync()

        with self.assertRaises(fx_cmd.CommandTransformFailed):
            await self.FailingEventTransformer(
                "ignored", inner=fx_cmd.FxCmd(build_directory="/fuchsia")
            ).sync()

    @mock.patch(
        "asyncio.subprocess.create_subprocess_exec",
        mock.Mock(
            side_effect=lambda *_args, **kwargs: real_subprocess_exec(
                "sleep", "10", **kwargs
            ),
        ),
    )
    async def test_transformer_timeout(self) -> None:
        """Timeouts are caught in the transformer"""
        with self.assertRaises(fx_cmd.CommandTimeout):
            await self.OutputCountTransformer(
                "ignored",
                inner=fx_cmd.FxCmd(build_directory="/fuchsia", timeout=0.1),
            ).sync()

    class FalsyEventTransformer(fx_cmd.CommandTransformer[str, int]):
        def _handle_event(
            self, event: CommandEvent, callback: typing.Callable[[str], None]
        ) -> None:
            if isinstance(event, StdoutEvent):
                callback("")
                callback("after_falsy")

        def _to_output(self, output: CommandOutput) -> int:
            return 0

    @mock.patch(
        "asyncio.subprocess.create_subprocess_exec",
        mock.Mock(
            side_effect=lambda *_args, **kwargs: real_subprocess_exec(
                "echo", "hello", **kwargs
            ),
        ),
    )
    async def test_falsy_event_does_not_terminate_drain(self) -> None:
        """Falsy events (e.g. empty strings) do not terminate the event drain prematurely."""
        received: list[str] = []
        await self.FalsyEventTransformer(
            "ignored",
            inner=fx_cmd.FxCmd(build_directory=pathlib.Path("/fuchsia")),
        ).sync(event_callback=lambda x: received.append(x))
        self.assertEqual(received, ["", "after_falsy"])
