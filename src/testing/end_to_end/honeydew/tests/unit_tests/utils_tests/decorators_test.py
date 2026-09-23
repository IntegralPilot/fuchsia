# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.utils.decorators."""

from __future__ import annotations

import asyncio
import unittest

from honeydew.utils import decorators


class DummyClass:
    """Dummy class for testing method formatting."""

    def dummy_method(self, arg1: str, arg2: int = 42) -> str:
        return f"{arg1}_{arg2}"


class DecoratorsTest(unittest.TestCase):
    """Unit tests for decorators.py."""

    def test_safe_repr_default_and_custom_limit(self) -> None:
        short_val = "hello"
        self.assertEqual(decorators._safe_repr(short_val), "'hello'")

        long_val = "a" * 50
        # Default limit from _truncate is 40
        self.assertEqual(
            decorators._safe_repr(long_val), "'" + "a" * 36 + "..."
        )
        self.assertEqual(
            decorators._safe_repr(long_val, limit=None),
            "'" + "a" * 36 + "...",
        )
        # Custom limit
        self.assertEqual(
            decorators._safe_repr(long_val, limit=20),
            "'" + "a" * 16 + "...",
        )

    def test_format_operation_with_shell_cmd_kwargs(self) -> None:
        cmd = [
            "/path/to/host-tools/ffx",
            "--strict",
            "-t",
            "[fe80::1]:22",
            "-o",
            "/tmp/ffx.log",
            "-c",
            '{"log": {"dir": "/tmp"}}',
            "target",
            "screenshot",
            "--format",
            "png",
            "-d",
            "/tmp/out.png",
        ]
        result = decorators._format_operation(
            func=lambda: None,
            args=(),
            kwargs={"cmd": cmd, "capture_output": True},
        )
        self.assertEqual(
            result,
            "'ffx -t [fe80::1]:22 target screenshot --format png -d out.png'",
        )

    def test_format_operation_with_shell_cmd_positional_arg(self) -> None:
        cmd = ["adb", "-s", "localhost:1234", "shell", "cmd", "nfc", "status"]
        result = decorators._format_operation(
            func=lambda: None,
            args=(cmd,),
            kwargs={},
        )
        self.assertEqual(
            result,
            "'adb -s localhost:1234 shell cmd nfc status'",
        )

    def test_format_operation_with_class_method(self) -> None:
        obj = DummyClass()
        result = decorators._format_operation(
            func=obj.dummy_method,
            args=(obj, "hello"),
            kwargs={"arg2": 99},
        )
        self.assertEqual(
            result,
            "'DummyClass.dummy_method('hello', arg2=99)'",
        )

    def test_format_operation_truncation(self) -> None:
        long_arg = "a" * 50
        result = decorators._format_operation(
            func=lambda: None,
            args=(long_arg,),
            kwargs={"key": "b" * 50},
        )
        self.assertIn("'" + "a" * 36 + "...", result)
        self.assertIn("key='" + "b" * 36 + "...", result)
        self.assertIn("...", result)

        long_cmd = ["binary"] + ["arg"] * 40
        cmd_result = decorators._format_operation(
            func=lambda: None,
            args=(),
            kwargs={"cmd": long_cmd},
        )
        self.assertTrue(cmd_result.endswith("...'"))
        self.assertLessEqual(len(cmd_result), 102)

    def test_format_operation_with_non_ffx_c_flag(self) -> None:
        cmd = ["sh", "-c", "echo hello"]
        result = decorators._format_operation(
            func=lambda: None,
            args=(),
            kwargs={"cmd": cmd},
        )
        self.assertEqual(result, "'sh -c echo hello'")

    def test_format_operation_with_standalone_function_custom_object(
        self,
    ) -> None:
        def standalone_fn(obj: DummyClass, val: int) -> None:
            pass

        obj = DummyClass()
        result = decorators._format_operation(
            func=standalone_fn,
            args=(obj, 42),
            kwargs={},
        )
        self.assertIn("standalone_fn", result)
        self.assertIn("DummyClass", result)

    def test_format_operation_with_url_preserved(self) -> None:
        cmd = ["curl", "http://127.0.0.1:8080/status"]
        result = decorators._format_operation(
            func=lambda: None,
            args=(),
            kwargs={"cmd": cmd},
        )
        self.assertEqual(result, "'curl http://127.0.0.1:8080/status'")

    def test_format_operation_with_broken_repr(self) -> None:
        class BrokenRepr:
            def __repr__(self) -> str:
                raise ValueError("repr failed")

        obj = BrokenRepr()
        result = decorators._format_operation(
            func=lambda x: None,
            args=(obj,),
            kwargs={},
        )
        self.assertIn("<BrokenRepr>", result)

    def test_liveness_check_decorator_executes_function(self) -> None:
        @decorators.liveness_check
        def sample_function(x: int, y: int) -> int:
            return x + y

        result = sample_function(3, 4)
        self.assertEqual(result, 7)

    def test_async_liveness_check_decorator_executes_coroutine(self) -> None:
        @decorators.async_liveness_check
        async def async_sample_function(x: int) -> int:
            return x * 2

        result = asyncio.run(async_sample_function(5))
        self.assertEqual(result, 10)


if __name__ == "__main__":
    unittest.main()
