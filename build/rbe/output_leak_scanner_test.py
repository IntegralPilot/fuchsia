#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Sequence
from pathlib import Path
from typing import Any
from unittest import mock

import output_leak_scanner


def _strs(items: Sequence[Any]) -> Sequence[str]:
    return [str(i) for i in items]


def _paths(items: Sequence[Any]) -> Sequence[Path]:
    return [Path(i) for i in items]


def _write_text_file_contents(path: Path, contents: str) -> None:
    with open(path, "w") as f:
        f.write(contents)


def _write_binary_file_contents(path: Path, contents: bytearray) -> None:
    with open(path, "wb") as f:
        f.write(contents)


class PathPatternTests(unittest.TestCase):
    def test_init(self) -> None:
        path = Path("over/the/rainbow")
        p = output_leak_scanner.PathPattern(path)
        self.assertEqual(p.text, str(path))
        self.assertTrue(p.re_text.search("somewhere/over/the/rainbow"))
        self.assertTrue(p.re_bin.search(b"somewhere/over/the/rainbow"))

    def test_equal(self) -> None:
        # different, but equivalent objects
        self.assertEqual(
            output_leak_scanner.PathPattern(Path("foo/bar")),
            output_leak_scanner.PathPattern(Path("foo/bar")),
        )

    def test_not_equal(self) -> None:
        self.assertNotEqual(
            output_leak_scanner.PathPattern(Path("foo/bar")),
            output_leak_scanner.PathPattern(Path("bar/foo")),
        )

    def test_dot_should_never_be_used(self) -> None:
        with self.assertRaises(ValueError):
            output_leak_scanner.PathPattern(Path())
        with self.assertRaises(ValueError):
            output_leak_scanner.PathPattern(Path("."))

    def test_dots_in_paths_are_literal(self) -> None:
        path = Path("build.obj.stuff")
        p = output_leak_scanner.PathPattern(path)
        self.assertEqual(p.text, str(path))
        self.assertTrue(p.re_text.search("foo/build.obj.stuff/bar"))
        self.assertFalse(p.re_text.search("foo/build_obj_stuff/bar"))


class MainArgParserTests(unittest.TestCase):
    def test_empty(self) -> None:
        parser = output_leak_scanner._MAIN_ARG_PARSER
        args = parser.parse_args([])
        self.assertIsNone(args.label)
        self.assertTrue(args.execute)
        self.assertEqual(args.outputs, [])

    def test_label(self) -> None:
        label = "//foo:bar"
        parser = output_leak_scanner._MAIN_ARG_PARSER
        args = parser.parse_args(["--label", label])
        self.assertEqual(args.label, label)

    def test_execute(self) -> None:
        parser = output_leak_scanner._MAIN_ARG_PARSER
        args = parser.parse_args(["--execute"])
        self.assertTrue(args.execute)

    def test_no_execute(self) -> None:
        parser = output_leak_scanner._MAIN_ARG_PARSER
        args = parser.parse_args(["--no-execute"])
        self.assertFalse(args.execute)

    def test_outputs(self) -> None:
        outputs = ["foo/bar.txt", "bar/foo.o"]
        parser = output_leak_scanner._MAIN_ARG_PARSER
        args = parser.parse_args(outputs)  # positional
        self.assertEqual(args.outputs, _paths(outputs))


class ErrorMsgTests(unittest.TestCase):
    def test_no_label(self) -> None:
        s = io.StringIO()
        with contextlib.redirect_stdout(s):
            output_leak_scanner.error_msg("oops")
        self.assertIn("oops", s.getvalue())

    def test_label(self) -> None:
        s = io.StringIO()
        with contextlib.redirect_stdout(s):
            output_leak_scanner.error_msg("oops", label="LABEL")
        self.assertIn("LABEL", s.getvalue())
        self.assertIn("oops", s.getvalue())


class WholeWordPatternTests(unittest.TestCase):
    def test_add_boundaries(self) -> None:
        self.assertEqual(
            output_leak_scanner._whole_word_pattern("zyx"), r"\bzyx\b"
        )

    def test_no_extra_boundaries(self) -> None:
        self.assertEqual(
            output_leak_scanner._whole_word_pattern(r"\bqwer\b"), r"\bqwer\b"
        )


