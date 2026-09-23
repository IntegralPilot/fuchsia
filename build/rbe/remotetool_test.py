#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import shlex
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

import cl_utils
import remotetool


class DictionaryDiffTests(unittest.TestCase):
    def test_empty(self) -> None:
        left: dict[Any, Any] = dict()
        right: dict[Any, Any] = dict()
        d = remotetool.DictionaryDiff(left, right)
        self.assertEqual(d.left_only, dict())
        self.assertEqual(d.right_only, dict())
        self.assertEqual(d.value_diffs, dict())
        self.assertEqual(d.matches, dict())
        self.assertEqual(list(d.report()), [])

    def test_left_only(self) -> None:
        left = {"a": 1}
        right: dict[Any, Any] = dict()
        d = remotetool.DictionaryDiff(left, right)
        self.assertEqual(d.left_only, left)
        self.assertEqual(d.right_only, dict())
        self.assertEqual(d.value_diffs, dict())
        self.assertEqual(d.matches, dict())
        self.assertIn("left only", list(d.report())[0])

    def test_right_only(self) -> None:
        left: dict[Any, Any] = dict()
        right = {"b": 2}
        d = remotetool.DictionaryDiff(left, right)
        self.assertEqual(d.left_only, dict())
        self.assertEqual(d.right_only, right)
        self.assertEqual(d.value_diffs, dict())
        self.assertEqual(d.matches, dict())
        self.assertIn("right only", list(d.report())[0])

    def test_matches(self) -> None:
        left = {"e": 5}
        right = left
        d = remotetool.DictionaryDiff(left, right)
        self.assertEqual(d.left_only, dict())
        self.assertEqual(d.right_only, dict())
        self.assertEqual(d.value_diffs, dict())
        self.assertEqual(d.matches, right)
        self.assertEqual(list(d.report()), [])

    def test_value_diff(self) -> None:
        left = {"c": 3}
        right = {"c": 4}
        d = remotetool.DictionaryDiff(left, right)
        self.assertEqual(d.left_only, dict())
        self.assertEqual(d.right_only, dict())
        self.assertEqual(d.value_diffs, {"c": (3, 4)})
        self.assertEqual(d.matches, dict())
        self.assertIn("different values", list(d.report())[0])

    def test_mixed(self) -> None:
        left = {"a": 1, "b": 2, "c": 3}
        right = {"b": 2, "c": 4, "d": 4}
        d = remotetool.DictionaryDiff(left, right)
        self.assertEqual(d.left_only, {"a": 1})
        self.assertEqual(d.right_only, {"d": 4})
        self.assertEqual(d.value_diffs, {"c": (3, 4)})
        self.assertEqual(d.matches, {"b": 2})
        self.assertEqual(len(list(d.report())), 3)


