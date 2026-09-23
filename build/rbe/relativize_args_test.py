#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from pathlib import Path
from unittest import mock

import relativize_args


class SplitTransformJoinTest(unittest.TestCase):
    def test_no_change(self) -> None:
        self.assertEqual(
            relativize_args.split_transform_join("text", "=", lambda x: x),
            "text",
        )

    def test_repeat(self) -> None:
        self.assertEqual(
            relativize_args.split_transform_join("text", "=", lambda x: x + x),
            "texttext",
        )

    def test_with_split(self) -> None:
        self.assertEqual(
            relativize_args.split_transform_join("a=b", "=", lambda x: x + x),
            "aa=bb",
        )

    def test_with_split_recorded(self) -> None:
        renamed_tokens = {}

        def recorded_transform(x: str) -> str:
            new_text = x + x
            renamed_tokens[x] = new_text
            return new_text

        self.assertEqual(
            relativize_args.split_transform_join(
                "a=b", "=", recorded_transform
            ),
            "aa=bb",
        )
        self.assertEqual(renamed_tokens, {"a": "aa", "b": "bb"})


class LexicallyRewriteTokenTest(unittest.TestCase):
    def test_repeat_text(self) -> None:
        self.assertEqual(
            relativize_args.lexically_rewrite_token("foo", lambda x: x + x),
            "foofoo",
        )

    def test_delimters_only(self) -> None:
        self.assertEqual(
            relativize_args.lexically_rewrite_token(
                ",,==,=,=,", lambda x: x + x
            ),
            ",,==,=,=,",
        )

    def test_flag_with_value(self) -> None:
        def transform(x: str) -> str:
            if x.startswith("file"):
                return "tmp-" + x
            else:
                return x

        self.assertEqual(
            relativize_args.lexically_rewrite_token("--foo=file1", transform),
            "--foo=tmp-file1",
        )
        self.assertEqual(
            relativize_args.lexically_rewrite_token(
                "notfile,file1,file2,notfile", transform
            ),
            "notfile,tmp-file1,tmp-file2,notfile",
        )
        self.assertEqual(
            relativize_args.lexically_rewrite_token(
                "--foo=file1,file2", transform
            ),
            "--foo=tmp-file1,tmp-file2",
        )


class GreatestPathParentTest(unittest.TestCase):
    def test_one_component(self) -> None:
        self.assertEqual(
            relativize_args.greatest_path_parent(Path("/root")), Path("/root")
        )

    def test_multiple_components(self) -> None:
        self.assertEqual(
            relativize_args.greatest_path_parent(
                Path("/root/for/the/home/team")
            ),
            Path("/root"),
        )


class RelativizePathTest(unittest.TestCase):
    def test_abspath(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_path("/a/b/c", Path("/a/d")),
                "../b/c",
            )
        mock_exists.assert_called_with()

    def test_relpath(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_path("x/y/z", Path("/a/d")), "x/y/z"
            )
        mock_exists.assert_not_called()  # no absolute path in arg

    def test_cxx_Iflag(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_path("-I/j/k", Path("/j/a/d")),
                "-I../../k",
            )
        mock_exists.assert_called_with()

    def test_cxx_Lflag(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_path("-L/p/q/r", Path("/p/q/z")),
                "-L../r",
            )
        mock_exists.assert_called_with()

    def test_cxx_isystemflag(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_path(
                    "-isystem/r/s/t", Path("/r/v/w")
                ),
                "-isystem../../s/t",
            )
        mock_exists.assert_called_with()

    def test_windows_style_flag(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=False
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_path("/Foo", Path("/p/q/z")), "/Foo"
            )
        mock_exists.assert_called_with()


class RelativizeCommandTest(unittest.TestCase):
    def test_no_transform(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_command(
                    ["echo", "hello"], Path("/home/sweet/home")
                ),
                ["echo", "hello"],
            )
        mock_exists.assert_not_called()  # nothing looks like an absolute path

    def test_with_env(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_command(
                    ["HOME=/home", "echo"], Path("/home/subdir")
                ),
                ["/usr/bin/env", "HOME=..", "echo"],
            )
        mock_exists.assert_called_with()

    def test_relativize(self) -> None:
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            self.assertEqual(
                relativize_args.relativize_command(
                    ["cat", "/meow/foo.txt"], Path("/meow/subdir")
                ),
                ["cat", "../foo.txt"],
            )
        mock_exists.assert_called_with()


class MainArgParserTest(unittest.TestCase):
    def test_no_flags(self) -> None:
        parser = relativize_args.main_arg_parser()
        args = parser.parse_args([])
        self.assertFalse(args.verbose)
        self.assertFalse(args.dry_run)
        self.assertTrue(args.enable)

    def test_verbose(self) -> None:
        parser = relativize_args.main_arg_parser()
        args = parser.parse_args(["--verbose"])
        self.assertTrue(args.verbose)

    def test_dry_run(self) -> None:
        parser = relativize_args.main_arg_parser()
        args = parser.parse_args(["--dry-run"])
        self.assertTrue(args.dry_run)

    def test_disable(self) -> None:
        parser = relativize_args.main_arg_parser()
        args = parser.parse_args(["--disable"])
        self.assertFalse(args.enable)

    def test_cwd(self) -> None:
        parser = relativize_args.main_arg_parser()
        args = parser.parse_args(["--cwd", "/home/foo"])
        self.assertEqual(args.cwd, Path("/home/foo"))

    def test_command(self) -> None:
        parser = relativize_args.main_arg_parser()
        args = parser.parse_args(["--", "echo", "bye"])
        self.assertEqual(args.command, ["echo", "bye"])


if __name__ == "__main__":
    unittest.main()
