# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

""" A basic read-eval-print loop to enable or disable test cases and annotate
them with bug info. Can print a list or filtered list; each test case is
labeled with a number NN.MM which is used for the bug and enable/disable
commands. """

import sys
from typing import Sequence, Tuple

import data_structure
import util


class CLI:
    """A command line interface satisfying the main.UI protocol."""

    def __init__(self) -> None:
        self.unsaved = False

    def inform(self, out: util.Outputs) -> None:
        """Show the user the given outputs."""

        for u in out.updates:
            print(f"    {u[:75]}")
        for p in out.prints:
            print(p)
        for e in out.errors:
            if e.startswith("ERROR: "):
                print(e)
            else:
                print(f"ERROR: {e}")

    def bail(self, failure_message: str) -> None:
        """Show a failure message and exit the program."""
        print(failure_message)
        sys.exit(1)

    def test_list(self, tests: data_structure.Tests) -> None:
        """Let the user interact with the test list."""

        self.tests = tests
        self._build_ui_structs()
        while True:
            self._read_eval_print()

    def _print_filtered_list(self, filter: str) -> None:
        print_whole_suite = False  # Set for every TestSuite
        for key, item in self.print_list:
            if isinstance(item, data_structure.TestSuite):
                print_whole_suite = filter in item.name
                if print_whole_suite:
                    print(f"{key}: {item.name}")
            else:  # it's a TestCase
                if print_whole_suite or filter in item.name:
                    if item.disabled:
                        dstr = "D"
                    else:
                        dstr = " "
                    print(f"{key}: {dstr} {item.name} {item.bug}")

    def _build_ui_structs(self) -> None:
        self.test_case_dict = {}
        self.print_list: Sequence[
            Tuple[str, data_structure.TestSuite]
            | Tuple[str, data_structure.TestCase]
        ] = []
        for i, suite in enumerate(self.tests.tests):
            self.print_list.append((str(i), suite))
            for j, case in enumerate(suite.cases):
                key = f"{i}.{j}"
                self.test_case_dict[key] = case
                self.print_list.append((key, case))

    def _set_disabled(self, args: str, value: bool) -> None:
        if args in self.test_case_dict:
            self.test_case_dict[args].disabled = value
        else:
            print(f"'{args}' is not a valid test case label.")

    def _set_bug(self, args: str) -> None:
        lb = args.split(maxsplit=1)
        label = lb[0]
        bug = lb[1:]
        if label in self.test_case_dict:
            if bug:
                bug_text = bug[0]
            else:
                bug_text = ""
            self.test_case_dict[label].bug = bug_text
        else:
            print(f"'{args}' is not a valid test case label")

    def _read_eval_print(self) -> None:
        """Reads a single input and responds appropriately."""
        i = input(
            "\n".join(
                [
                    "b(ug) <label> [<str>] |  d(isable)/e(nable) <label> |",
                    "f(ilter) <str> | l(ist) |",
                    "q(uit) | q! (q unsaved) | s(ave) | sq(SaveQuit): ",
                ]
            )
        )
        words = i.split(maxsplit=1)
        cmd = words[0]
        args = words[1:]  # Empty list or one string containing all args.
        if cmd not in "b d e f l q q! s sq".split():
            print("I couldn't understand that")
            return
        if cmd in "b d e".split():
            self.unsaved = True
        if cmd == "q":
            if self.unsaved:
                print("State not saved. Use q! to quit without saving.")
            else:
                sys.exit(0)
        if cmd == "q!":
            sys.exit()
        if cmd == "s":
            self.tests.save()
            self.unsaved = False
            print("Use 'sq' to save and quit")
        if cmd == "sq":
            self.tests.save()
            sys.exit()
        if cmd == "l":
            self._print_filtered_list("")

        # Above this line, we should have handled all commands without args.
        # Commands with arg
        if cmd not in "b d e f":
            return
        elif not args:
            print("Command needs arguments")
            return
        else:
            arg_string = args[0]
        if cmd == "d":
            self._set_disabled(arg_string, True)
        if cmd == "e":
            self._set_disabled(arg_string, False)
        if cmd == "b":
            self._set_bug(arg_string)
        if cmd == "f":
            self._print_filtered_list(arg_string)
