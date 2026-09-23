# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from shell_utils import ParsedShellCommand, ShellCommand, find_command_with_tool


class TestShellCommand(unittest.TestCase):
    def test_split_simple(self) -> None:
        cmd = ShellCommand("echo a && echo b")
        parts = cmd.split()
        self.assertEqual(
            parts, [ShellCommand("echo a"), ShellCommand("echo b")]
        )

    def test_split_quoted_operator(self) -> None:
        cmd = ShellCommand("echo '&&' || echo b")
        parts = cmd.split()
        self.assertEqual(
            parts, [ShellCommand("echo '&&'"), ShellCommand("echo b")]
        )

    def test_split_chained(self) -> None:
        cmd = ShellCommand("a && b || c && d")
        parts = cmd.split()
        self.assertEqual(
            parts,
            [
                ShellCommand("a"),
                ShellCommand("b"),
                ShellCommand("c"),
                ShellCommand("d"),
            ],
        )

    def test_split_custom_separators(self) -> None:
        cmd = ShellCommand("a ; b ; c")
        parts = cmd.split(separators={";"})
        self.assertEqual(
            parts, [ShellCommand("a"), ShellCommand("b"), ShellCommand("c")]
        )

    def test_unwrap_simple(self) -> None:
        cmd = ShellCommand("wrapper -- inner_cmd arg")
        unwrapped = cmd.unwrap()
        self.assertEqual(unwrapped, ShellCommand("inner_cmd arg"))

    def test_unwrap_nested(self) -> None:
        cmd = ShellCommand("w1 -- w2 -- inner")
        unwrapped = cmd.unwrap()
        self.assertEqual(unwrapped, ShellCommand("w2 -- inner"))

    def test_unwrap_with_flags(self) -> None:
        cmd = ShellCommand("wrapper --opt -- inner --flag")
        unwrapped = cmd.unwrap()
        self.assertEqual(unwrapped, ShellCommand("inner --flag"))

    def test_unwrap_quotes(self) -> None:
        cmd = ShellCommand("wrapper -- 'inner --foo'")
        unwrapped = cmd.unwrap()
        self.assertEqual(unwrapped, ShellCommand("'inner --foo'"))

    def test_unwrap_empty_after_separator(self) -> None:
        cmd = ShellCommand("wrapper --")
        unwrapped = cmd.unwrap()
        self.assertEqual(unwrapped, ShellCommand(""))

    def test_unwrap_no_separator(self) -> None:
        cmd = ShellCommand("echo hello")
        self.assertIsNone(cmd.unwrap())

    def test_parse_simple(self) -> None:
        cmd = ShellCommand("cmd arg")
        parsed = cmd.parse()
        self.assertEqual(parsed, ParsedShellCommand({}, "cmd", ["arg"]))

    def test_parse_with_env(self) -> None:
        cmd = ShellCommand("VAR=1 cmd")
        parsed = cmd.parse()
        self.assertEqual(parsed, ParsedShellCommand({"VAR": "1"}, "cmd", []))

    def test_parse_complex(self) -> None:
        cmd = ShellCommand("VAR=1 ../path/to/cmd --flag")
        parsed = cmd.parse()
        self.assertEqual(
            parsed,
            ParsedShellCommand({"VAR": "1"}, "../path/to/cmd", ["--flag"]),
        )

    def test_parse_quoted_env(self) -> None:
        cmd = ShellCommand('VAR="a b" cmd')
        parsed = cmd.parse()
        self.assertEqual(parsed, ParsedShellCommand({"VAR": "a b"}, "cmd", []))

    def test_parse_env_only(self) -> None:
        cmd = ShellCommand("A=B C=D")
        parsed = cmd.parse()
        self.assertEqual(
            parsed, ParsedShellCommand({"A": "B", "C": "D"}, "", [])
        )

    def test_parse_python_simple(self) -> None:
        cmd = ShellCommand("python3 script.py arg")
        parsed = cmd.parse()
        self.assertEqual(parsed, ParsedShellCommand({}, "script.py", ["arg"]))

    def test_fuchsia_vendored_python(self) -> None:
        cmd = ShellCommand("fuchsia-vendored-python script.py arg")
        parsed = cmd.parse()
        self.assertEqual(parsed, ParsedShellCommand({}, "script.py", ["arg"]))

    def test_parse_python_with_flags(self) -> None:
        cmd = ShellCommand("python3 -u script.py arg")
        parsed = cmd.parse()
        self.assertEqual(parsed, ParsedShellCommand({}, "script.py", ["arg"]))

    def test_parse_python_absolute_path(self) -> None:
        cmd = ShellCommand("/usr/bin/python3 script.py")
        parsed = cmd.parse()
        self.assertEqual(parsed, ParsedShellCommand({}, "script.py", []))

    def test_parse_python_no_script(self) -> None:
        # Fallback to python as tool if no .py script found
        cmd = ShellCommand("python3 -c code")
        parsed = cmd.parse()
        self.assertEqual(
            parsed, ParsedShellCommand({}, "python3", ["-c", "code"])
        )


class TestFindCommandWithTool(unittest.TestCase):
    def test_find_simple(self) -> None:
        cmd = ShellCommand("my_tool arg")
        found = find_command_with_tool([cmd], "my_tool")
        self.assertEqual(found, cmd)

    def test_find_wrapped(self) -> None:
        cmd = ShellCommand("wrapper -- my_tool arg")
        found = find_command_with_tool([cmd], "my_tool")
        self.assertEqual(found, ShellCommand("my_tool arg"))

    def test_find_in_list(self) -> None:
        cmd1 = ShellCommand("other_tool arg")
        cmd2 = ShellCommand("my_tool arg")
        found = find_command_with_tool([cmd1, cmd2], "my_tool")
        self.assertEqual(found, cmd2)

    def test_find_wrapped_in_list(self) -> None:
        cmd1 = ShellCommand("other_tool arg")
        cmd2 = ShellCommand("wrapper -- my_tool arg")
        found = find_command_with_tool([cmd1, cmd2], "my_tool")
        self.assertEqual(found, ShellCommand("my_tool arg"))

    def test_not_found(self) -> None:
        cmd = ShellCommand("other_tool arg")
        self.assertIsNone(find_command_with_tool([cmd], "my_tool"))

    def test_python_script(self) -> None:
        cmd = ShellCommand("python3 script.py arg")
        found = find_command_with_tool([cmd], "script.py")
        self.assertEqual(found, cmd)

    def test_python_script_wrapped(self) -> None:
        cmd = ShellCommand("wrapper -- python3 script.py arg")
        found = find_command_with_tool([cmd], "script.py")
        self.assertEqual(found, ShellCommand("python3 script.py arg"))

    def test_find_multi_wrapped(self) -> None:
        cmd = ShellCommand("wrapper1 -- wrapper2 -- my_tool arg")
        found = find_command_with_tool([cmd], "my_tool")
        self.assertEqual(found, ShellCommand("my_tool arg"))


if __name__ == "__main__":
    unittest.main()
