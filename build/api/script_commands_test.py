# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import sys
import unittest

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
from script_commands import ScriptCommandBase, ScriptCommandList


class ExitCodeCommand(ScriptCommandBase):
    """A command only used during unit-testing."""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--exit_code",
            type=int,
            default=0,
            help="Exit code for the test command, default is 0",
        )

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        return args.exit_code


class StatusCodeTestCommand(ScriptCommandBase):
    PARSER_KWARGS = {
        "name": "status_code",
        "help": "another command only used during unit-testing.",
    }

    def __init__(self, status_code: int) -> None:
        self._status_code = status_code

    # This is a regular, i.e. non-static, method.
    def run(self, args: argparse.Namespace) -> int:
        return self._status_code


class NoNameAndInvalidCommandSuffix(ScriptCommandBase):
    """Ignored help."""

    def run(self, args: argparse.Namespace) -> int:
        return 1


class NoHelpAndDocstringCommand(ScriptCommandBase):
    # This class must not have a docstring, and no "help" field in PARSER_KWARGS
    def run(self, args: argparse.Namespace) -> int:
        return 1


class SimpleDescriptionCommand(ScriptCommandBase):
    """Simple command"""

    DESCRIPTION = "Simple description"


class MultiLineDescriptionCommand(ScriptCommandBase):
    """Multi-line command"""

    DESCRIPTION = """A multi-line
description test
that can be reformatted.
"""


class RawDescriptionCommand(ScriptCommandBase):
    """Raw command"""

    DESCRIPTION_RAW = """
A raw help description
  that should not be reformatted

in any way
"""


class ScriptCommandListTest(unittest.TestCase):
    @staticmethod
    def init() -> tuple[argparse.ArgumentParser, ScriptCommandList]:
        parser = argparse.ArgumentParser(description="parser for tests")
        commands = ScriptCommandList(parser)
        return parser, commands

    def test_commands(self) -> None:
        parser, commands = self.init()
        commands.add_command(ExitCodeCommand())
        commands.add_command(StatusCodeTestCommand(31))

        args = parser.parse_args(args=["exit_code"])
        self.assertEqual(commands.run(args), 0)

        args = parser.parse_args(args=["exit_code", "--exit_code", "42"])
        self.assertEqual(commands.run(args), 42)

        args = parser.parse_args(args=["status_code"])
        self.assertEqual(commands.run(args), 31)

    def test_bad_command_class_name(self) -> None:
        parser, commands = self.init()

        with self.assertRaises(AssertionError) as cm:
            commands.add_command(NoNameAndInvalidCommandSuffix())

        self.assertEqual(
            str(cm.exception),
            "ScriptCommandBase derived class name (NoNameAndInvalidCommandSuffix) "
            + 'does not end with Command suffix. Please ensure its PARSER_KWARGS value provides a "name" value.',
        )

    def test_missing_docstring_in_command_class(self) -> None:
        parser, commands = self.init()

        with self.assertRaises(AssertionError) as cm:
            commands.add_command(NoHelpAndDocstringCommand())

        self.assertEqual(
            str(cm.exception),
            "ScriptCommandBase derived class (NoHelpAndDocstringCommand) "
            + 'has no docstring. Please ensure its PARSER_KWARGS value provides a "help" value.',
        )

    def test_description_in_command_class(self) -> None:
        parser, commands = self.init()
        commands.add_command(SimpleDescriptionCommand())
        desc_command = commands.parsers[0]
        self.assertEqual(
            desc_command.format_help(),
            """usage: script_commands_test.py simple_description [-h]

Simple description

options:
  -h, --help  show this help message and exit
""",
        )

    def test_multiline_description_in_command_class(self) -> None:
        parser, commands = self.init()
        commands.add_command(MultiLineDescriptionCommand())
        desc_command = commands.parsers[0]
        self.assertEqual(
            desc_command.format_help(),
            """usage: script_commands_test.py multi_line_description [-h]

A multi-line description test that can be reformatted.

options:
  -h, --help  show this help message and exit
""",
        )

    def test_raw_description_in_command_class(self) -> None:
        parser, commands = self.init()
        commands.add_command(RawDescriptionCommand())
        desc_command = commands.parsers[0]
        self.assertEqual(
            desc_command.format_help(),
            """usage: script_commands_test.py raw_description [-h]

A raw help description
  that should not be reformatted

in any way

options:
  -h, --help  show this help message and exit
""",
        )


if __name__ == "__main__":
    unittest.main()