class FileContainsSubpathTests(unittest.TestCase):
    def test_ignore_nonexistent(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            self.assertFalse(
                output_leak_scanner.file_contains_subpath(
                    Path(td) / "nonexistent",
                    output_leak_scanner.PathPattern(Path("nonexistent")),
                )
            )

    def test_ignore_dir(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            self.assertFalse(
                output_leak_scanner.file_contains_subpath(
                    Path(td), output_leak_scanner.PathPattern(Path("other"))
                )
            )

    def test_text_no_match(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tf = (Path(td) / __name__).with_suffix(".txt")
            _write_text_file_contents(tf, "a\nb\nc\n")
            self.assertFalse(
                output_leak_scanner.file_contains_subpath(
                    tf, output_leak_scanner.PathPattern(Path("def"))
                )
            )

    def test_text_match(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tf = (Path(td) / __name__).with_suffix(".txt")
            _write_text_file_contents(tf, "a\nb\nc\n")
            self.assertTrue(
                output_leak_scanner.file_contains_subpath(
                    tf, output_leak_scanner.PathPattern(Path("b"))
                )
            )

    def test_text_no_match_partial_word(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tf = (Path(td) / __name__).with_suffix(".txt")
            _write_text_file_contents(tf, "ab\nbb\nbc\n")
            self.assertFalse(
                output_leak_scanner.file_contains_subpath(
                    tf, output_leak_scanner.PathPattern(Path("b"))
                )
            )
            self.assertTrue(
                output_leak_scanner.file_contains_subpath(
                    tf, output_leak_scanner.PathPattern(Path("bb"))
                )
            )

    def test_binary_no_match(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tf = (Path(td) / __name__).with_suffix(".txt")
            _write_binary_file_contents(
                tf, bytearray(b"\xcc\n\xdd\xee\n\xff\n+abc+\n")
            )
            self.assertFalse(
                output_leak_scanner.file_contains_subpath(
                    tf, output_leak_scanner.PathPattern(Path("pdq"))
                )
            )

    def test_binary_match(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tf = (Path(td) / __name__).with_suffix(".txt")
            _write_binary_file_contents(
                tf, bytearray(b"\xcc\n\xdd\xee\n\xff\n+abc+\n")
            )
            self.assertTrue(
                output_leak_scanner.file_contains_subpath(
                    tf, output_leak_scanner.PathPattern(Path("abc"))
                )
            )

    def test_binary_no_match_partial(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tf = (Path(td) / __name__).with_suffix(".txt")
            _write_binary_file_contents(
                tf, bytearray(b"\xcc\n\xdd\xee\n\xff\n+abc+\n")
            )
            self.assertFalse(
                output_leak_scanner.file_contains_subpath(
                    tf, output_leak_scanner.PathPattern(Path("b"))
                )
            )


class PathsWithBuildDirLeaksTests(unittest.TestCase):
    def test_negatives(self) -> None:
        build_dir = Path("build/me/here")
        cases = _paths(
            [
                "build/me/there",
                "out/default",
                "rebuild/me/here",
                "build/me/heretic",
            ]
        )
        pattern = output_leak_scanner.PathPattern(build_dir)
        actual = list(
            output_leak_scanner.paths_with_build_dir_leaks(
                cases, pattern.re_text
            )
        )
        self.assertEqual(actual, [])

    def test_positives(self) -> None:
        build_dir = Path("build/me/here")
        cases = _paths(
            ["/tmp/build/me/here", "build/me/here", "build/me/here/foo/bar.txt"]
        )
        pattern = output_leak_scanner.PathPattern(build_dir)
        actual = set(
            output_leak_scanner.paths_with_build_dir_leaks(
                cases, pattern.re_text
            )
        )
        self.assertEqual(actual, set(cases))


class TokensWithBuildDirLeaks(unittest.TestCase):
    def test_negatives(self) -> None:
        build_dir = Path("build/here")
        cases = [
            "-f",
            "build/not/here",
            "out/default",
            "rebuild/here",
            "build/heretic",
        ]
        pattern = output_leak_scanner.PathPattern(build_dir)
        actual = list(
            output_leak_scanner.tokens_with_build_dir_leaks(
                cases, pattern.re_text
            )
        )
        self.assertEqual(actual, [])

    def test_positives(self) -> None:
        build_dir = Path("build/here")
        cases = ["/work/out/build/here", "build/here", "build/here/out.txt"]
        pattern = output_leak_scanner.PathPattern(build_dir)
        actual = list(
            output_leak_scanner.tokens_with_build_dir_leaks(
                cases, pattern.re_text
            )
        )
        self.assertEqual(actual, cases)

    def test_exceptions(self) -> None:
        build_dir = Path("build/here")
        cases = [
            f"-fdebug-prefix-map=/some/where/{build_dir}",
            f"-ffile-prefix-map=/some/where/{build_dir}/foo",
            f"-fmacro-prefix-map=/some/where/{build_dir}/bar",
            f"-fcoverage-prefix-map=/some/where/{build_dir}",
        ]
        pattern = output_leak_scanner.PathPattern(build_dir)
        actual = list(
            output_leak_scanner.tokens_with_build_dir_leaks(
                cases, pattern.re_text
            )
        )
        self.assertEqual(actual, [])


class PreflightChecksTests(unittest.TestCase):
    def test_no_findings(self) -> None:
        build_dir_pattern = output_leak_scanner.PathPattern(Path("any"))
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            return_code = output_leak_scanner.preflight_checks(
                paths=[], command=[], pattern=build_dir_pattern
            )
            self.assertEqual(return_code, 0)
        self.assertEqual(stdout.getvalue(), "")  # quiet

    def test_path_leak(self) -> None:
        build_dir = Path("as/df")
        build_dir_pattern = output_leak_scanner.PathPattern(build_dir)
        stdout = io.StringIO()
        bad = build_dir / "g.h"
        with contextlib.redirect_stdout(stdout):
            return_code = output_leak_scanner.preflight_checks(
                paths=[bad], command=[], pattern=build_dir_pattern
            )
            self.assertEqual(return_code, 1)
        message = stdout.getvalue()
        self.assertIn("Error", message)
        self.assertIn(str(bad), message)

    def test_command_leak(self) -> None:
        build_dir = Path("zs/df")
        build_dir_pattern = output_leak_scanner.PathPattern(build_dir)
        stdout = io.StringIO()
        bad = build_dir / "g.h"
        with contextlib.redirect_stdout(stdout):
            return_code = output_leak_scanner.preflight_checks(
                paths=[],
                command=_strs(["touch", bad]),
                pattern=build_dir_pattern,
            )
            self.assertEqual(return_code, 1)
        message = stdout.getvalue()
        self.assertIn("Error", message)
        self.assertIn(str(bad), message)


class PostflightChecksTests(unittest.TestCase):
    def test_no_findings(self) -> None:
        build_dir_pattern = output_leak_scanner.PathPattern(Path("out/f/b"))
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                output_leak_scanner, "file_contains_subpath", return_value=False
            ) as mock_search:
                return_code = output_leak_scanner.postflight_checks(
                    outputs=[Path("foo.o")], subpath=build_dir_pattern
                )
                self.assertEqual(return_code, 0)
        mock_search.assert_called_once()
        self.assertEqual(stdout.getvalue(), "")

    def test_has_findings(self) -> None:
        build_dir = Path("out/f/b")
        build_dir_pattern = output_leak_scanner.PathPattern(build_dir)
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                output_leak_scanner, "file_contains_subpath", return_value=True
            ) as mock_search:
                return_code = output_leak_scanner.postflight_checks(
                    outputs=[Path("foo.o")], subpath=build_dir_pattern
                )
                self.assertEqual(return_code, 1)
        mock_search.assert_called_once()
        message = stdout.getvalue()
        self.assertIn("Error", message)
        self.assertIn(str(build_dir), message)


class ScanLeaksTests(unittest.TestCase):
    def test_missing_ddash(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            # --help will trigger call to sys.exit, which we
            # want to avoid in testing.
            with mock.patch.object(sys, "exit") as mock_exit:
                return_code = output_leak_scanner.scan_leaks(
                    [],
                    exec_root=Path("/home"),
                    working_dir=Path("/home/build"),
                )
        mock_exit.assert_called_once()
        self.assertEqual(return_code, 1)
        message = stdout.getvalue()
        self.assertIn("Error", message)
        self.assertIn("'--' is missing", message)

    def test_skip_checking_subdir_dot(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                output_leak_scanner, "preflight_checks", return_value=0
            ) as mock_pre_check:
                with mock.patch.object(
                    output_leak_scanner, "postflight_checks", return_value=0
                ) as mock_post_check:
                    with mock.patch.object(
                        subprocess, "call", return_value=0
                    ) as mock_call:
                        return_code = output_leak_scanner.scan_leaks(
                            ["--", "touch", "down.txt"],
                            exec_root=Path("/home/project"),
                            working_dir=Path("/home/project"),
                        )
        mock_pre_check.assert_not_called()
        mock_post_check.assert_not_called()
        mock_call.assert_called_once()
        self.assertEqual(return_code, 0)
        message = stdout.getvalue()
        self.assertEqual(message, "")

    def test_no_execute(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                output_leak_scanner, "preflight_checks", return_value=0
            ) as mock_pre_check:
                with mock.patch.object(
                    subprocess, "call", return_value=1
                ) as mock_call:
                    return_code = output_leak_scanner.scan_leaks(
                        ["--no-execute", "--", "touch", "down.txt"],
                        exec_root=Path("/home"),
                        working_dir=Path("/home/build"),
                    )
            mock_pre_check.assert_called_once()
            mock_call.assert_not_called()
        self.assertEqual(return_code, 0)
        message = stdout.getvalue()
        self.assertEqual(message, "")

    def test_execute_command_error(self) -> None:
        stdout = io.StringIO()
        output = "down.txt"
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                subprocess, "call", return_value=2
            ) as mock_call:
                return_code = output_leak_scanner.scan_leaks(
                    [output, "--", "touch", output],
                    exec_root=Path("/home"),
                    working_dir=Path("/home/build"),
                )
            mock_call.assert_called_once()
        self.assertEqual(return_code, 2)
        message = stdout.getvalue()
        self.assertEqual(message, "")

    def test_execute_success_preflight_error(self) -> None:
        stdout = io.StringIO()
        output = "down.txt"
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                output_leak_scanner, "preflight_checks", return_value=1
            ) as mock_pre_check:
                with mock.patch.object(
                    output_leak_scanner, "postflight_checks", return_value=0
                ) as mock_post_check:
                    with mock.patch.object(
                        subprocess, "call", return_value=0
                    ) as mock_call:
                        return_code = output_leak_scanner.scan_leaks(
                            [output, "--", "touch", output],
                            exec_root=Path("/home"),
                            working_dir=Path("/home/build"),
                        )
            mock_pre_check.assert_called_once()
            mock_call.assert_called_once()
            mock_post_check.assert_called_once()
        self.assertEqual(return_code, 1)

    def test_execute_success_postflight_error(self) -> None:
        stdout = io.StringIO()
        output = Path("down.txt")
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                output_leak_scanner, "preflight_checks", return_value=0
            ) as mock_pre_check:
                with mock.patch.object(
                    output_leak_scanner, "postflight_checks", return_value=1
                ) as mock_post_check:
                    with mock.patch.object(
                        subprocess, "call", return_value=0
                    ) as mock_call:
                        return_code = output_leak_scanner.scan_leaks(
                            _strs([output, "--", "touch", output]),
                            exec_root=Path("/home"),
                            working_dir=Path("/home/build"),
                        )
            mock_pre_check.assert_called_once()
            mock_call.assert_called_once()
            mock_post_check.assert_called_once()
        self.assertEqual(return_code, 1)

    def test_execute_success_scans_clean(self) -> None:
        stdout = io.StringIO()
        output = Path("down.txt")
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                output_leak_scanner, "preflight_checks", return_value=0
            ) as mock_pre_check:
                with mock.patch.object(
                    output_leak_scanner, "postflight_checks", return_value=0
                ) as mock_post_check:
                    with mock.patch.object(
                        subprocess, "call", return_value=0
                    ) as mock_call:
                        return_code = output_leak_scanner.scan_leaks(
                            _strs([output, "--", "touch", output]),
                            exec_root=Path("/home"),
                            working_dir=Path("/home/build"),
                        )
            mock_pre_check.assert_called_once()
            mock_call.assert_called_once()
            mock_post_check.assert_called_once()
        self.assertEqual(return_code, 0)


if __name__ == "__main__":
    unittest.main()