class ShowActionResultDiffTests(unittest.TestCase):
    def test_diff_empty(self) -> None:
        left = remotetool.ShowActionResult(
            command=[], platform=dict(), inputs=dict(), output_files=dict()
        )
        diff = left.diff(left)
        self.assertEqual(diff.command_unified_diffs, [])
        self.assertEqual(diff.platform_diffs.left_only, dict())
        self.assertEqual(diff.platform_diffs.right_only, dict())
        self.assertEqual(diff.platform_diffs.value_diffs, dict())
        self.assertEqual(diff.input_diffs.left_only, dict())
        self.assertEqual(diff.input_diffs.right_only, dict())
        self.assertEqual(diff.input_diffs.value_diffs, dict())

    def test_diff_platform(self) -> None:
        left = remotetool.ShowActionResult(
            command=[], platform=dict(), inputs=dict(), output_files=dict()
        )
        right = remotetool.ShowActionResult(
            command=[],
            platform={"xx": "yy"},
            inputs=dict(),
            output_files=dict(),
        )
        diff = left.diff(right)
        self.assertEqual(diff.command_unified_diffs, [])
        self.assertEqual(diff.platform_diffs.left_only, dict())
        self.assertEqual(diff.platform_diffs.right_only, {"xx": "yy"})
        self.assertEqual(diff.platform_diffs.value_diffs, dict())
        self.assertEqual(diff.input_diffs.left_only, dict())
        self.assertEqual(diff.input_diffs.right_only, dict())
        self.assertEqual(diff.input_diffs.value_diffs, dict())

    def test_diff_command(self) -> None:
        left = remotetool.ShowActionResult(
            command=["f", "g"],
            platform=dict(),
            inputs=dict(),
            output_files=dict(),
        )
        right = remotetool.ShowActionResult(
            command=["f", "h"],
            platform=dict(),
            inputs=dict(),
            output_files=dict(),
        )
        diff = left.diff(right)
        self.assertNotEqual(diff.command_unified_diffs, [])
        self.assertEqual(diff.platform_diffs.left_only, dict())
        self.assertEqual(diff.platform_diffs.right_only, dict())
        self.assertEqual(diff.platform_diffs.value_diffs, dict())
        self.assertEqual(diff.input_diffs.left_only, dict())
        self.assertEqual(diff.input_diffs.right_only, dict())
        self.assertEqual(diff.input_diffs.value_diffs, dict())

    def test_diff_platform_with_inputs(self) -> None:
        left = remotetool.ShowActionResult(
            command=[],
            platform=dict(),
            inputs={Path("j.k"): "0123/12"},
            output_files=dict(),
        )
        right = remotetool.ShowActionResult(
            command=[],
            platform=dict(),
            inputs={Path("j.k"): "5678/12"},
            output_files=dict(),
        )
        diff = left.diff(right)
        self.assertEqual(diff.command_unified_diffs, [])
        self.assertEqual(diff.platform_diffs.left_only, dict())
        self.assertEqual(diff.platform_diffs.right_only, dict())
        self.assertEqual(diff.platform_diffs.value_diffs, dict())
        self.assertEqual(diff.input_diffs.left_only, dict())
        self.assertEqual(diff.input_diffs.right_only, dict())
        self.assertEqual(
            diff.input_diffs.value_diffs, {Path("j.k"): ("0123/12", "5678/12")}
        )


class ParseShowActionOutputTests(unittest.TestCase):
    def test_example(self) -> None:
        command = "/usr/bin/env FOO=BAR echo hello world"
        inputs = [
            (Path("path/to/tool"), "1827b1bcd12d0/98"),
            (Path("build/out/obj/intermediate.o"), "bc098af8d8ee/1231"),
        ]
        output_files = [
            (Path("out/dir/lib.a"), "77debacd9001/634"),
            (Path("out/dir/dep.d"), "0efb76111212a/533"),
        ]
        platform = [
            ("OSFamily", "Linux"),
            (
                "container-image",
                "docker://gcr.io/cloud-marketplace/google/debian11@sha256:aaaaaaaaaaaaa5555555555555aaaaaaaaaaaaa555555555",
            ),
        ]
        result = remotetool.ShowActionResult(
            command=shlex.split(command),
            platform={k: v for k, v in platform},
            inputs={k: v for k, v in inputs},
            output_files={k: v for k, v in output_files},
        )
        lines = f"""
Timeout: 1h0m0s
Command
=======
Command Digest: eeeeeefffffffffffffbbbbbbbbbbaaaaaaaaa888888888/8799
	{command}

Platform
========
	{platform[0][0]}={platform[0][1]}
	{platform[1][0]}={platform[1][1]}

Inputs
======
[Root directory digest: 3adf46eb9e1c36bda72009a2a783b188a3c3d3ed48004f4f2cdfc90cbd451acd/248]
{inputs[0][0]}: [File digest: {inputs[0][1]}]
{inputs[1][0]}: [File digest: {inputs[1][1]}]

------------------------------------------------------------------------
Action Result

Exit code: 0

Output Files
============
{output_files[0][0]}, digest: {output_files[0][1]}
{output_files[1][0]}, digest: {output_files[1][1]}

Output Files From Directories
=============================
""".splitlines()
        actual = remotetool.parse_show_action_output(lines)
        self.assertEqual(actual.command, result.command)
        self.assertEqual(actual.platform, result.platform)
        self.assertEqual(actual.inputs, result.inputs)
        self.assertEqual(actual.output_files, result.output_files)
        self.assertEqual(actual, result)

    def test_missing_section(self) -> None:
        with self.assertRaises(remotetool.ParseError):
            remotetool.parse_show_action_output([])


_TEST_CFG = {
    "service": "other.remote.service.com:443",
    "instance": "projects/your-project/instance/default",
}


