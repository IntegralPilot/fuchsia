#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the top-level `fx agents` CLI dispatcher."""

from __future__ import annotations

import argparse
import unittest
from unittest import mock

from agents import main
from agents.commands import setup
from agents_testing.base import BaseTestCase


class MainCliTest(BaseTestCase):
    """Test top-level argument parsing and subcommand dispatching."""

    def test_help_flag(self) -> None:
        parser = main.create_parser()
        with self.assertRaises(SystemExit) as cm:
            parser.parse_args(["--help"])
        self.assertEqual(cm.exception.code, 0)

    def test_no_subcommand_prints_help_and_fails(self) -> None:
        exit_code = main.main([])
        self.assertEqual(exit_code, 1)
        self.assertIn("fx agents", self.stderr)

    def test_subcommand_dispatch(self) -> None:
        parser = argparse.ArgumentParser(prog="fx agents")
        subparsers = parser.add_subparsers(dest="subcommand")
        mock_handler = mock.MagicMock(return_value=42)
        subparsers.add_parser("test").set_defaults(func=mock_handler)

        with mock.patch.object(main, "create_parser", return_value=parser):
            exit_code = main.main(["test"])
            self.assertEqual(exit_code, 42)
            mock_handler.assert_called_once()

    def test_setup_help_flag(self) -> None:
        parser = main.create_parser()
        with self.assertRaises(SystemExit) as cm:
            parser.parse_args(["setup", "--help"])
        self.assertEqual(cm.exception.code, 0)

    def test_setup_subcommand_dispatch(self) -> None:
        with mock.patch.object(setup, "run", return_value=0) as mock_run:
            exit_code = main.main(["setup", "-a", "grant1", "--dry-run"])
            self.assertEqual(exit_code, 0)
            mock_run.assert_called_once()
            args = mock_run.call_args[0][0]
            self.assertEqual(args.allow, ["grant1"])
            self.assertTrue(args.dry_run)


if __name__ == "__main__":
    unittest.main()
