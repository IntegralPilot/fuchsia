# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import unittest

import selection_action


class TestSelectionAction(unittest.TestCase):
    def test_selection_action(self) -> None:
        """Test SelectionAction.

        This test ensures that multiple arguments can all write to the
        same destination variable. Short/long names for flags are
        canonicalized to the long version.
        """

        parser = argparse.ArgumentParser()
        parser.add_argument(
            "-p",
            "--package",
            action=selection_action.SelectionAction,
            nargs=0,
            dest="option",
        )
        parser.add_argument(
            "-c",
            "--component",
            action=selection_action.SelectionAction,
            nargs=0,
            dest="option",
        )
        parser.add_argument(
            "option", action=selection_action.SelectionAction, nargs="*"
        )

        args = parser.parse_intermixed_args(
            selection_action.SelectionAction.preprocess_args(
                ["-p", "one", "two", "-c", "three", "four"]
            )
        )
        self.assertListEqual(
            args.option,
            [
                "--package",
                "one",
                "two",
                "--component",
                "three",
                "four",
            ],
        )

    def test_selection_action_with_equals(self) -> None:
        """Test that --package=val and --component=val are parsed properly without crashing."""
        parser = argparse.ArgumentParser()
        parser.add_argument(
            "-p",
            "--package",
            action=selection_action.InvalidAction,
            dest="option",
        )
        parser.add_argument(
            "-c",
            "--component",
            action=selection_action.InvalidAction,
            dest="option",
        )
        parser.add_argument(
            "option", action=selection_action.SelectionAction, nargs="*"
        )

        args = parser.parse_intermixed_args(
            selection_action.SelectionAction.preprocess_args(
                ["--package=one", "--component=two"]
            )
        )
        self.assertListEqual(
            args.option,
            [
                "--package",
                "one",
                "--component",
                "two",
            ],
        )
