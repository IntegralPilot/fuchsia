# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Entry point of the program.

App ensures CTF tests are built, scans the available tests,
and then hands off control to a UI.
"""

import argparse
import os
import sys
import time
from typing import Protocol

import cli
import command_runner
import data_structure
import util


class UI(Protocol):
    """The program uses a UI to interact with the user."""

    def inform(self, out: util.Outputs) -> None:
        """Outputs contains text the user may want to see."""
        ...

    def test_list(self, tests: data_structure.Tests) -> None:
        """The UI should take over with the given tests."""
        ...

    def bail(self, failure_message: str) -> None:
        """Something failed. Tell the user, and exit."""
        ...


class App:
    def __init__(self, argv: list[str]) -> None:
        args = self._parse_args(argv)
        if args.gui:
            try:
                import gui

                self.ui: UI = gui.GUI()
            except ImportError:
                if os.path.exists(__file__):
                    script_path = __file__
                elif os.path.isfile((dirname := os.path.dirname(__file__))):
                    script_path = dirname
                else:
                    script_path = None
                print(
                    "ERROR: Failed to load GUI modules. Your Python installation may not have Tkinter installed.\n"
                )
                if script_path:
                    print(
                        "If you are using fx, you can try running this script using your installed Python version:\n"
                        f"$ python3 {script_path} --gui\n"
                    )
                else:
                    print("Try reinstalling Python.\n")
                sys.exit(1)
        else:
            self.ui = cli.CLI()
        self.build_target = "//sdk/ctf/release:tests"
        self.scan_target = "//sdk/ctf/release"
        if args.tiny_scan:
            print(
                "Developer shortcut mode: only scanning fdio-spawn-tests-package"
            )
            self.build_target = (
                "//sdk/ctf/tests/pkg/fdio:fdio-spawn-tests-package"
            )
            self.scan_target = self.build_target

    def _parse_args(self, argv: list[str]) -> argparse.Namespace:
        parser = argparse.ArgumentParser()
        parser.add_argument(
            "--gui",
            help="Use a GUI",
            action="store_true",
        )
        parser.add_argument(
            "--tiny-scan",
            help="Just scan a couple of tests, to save time when debugging",
            action="store_true",
        )
        return parser.parse_args(argv)

    def run(self) -> None:
        """Main entry point."""
        builder = command_runner.TargetBuilder(self.build_target)
        if self._run_command(builder) != util.Status.SUCCESS:
            self.ui.bail("Builder failed")
        scanner = command_runner.TestEnumerator(self.scan_target)
        if self._run_command(scanner) != util.Status.SUCCESS:
            self.ui.bail("Test scanner failed")
        merged_tests = data_structure.load_and_merge(scanner.tests)
        self.ui.test_list(merged_tests)

    def _run_command(
        self, command: command_runner.CommandRunner
    ) -> util.Status:
        while True:
            results = command.poll()
            self.ui.inform(results.outputs)
            if (
                results.status == util.Status.FAILURE
                or results.status == util.Status.SUCCESS
            ):
                return results.status
            time.sleep(0.1)


def main() -> int:
    app = App(sys.argv[1:])
    app.run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