class ConfigureRemotetoolTests(unittest.TestCase):
    def test_configure(self) -> None:
        with mock.patch.object(
            Path,
            "read_text",
            return_value="".join([f"{k}={v}\n" for k, v in _TEST_CFG.items()]),
        ) as mock_read:
            tool = remotetool.configure_remotetool(Path("r.cfg"))
        self.assertEqual(tool.config, _TEST_CFG)
        mock_read.assert_called_once_with()


class RemotetoolRunTests(unittest.TestCase):
    @property
    def tool(self) -> remotetool.RemoteTool:
        return remotetool.RemoteTool(reproxy_cfg=_TEST_CFG)

    def test_missing_params(self) -> None:
        tool = remotetool.RemoteTool(reproxy_cfg={})
        with self.assertRaises(KeyError):
            tool.run(args=[])

    def test_run_success(self) -> None:
        exit_code = 0
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(exit_code),
        ) as mock_call:
            result = self.tool.run(
                args=[
                    "--operation",
                    "show_action",
                    "--digest",
                    "198273aaabbef87/323",
                ]
            )
        self.assertEqual(result.returncode, exit_code)
        mock_call.assert_called_once()

    def test_run_failure(self) -> None:
        exit_code = 1
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(exit_code),
        ) as mock_call:
            with self.assertRaises(subprocess.CalledProcessError):
                self.tool.run(
                    args=[
                        "--operation",
                        "show_action",  # missing '--digest'
                    ]
                )
        mock_call.assert_called_once()

    def test_show_action_uncached(self) -> None:
        exit_code = 0
        action_result = remotetool.ShowActionResult(
            command=[], platform=dict(), inputs=dict(), output_files=dict()
        )
        with tempfile.TemporaryDirectory() as td:
            # Use a temporary directory for the cache, not the real cache dir.
            with mock.patch.object(
                tempfile, "gettempdir", return_value=td
            ) as mock_tempdir:
                with mock.patch.object(
                    cl_utils,
                    "subprocess_call",
                    return_value=cl_utils.SubprocessResult(exit_code),
                ) as mock_call:
                    with mock.patch.object(
                        remotetool,
                        "parse_show_action_output",
                        return_value=action_result,
                    ) as mock_parse:
                        with mock.patch.object(
                            Path, "exists", return_value=False
                        ) as mock_exists:
                            result = self.tool.show_action(
                                digest="0abb771b3198273aaabbef87/323"
                            )
        self.assertEqual(result, action_result)
        mock_tempdir.assert_called_once_with()
        mock_call.assert_called_once()
        mock_parse.assert_called_once()
        mock_exists.assert_called_once_with()

    def test_show_action_cached(self) -> None:
        action_result = remotetool.ShowActionResult(
            command=[], platform=dict(), inputs=dict(), output_files=dict()
        )
        with tempfile.TemporaryDirectory() as td:
            # Use a temporary directory for the cache, not the real cache dir.
            with mock.patch.object(
                tempfile, "gettempdir", return_value=td
            ) as mock_tempdir:
                with mock.patch.object(
                    remotetool,
                    "parse_show_action_output",
                    return_value=action_result,
                ) as mock_parse:
                    with mock.patch.object(
                        Path, "exists", return_value=True
                    ) as mock_exists:
                        with mock.patch.object(
                            Path, "read_text", return_value="\n"
                        ) as mock_read:
                            result = self.tool.show_action(
                                digest="cc8a65a0abb771b3143abbef87/997"
                            )
        self.assertEqual(result, action_result)
        mock_tempdir.assert_called_once_with()
        mock_parse.assert_called_once()
        mock_exists.assert_called_once_with()
        mock_read.assert_called_once_with()

    def test_download_blob(self) -> None:
        exit_code = 0
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(exit_code),
        ) as mock_call:
            result = self.tool.download_blob(
                path=Path("stash/me/here.txt"),
                digest="17177bbbbe81001bccc/9696",
            )
        mock_call.assert_called_once()
        self.assertEqual(result.returncode, exit_code)

    def test_download_dir(self) -> None:
        exit_code = 0
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(exit_code),
        ) as mock_call:
            result = self.tool.download_dir(
                path=Path("stash/me/dir"),
                digest="000abe888ecedbb11/6936",
            )
        mock_call.assert_called_once()
        self.assertEqual(result.returncode, exit_code)


if __name__ == "__main__":
    unittest.main()
