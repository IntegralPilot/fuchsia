#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(__file__))
import bazel_compdb_utils


class BazelCompdbUtilsTests(unittest.TestCase):
    """Tests for bazel_compdb_utils."""

    def test_extract_file_from_args(self) -> None:
        self.assertEqual(
            bazel_compdb_utils.extract_file_from_args(
                ["gcc", "-c", "foo.cc", "-o", "foo.o"]
            ),
            "foo.cc",
        )
        self.assertEqual(
            bazel_compdb_utils.extract_file_from_args(["clang", "bar.cpp"]),
            "bar.cpp",
        )
        self.assertEqual(
            bazel_compdb_utils.extract_file_from_args(["clang", "baz.c"]),
            "baz.c",
        )
        with self.assertRaises(ValueError):
            bazel_compdb_utils.extract_file_from_args(
                ["gcc", "foo.cc", "bar.cc"]
            )
        with self.assertRaises(ValueError):
            bazel_compdb_utils.extract_file_from_args(["gcc", "foo.h"])
        with self.assertRaises(ValueError):
            bazel_compdb_utils.extract_file_from_args([])

    def test_action_init(self) -> None:
        action_dict = {
            "targetId": 1,
            "actionKey": "key",
            "arguments": ["gcc", "foo.cc"],
            "environmentVariables": {},
        }
        target_dict = {"label": "//foo:bar"}
        action = bazel_compdb_utils.Action(action_dict, target_dict)
        self.assertEqual(action.label, "//foo:bar")
        self.assertEqual(action.target_id, 1)
        self.assertEqual(action.action_key, "key")
        self.assertEqual(action.arguments, ["gcc", "foo.cc"])
        self.assertEqual(action.environment_vars, {})
        self.assertEqual(action.file, "foo.cc")

    def test_action_is_external(self) -> None:
        action_dict = {
            "targetId": 1,
            "actionKey": "key",
            "arguments": ["gcc", "foo.cc"],
            "environmentVariables": {},
        }
        internal_target = {"label": "//foo:bar"}
        external_target = {"label": "@ext//foo:bar"}
        internal_action = bazel_compdb_utils.Action(
            action_dict, internal_target
        )
        external_action = bazel_compdb_utils.Action(
            action_dict, external_target
        )
        self.assertFalse(internal_action.is_external())
        self.assertTrue(external_action.is_external())

    def test_compdb_formatter_init(self) -> None:
        formatter = bazel_compdb_utils.CompDBFormatter(
            "/build_dir", "/output_base", "/output_path"
        )
        self.assertEqual(formatter.build_dir, "/build_dir")
        self.assertEqual(formatter.output_base, "/output_base")
        self.assertEqual(formatter.output_path, "/output_path")
        self.assertEqual(formatter.output_base_rel, "../output_base")
        self.assertEqual(formatter.output_path_rel, "../output_path")

    def test_compdb_formatter_rewrite_file(self) -> None:
        formatter = bazel_compdb_utils.CompDBFormatter(
            "/build", "/build/out/base", "/build/out/path"
        )

        # External action
        action_dict_ext = {
            "targetId": 1,
            "actionKey": "key",
            "arguments": ["gcc", "external/file.cc"],
            "environmentVariables": {},
        }
        target_dict_ext = {"label": "@ext//:lib"}
        action_ext = bazel_compdb_utils.Action(action_dict_ext, target_dict_ext)
        self.assertEqual(
            formatter.rewrite_file(action_ext), "out/base/external/file.cc"
        )

        # Internal action
        action_dict_int = {
            "targetId": 2,
            "actionKey": "key",
            "arguments": ["gcc", "internal/file.cc"],
            "environmentVariables": {},
        }
        target_dict_int = {"label": "//:lib"}
        action_int = bazel_compdb_utils.Action(action_dict_int, target_dict_int)
        self.assertEqual(
            formatter.rewrite_file(action_int), "../../internal/file.cc"
        )

    def test_compdb_formatter_maybe_rewrite_path(self) -> None:
        formatter = bazel_compdb_utils.CompDBFormatter(
            "/build", "/build/out/base", "/build/out/path"
        )
        action_dict = {
            "targetId": 1,
            "actionKey": "key",
            "arguments": ["gcc", "action_file.cc"],
            "environmentVariables": {},
        }
        target_dict = {"label": "//:lib"}
        action = bazel_compdb_utils.Action(action_dict, target_dict)

        # Case 1: file_path == action.file
        with mock.patch.object(
            formatter, "rewrite_file", return_value="rewritten_action_file"
        ) as mock_rewrite_file:
            self.assertEqual(
                formatter.maybe_rewrite_path("action_file.cc", action),
                "rewritten_action_file",
            )
            mock_rewrite_file.assert_called_once_with(action)

        # Case 2: file_path == "."
        self.assertEqual(formatter.maybe_rewrite_path(".", action), "../../")

        # Case 3: file_path starts with sdk/, src/, vendor/, zircon/
        self.assertEqual(
            formatter.maybe_rewrite_path("sdk/some/path", action),
            "../../sdk/some/path",
        )
        self.assertEqual(
            formatter.maybe_rewrite_path("src/some/path", action),
            "../../src/some/path",
        )

        # Case 4: file_path contains "bazel-out/"
        self.assertEqual(
            formatter.maybe_rewrite_path("foo/bazel-out/bar", action),
            "foo/out/path/bar",
        )

        # Case 5: file_path contains "external/"
        self.assertEqual(
            formatter.maybe_rewrite_path("foo/external/bar", action),
            "foo/out/base/external/bar",
        )

        # Case 6: Default case
        self.assertEqual(
            formatter.maybe_rewrite_path("some/other/path", action),
            "some/other/path",
        )

    @mock.patch("bazel_compdb_utils.CompDBFormatter.maybe_rewrite_path")
    @mock.patch("bazel_compdb_utils.CompDBFormatter.rewrite_file")
    def test_compdb_formatter_action_to_compile_commands(
        self,
        mock_rewrite_file: mock.MagicMock,
        mock_maybe_rewrite_path: mock.MagicMock,
    ) -> None:
        formatter = bazel_compdb_utils.CompDBFormatter(
            "/build", "/build/out/base", "/build/out/path"
        )
        action_dict = {
            "targetId": 1,
            "actionKey": "key",
            "arguments": ["gcc", "foo.cc", "arg1", "arg2"],
            "environmentVariables": {},
        }
        target_dict = {"label": "//:lib"}
        action = bazel_compdb_utils.Action(action_dict, target_dict)

        mock_rewrite_file.return_value = "rewritten_file"
        mock_maybe_rewrite_path.side_effect = (
            lambda arg, act: f"rewritten_{arg}"
            if arg in ["arg1", "arg2"]
            else arg
        )

        expected_compdb = {
            "directory": "/build",
            "file": "rewritten_file",
            "arguments": ["gcc", "foo.cc", "rewritten_arg1", "rewritten_arg2"],
        }
        self.assertEqual(
            formatter.action_to_compile_commands(action), expected_compdb
        )
        mock_rewrite_file.assert_called_once_with(action)
        mock_maybe_rewrite_path.assert_has_calls(
            [
                mock.call("gcc", action),
                mock.call("foo.cc", action),
                mock.call("arg1", action),
                mock.call("arg2", action),
            ],
            any_order=True,
        )

    def test_collect_actions(self) -> None:
        action_graph = {
            "targets": [
                {"id": 1, "label": "//foo:bar"},
                {"id": 2, "label": "//baz:qux"},
            ],
            "actions": [
                {
                    "targetId": 1,
                    "actionKey": "key1",
                    "arguments": ["gcc", "foo.cc"],
                    "environmentVariables": {},
                },
                {
                    "targetId": 2,
                    "actionKey": "key2",
                    "arguments": ["clang", "baz.cc"],
                    "environmentVariables": {},
                },
            ],
        }

        actions = bazel_compdb_utils.collect_actions(action_graph)

        self.assertEqual(len(actions), 2)
        self.assertIsInstance(actions[0], bazel_compdb_utils.Action)
        self.assertEqual(actions[0].label, "//foo:bar")
        self.assertEqual(actions[0].file, "foo.cc")
        self.assertIsInstance(actions[1], bazel_compdb_utils.Action)
        self.assertEqual(actions[1].label, "//baz:qux")
        self.assertEqual(actions[1].file, "baz.cc")

    def test_collect_actions_empty(self) -> None:
        self.assertEqual(bazel_compdb_utils.collect_actions({}), [])
        self.assertEqual(
            bazel_compdb_utils.collect_actions({"targets": [], "actions": []}),
            [],
        )

    @mock.patch("bazel_compdb_utils.run")
    @mock.patch("bazel_compdb_utils.get_action_graph_from_labels")
    def test_compdb_for_labels(
        self,
        mock_get_actions: mock.MagicMock,
        mock_run: mock.MagicMock,
    ) -> None:
        mock_run.return_value = (
            "output_base: /build/out/base\noutput_path: /build/out/path"
        )
        action_dict1 = {
            "targetId": 1,
            "actionKey": "key1",
            "arguments": ["gcc", "foo.cc"],
            "environmentVariables": {},
        }
        target_dict1 = {"label": "//foo:bar"}
        action1 = bazel_compdb_utils.Action(action_dict1, target_dict1)

        action_dict2 = {
            "targetId": 2,
            "actionKey": "key2",
            "arguments": ["clang", "baz.cc"],
            "environmentVariables": {},
        }
        target_dict2 = {"label": "//baz:qux"}
        action2 = bazel_compdb_utils.Action(action_dict2, target_dict2)
        mock_get_actions.return_value = [action1, action2]

        build_dir = Path("/build")
        result = bazel_compdb_utils.compdb_for_labels(
            build_dir, "bazel", ["--config=fuchsia"], ["//foo:bar"]
        )

        self.assertEqual(
            result,
            [
                {
                    "directory": "/build",
                    "file": "../../foo.cc",
                    "arguments": ["gcc", "../../foo.cc"],
                },
                {
                    "directory": "/build",
                    "file": "../../baz.cc",
                    "arguments": ["clang", "../../baz.cc"],
                },
            ],
        )
        mock_get_actions.assert_called_once_with(
            "bazel", ["--config=fuchsia"], ["//foo:bar"]
        )
        mock_run.assert_called_once_with(
            "bazel", "info", "output_base", "output_path"
        )

    def test_dedupe(self) -> None:
        compdb = [
            {"file": "a.cc", "arguments": ["gcc", "a.cc"]},
            {"file": "b.cc", "arguments": ["gcc", "b.cc"]},
            {"file": "a.cc", "arguments": ["clang", "a.cc"]},
        ]
        deduped = bazel_compdb_utils.dedupe(compdb)
        self.assertEqual(
            sorted(deduped, key=lambda c: c["file"]),
            [
                {"file": "a.cc", "arguments": ["clang", "a.cc"]},
                {"file": "b.cc", "arguments": ["gcc", "b.cc"]},
            ],
        )

    def test_dedupe_empty(self) -> None:
        self.assertEqual(bazel_compdb_utils.dedupe([]), [])


if __name__ == "__main__":
    unittest.main()
