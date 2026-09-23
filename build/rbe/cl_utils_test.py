#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import contextlib
import filecmp
import io
import multiprocessing
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import cl_utils


class ImmediateExit(Exception):
    """Mocked calls that are not expected to return can raise this.

    Examples: os.exec*(), sys.exit()
    """


class AutoEnvPrefixCommandTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(cl_utils.auto_env_prefix_command([]), [])

    def test_no_prefix(self) -> None:
        self.assertEqual(cl_utils.auto_env_prefix_command(["echo"]), ["echo"])

    def test_env_looking_arg(self) -> None:
        self.assertEqual(
            cl_utils.auto_env_prefix_command(["echo", "BAR=FOO"]),
            ["echo", "BAR=FOO"],
        )

    def test_need_prefix(self) -> None:
        self.assertEqual(
            cl_utils.auto_env_prefix_command(["FOO=BAR", "echo"]),
            [cl_utils._ENV, "FOO=BAR", "echo"],
        )


class TimerCMTests(unittest.TestCase):
    @mock.patch("cl_utils._ENABLE_TIMERS", True)
    def test_basic(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            with cl_utils.timer_cm("descriptive text"):
                pass
        lines = output.getvalue().splitlines()
        self.assertIn("start: descriptive text", lines[0])
        self.assertIn("end  : descriptive text", lines[1])


class BoolGolangFlagTests(unittest.TestCase):
    def test_true(self) -> None:
        for v in ("1", "t", "T", "true", "True", "TRUE"):
            self.assertTrue(cl_utils.bool_golang_flag(v))

    def test_false(self) -> None:
        for v in ("0", "f", "F", "false", "False", "FALSE"):
            self.assertFalse(cl_utils.bool_golang_flag(v))

    def test_invalid(self) -> None:
        for v in ("", "maybe", "true-ish", "false-y"):
            with self.assertRaises(KeyError):
                cl_utils.bool_golang_flag(v)


class CopyPreserveSubpathTests(unittest.TestCase):
    def test_subpath(self) -> None:
        with tempfile.TemporaryDirectory() as td1:
            tdp1 = Path(td1)
            dest_dir = tdp1 / "backups"
            with cl_utils.chdir_cm(tdp1):  # working directory
                srcdir = Path("aa/bb")
                srcdir.mkdir(parents=True, exist_ok=True)
                src_file = srcdir / "c.txt"
                src_file.write_text("hello\n")
                cl_utils.copy_preserve_subpath(src_file, dest_dir)
                dest_file = dest_dir / src_file
                self.assertTrue(filecmp.cmp(src_file, dest_file, shallow=False))

    def test_do_not_recopy_if_identical(self) -> None:
        with tempfile.TemporaryDirectory() as td1:
            tdp1 = Path(td1)
            dest_dir = tdp1 / "backups"
            with cl_utils.chdir_cm(tdp1):  # working directory
                srcdir = Path("aa/bb")
                srcdir.mkdir(parents=True, exist_ok=True)
                src_file = srcdir / "c.txt"
                src_file.write_text("hello\n")
                cl_utils.copy_preserve_subpath(src_file, dest_dir)
                dest_file = dest_dir / src_file
                self.assertTrue(filecmp.cmp(src_file, dest_file, shallow=False))

                # Attempting to copy over identical file should be suppressed.
                with mock.patch.object(shutil, "copy2") as mock_copy:
                    cl_utils.copy_preserve_subpath(src_file, dest_dir)
                mock_copy.assert_not_called()

    def test_do_not_copy_overwrite_if_different(self) -> None:
        with tempfile.TemporaryDirectory() as td1:
            tdp1 = Path(td1)
            dest_dir = tdp1 / "backups"
            with cl_utils.chdir_cm(tdp1):  # working directory
                srcdir = Path("aa/bb")
                srcdir.mkdir(parents=True, exist_ok=True)
                src_file = srcdir / "c.txt"
                src_file.write_text("hello\n")
                cl_utils.copy_preserve_subpath(src_file, dest_dir)
                dest_file = dest_dir / src_file
                self.assertTrue(filecmp.cmp(src_file, dest_file, shallow=False))

                # Attempting to copy over different file is suppressed
                src_file.write_text("not hello\n")
                with mock.patch.object(shutil, "copy2") as mock_copy:
                    cl_utils.copy_preserve_subpath(src_file, dest_dir)
                mock_copy.assert_not_called()

    def test_src_is_dir(self) -> None:
        with tempfile.TemporaryDirectory() as wd:
            wdp = Path(wd)
            src_subdir = Path("srcs/a/b")  # relative
            dest_subdir = Path("dest_dir/x/y")
            dest_dir = wdp / dest_subdir
            with cl_utils.chdir_cm(wdp):  # working directory
                src_subdir.mkdir(parents=True)
                src_file = src_subdir / "cdef.txt"
                src_file.write_text("nothing to see\n")
                expected_dest_file = dest_dir / src_subdir / "cdef.txt"
                self.assertFalse(expected_dest_file.exists())

                cl_utils.copy_preserve_subpath(src_subdir, dest_dir)
                self.assertTrue(expected_dest_file.exists())
                self.assertEqual(
                    src_file.read_text(), expected_dest_file.read_text()
                )


class PartitionSequenceTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(
            cl_utils.partition_sequence([], 28),
            ([], None, []),
        )

    def test_int_not_found(self) -> None:
        seq = [5, 234, 1, 9]
        self.assertEqual(
            cl_utils.partition_sequence(seq, 28),
            (seq, None, []),
        )

    def test_sep_found_at_beginning(self) -> None:
        left: list[str] = []
        sep = "z"
        right = ["x", "y"]
        seq = left + [sep] + right
        self.assertEqual(
            cl_utils.partition_sequence(seq, sep),
            (left, sep, right),
        )

    def test_sep_found_in_middle(self) -> None:
        left = ["12", "34"]
        sep = "zz"
        right = ["23", "asdf"]
        seq = left + [sep] + right
        self.assertEqual(
            cl_utils.partition_sequence(seq, sep),
            (left, sep, right),
        )

    def test_sep_found_at_end(self) -> None:
        left = ["12", "34", "qw", "er"]
        sep = "yy"
        right: list[str] = []
        seq = left + [sep] + right
        self.assertEqual(
            cl_utils.partition_sequence(seq, sep),
            (left, sep, right),
        )


class SplitIntoSubequencesTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(
            list(cl_utils.split_into_subsequences([], None)),
            [[]],
        )

    def test_only_separators(self) -> None:
        sep = ":"
        self.assertEqual(
            list(cl_utils.split_into_subsequences([sep] * 4, sep)),
            [[]] * 5,
        )

    def test_no_match_separators(self) -> None:
        seq = ["a", "b", "c", "d", "e"]
        sep = "%"
        self.assertEqual(
            list(cl_utils.split_into_subsequences(seq, sep)),
            [seq],
        )

    def test_different_size_slices(self) -> None:
        seq = ["a", "b", "%", "c", "%", "d", "e", "f"]
        sep = "%"
        self.assertEqual(
            list(cl_utils.split_into_subsequences(seq, sep)),
            [
                ["a", "b"],
                ["c"],
                ["d", "e", "f"],
            ],
        )


class MatchPrefixTransformSuffixTests(unittest.TestCase):
    def test_no_match(self) -> None:
        result = cl_utils.match_prefix_transform_suffix(
            "abc", "xyz", lambda x: x
        )
        self.assertIsNone(result)

    def test_match(self) -> None:
        result = cl_utils.match_prefix_transform_suffix(
            "abcdef", "abc", lambda x: x.upper()
        )
        self.assertEqual(result, "abcDEF")


class FlattenCommaListTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(
            list(cl_utils.flatten_comma_list([])),
            [],
        )

    def test_singleton(self) -> None:
        self.assertEqual(
            list(cl_utils.flatten_comma_list(["qwe"])),
            ["qwe"],
        )

    def test_one_comma(self) -> None:
        self.assertEqual(
            list(cl_utils.flatten_comma_list(["qw,er"])),
            ["qw", "er"],
        )

    def test_two_items(self) -> None:
        self.assertEqual(
            list(cl_utils.flatten_comma_list(["as", "df"])),
            ["as", "df"],
        )

    def test_multiple_items_with_commas(self) -> None:
        self.assertEqual(
            list(cl_utils.flatten_comma_list(["as,12", "df", "zx,cv,bn"])),
            ["as", "12", "df", "zx", "cv", "bn"],
        )


class RemoveHashCommentsTests(unittest.TestCase):
    def test_empty_line(self) -> None:
        self.assertEqual(list(cl_utils.remove_hash_comments([""])), [""])

    def test_newline(self) -> None:
        self.assertEqual(list(cl_utils.remove_hash_comments(["\n"])), ["\n"])

    def test_comments(self) -> None:
        self.assertEqual(list(cl_utils.remove_hash_comments(["#"])), [])
        self.assertEqual(list(cl_utils.remove_hash_comments(["##"])), [])
        self.assertEqual(list(cl_utils.remove_hash_comments(["# comment"])), [])

    def test_mixed(self) -> None:
        self.assertEqual(
            list(
                cl_utils.remove_hash_comments(
                    ["#!/she/bang", "--foo", "", "# BAR section", "--bar=baz"]
                )
            ),
            ["--foo", "", "--bar=baz"],
        )


class RemoveCCommentsTests(unittest.TestCase):
    def test_empty_string(self) -> None:
        self.assertEqual(cl_utils.remove_c_comments(""), "")

    def test_whitepace_only(self) -> None:
        self.assertEqual(cl_utils.remove_c_comments(" "), " ")
        self.assertEqual(cl_utils.remove_c_comments("\t"), "\t")
        self.assertEqual(cl_utils.remove_c_comments("\n"), "\n")
        self.assertEqual(cl_utils.remove_c_comments("  \n"), "  \n")
        self.assertEqual(cl_utils.remove_c_comments("\t\n"), "\t\n")

    def test_strings(self) -> None:
        self.assertEqual(cl_utils.remove_c_comments('"qwer"\n'), '"qwer"\n')
        self.assertEqual(
            cl_utils.remove_c_comments('"/* not a comment*/"\n'),
            '"/* not a comment*/"\n',
        )
        self.assertEqual(
            cl_utils.remove_c_comments('"// not a comment"\n'),
            '"// not a comment"\n',
        )

    def test_eol_comment(self) -> None:
        self.assertEqual(cl_utils.remove_c_comments("//ab\n"), " \n")
        self.assertEqual(cl_utils.remove_c_comments("a//b\n"), "a \n")
        self.assertEqual(
            cl_utils.remove_c_comments("c d\ne // f\ngh\n"), "c d\ne  \ngh\n"
        )

    def test_block_comment(self) -> None:
        self.assertEqual(cl_utils.remove_c_comments("a/**/b"), "a b")
        self.assertEqual(cl_utils.remove_c_comments("a/****/b"), "a b")
        self.assertEqual(
            cl_utils.remove_c_comments("a/*nothing to see here*/b"), "a b"
        )
        self.assertEqual(
            cl_utils.remove_c_comments("a/*\nzz\n*/b"), "a b"
        )  # multiline


class StringSetArgparseActionTests(unittest.TestCase):
    def _parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="For testing", add_help=False
        )
        parser.add_argument(
            "--add",
            type=str,
            dest="strset",
            default=set(),
            action=cl_utils.StringSetAdd,
            help="add to set",
        )
        parser.add_argument(
            "--remove",
            type=str,
            dest="strset",
            action=cl_utils.StringSetRemove,
            help="remove from set",
        )
        return parser

    def test_set_default(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args([])
        self.assertEqual(attrs.strset, set())

    def test_set_add(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args(["--add", "AAA"])
        self.assertEqual(attrs.strset, {"AAA"})

    def test_set_add_dupe(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args(
            ["--add", "AAA", "--add", "AAA"]
        )
        self.assertEqual(attrs.strset, {"AAA"})

    def test_set_add_different(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args(
            ["--add", "AAA", "--add", "YYY"]
        )
        self.assertEqual(attrs.strset, {"AAA", "YYY"})

    def test_set_remove_nonexisting(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args(["--remove", "BBB"])
        self.assertEqual(attrs.strset, set())

    def test_set_add_then_remove(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args(
            ["--add", "DDD", "--remove", "DDD"]
        )
        self.assertEqual(attrs.strset, set())

    def test_set_remove_then_add(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args(
            ["--remove", "EEE", "--add", "EEE"]
        )
        self.assertEqual(attrs.strset, {"EEE"})

    def test_set_remove_all(self) -> None:
        parser = self._parser()
        (attrs, others) = parser.parse_known_args(
            ["--add", "AAA", "--add", "YYY", "--remove=all"]
        )
        self.assertEqual(attrs.strset, set())


class ExpandResponseFilesTests(unittest.TestCase):
    def test_no_rspfiles(self) -> None:
        command = ["sed", "-e", "s|foo|bar|"]
        rspfiles: list[Path] = []
        self.assertEqual(
            list(cl_utils.expand_response_files(command, rspfiles)), command
        )
        self.assertEqual(rspfiles, [])

    def test_space_only_rspfile(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            rsp = tdp / "args.rsp"
            rsp.write_text(" \n")
            command = ["tool.sh", f"@{rsp}", "-o", "space.out"]
            rspfiles: list[Path] = []
            self.assertEqual(
                list(cl_utils.expand_response_files(command, rspfiles)),
                ["tool.sh", "-o", "space.out"],
            )
            self.assertEqual(rspfiles, [rsp])

    def test_blank_line_rspfile(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            rsp = tdp / "args.rsp"
            rsp.write_text("\n")
            command = ["tool.sh", f"@{rsp}", "-o", "blank.out"]
            rspfiles: list[Path] = []
            self.assertEqual(
                list(cl_utils.expand_response_files(command, rspfiles)),
                ["tool.sh", "-o", "blank.out"],
            )
            self.assertEqual(rspfiles, [rsp])

    def test_one_rspfile(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            rsp = tdp / "args.rsp"
            rsp.write_text("12\n\n34\n56\n")
            command = ["tool.sh", f"@{rsp}", "-o", "cmd.out"]
            rspfiles: list[Path] = []
            self.assertEqual(
                list(cl_utils.expand_response_files(command, rspfiles)),
                ["tool.sh", "12", "34", "56", "-o", "cmd.out"],
            )
            self.assertEqual(rspfiles, [rsp])

    def test_rspfile_rustc_alternative_syntax(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            rsp = tdp / "args.rsp"
            rsp.write_text("56\n78\n")
            command = ["tool.sh", f"@shell:{rsp}", "-o", "cmd4.out"]
            rspfiles: list[Path] = []
            self.assertEqual(
                list(cl_utils.expand_response_files(command, rspfiles)),
                ["tool.sh", "56", "78", "-o", "cmd4.out"],
            )
            self.assertEqual(rspfiles, [rsp])

    def test_nested_repeated_rspfiles(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            rsp1 = tdp / "args1.rsp"
            rsp2 = tdp / "args2.rsp"
            rsp1.write_text(f"@{rsp2}\nand\n@{rsp2}")
            rsp2.write_text("fee\n#comment\nfigh\n")
            command = ["tool.sh", f"@{rsp1}"]
            rspfiles: list[Path] = []
            self.assertEqual(
                list(cl_utils.expand_response_files(command, rspfiles)),
                ["tool.sh", "fee", "figh", "and", "fee", "figh"],
            )
            self.assertEqual(set(rspfiles), {rsp1, rsp2})

    def test_multitoken_lines(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            rsp = tdp / "args.rsp"
            rsp.write_text(" a  b \nc    d  \n")
            command = ["tool.sh", f"@{rsp}", "-o", "space.out"]
            rspfiles: list[Path] = []
            self.assertEqual(
                list(cl_utils.expand_response_files(command, rspfiles)),
                ["tool.sh", "a", "b", "c", "d", "-o", "space.out"],
            )
            self.assertEqual(rspfiles, [rsp])


class ExpandFusedFlagsTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(
            list(cl_utils.expand_fused_flags([], ["-Z"])),
            [],
        )

    def test_no_expand(self) -> None:
        self.assertEqual(
            list(cl_utils.expand_fused_flags(["-Yfoo"], ["-Z"])),
            ["-Yfoo"],
        )

    def test_expand_one(self) -> None:
        self.assertEqual(
            list(cl_utils.expand_fused_flags(["-Yfoo"], ["-Y"])),
            ["-Y", "foo"],
        )

    def test_expand_multiple(self) -> None:
        self.assertEqual(
            list(
                cl_utils.expand_fused_flags(
                    ["-Xxx", "-Yfog", "-Dbar"], ["-Y", "-X"]
                )
            ),
            ["-X", "xx", "-Y", "fog", "-Dbar"],
        )

    def test_already_expanded(self) -> None:
        self.assertEqual(
            list(cl_utils.expand_fused_flags(["-Y", "foo"], ["-Y"])),
            ["-Y", "foo"],
        )

    def test_expand_repeated(self) -> None:
        self.assertEqual(
            list(
                cl_utils.expand_fused_flags(
                    ["-Yfoo=f", "other", "-Ybar=g"], ["-Y"]
                )
            ),
            ["-Y", "foo=f", "other", "-Y", "bar=g"],
        )


class FuseExpandedFlagsTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(
            list(cl_utils.fuse_expanded_flags([], frozenset({"-Z"}))),
            [],
        )

    def test_no_fuse(self) -> None:
        self.assertEqual(
            list(
                cl_utils.fuse_expanded_flags(["-Y", "foo"], frozenset({"-Z"}))
            ),
            ["-Y", "foo"],
        )

    def test_fuse_one(self) -> None:
        self.assertEqual(
            list(
                cl_utils.fuse_expanded_flags(["-Y", "foo"], frozenset({"-Y"}))
            ),
            ["-Yfoo"],
        )

    def test_already_fused(self) -> None:
        self.assertEqual(
            list(cl_utils.fuse_expanded_flags(["-Wfoo"], frozenset({"-W"}))),
            ["-Wfoo"],
        )

    def test_fuse_repeated(self) -> None:
        self.assertEqual(
            list(
                cl_utils.fuse_expanded_flags(
                    ["-W", "zoo", "blah", "-W", "woof"], frozenset({"-W"})
                )
            ),
            ["-Wzoo", "blah", "-Wwoof"],
        )


class ReadConfigFileLinesTests(unittest.TestCase):
    def test_empty_file(self) -> None:
        self.assertEqual(cl_utils.read_config_file_lines([]), dict())

    def test_ignore_blank_lines(self) -> None:
        self.assertEqual(
            cl_utils.read_config_file_lines(["", "\t", "\n"]), dict()
        )

    def test_ignore_comments(self) -> None:
        self.assertEqual(
            cl_utils.read_config_file_lines(["####", "# comment"]), dict()
        )

    def test_ignore_non_key_value_pairs(self) -> None:
        self.assertEqual(
            cl_utils.read_config_file_lines(["value-only"]), dict()
        )

    def test_key_value(self) -> None:
        self.assertEqual(
            cl_utils.read_config_file_lines(["key=value"]), {"key": "value"}
        )

    def test_last_wins(self) -> None:
        self.assertEqual(
            cl_utils.read_config_file_lines(["key=value-1", "key=value-2"]),
            {"key": "value-2"},
        )


class ValuesDictToConfigValueTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(cl_utils.values_dict_to_config_value({}), "")

    def test_one_value(self) -> None:
        self.assertEqual(
            cl_utils.values_dict_to_config_value({"a": "bb"}), "a=bb"
        )

    def test_bunch_of_values_must_be_key_sorted(self) -> None:
        self.assertEqual(
            cl_utils.values_dict_to_config_value(
                {"a": "yy", "zz": "aa", "pp": "qqq"}
            ),
            "a=yy,pp=qqq,zz=aa",
        )


class KeyedFlagsToValuesDictTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(
            cl_utils.keyed_flags_to_values_dict([]),
            dict(),
        )

    def test_key_no_value(self) -> None:
        self.assertEqual(
            cl_utils.keyed_flags_to_values_dict(["a", "z"]),
            {
                "a": [],
                "z": [],
            },
        )

    def test_blank_string_values(self) -> None:
        self.assertEqual(
            cl_utils.keyed_flags_to_values_dict(["b=", "b=", "e="]),
            {
                "b": ["", ""],
                "e": [""],
            },
        )

    def test_no_repeat_keys(self) -> None:
        self.assertEqual(
            cl_utils.keyed_flags_to_values_dict(["a=b", "c=d"]),
            {
                "a": ["b"],
                "c": ["d"],
            },
        )

    def test_repeat_keys(self) -> None:
        self.assertEqual(
            cl_utils.keyed_flags_to_values_dict(["a=b", "c=d", "a=b", "c=e"]),
            {
                "a": ["b", "b"],
                "c": ["d", "e"],
            },
        )

    def test_convert_values_to_int(self) -> None:
        self.assertEqual(
            cl_utils.keyed_flags_to_values_dict(
                ["a=7", "c=8"], convert_type=int
            ),
            {
                "a": [7],
                "c": [8],
            },
        )

    def test_convert_values_to_path(self) -> None:
        self.assertEqual(
            cl_utils.keyed_flags_to_values_dict(
                ["a=/foo/bar", "c=bar/foo.quux"], convert_type=Path
            ),
            {
                "a": [Path("/foo/bar")],
                "c": [Path("bar/foo.quux")],
            },
        )


class LastValueOrDefaultTests(unittest.TestCase):
    def test_default(self) -> None:
        self.assertEqual(
            cl_utils.last_value_or_default([], "3"),
            "3",
        )

    def test_last_value(self) -> None:
        self.assertEqual(
            cl_utils.last_value_or_default(["1", "2", "5", "6"], "4"),
            "6",
        )


class LastValueOfDictFlagTests(unittest.TestCase):
    def test_default_no_key(self) -> None:
        self.assertEqual(
            cl_utils.last_value_of_dict_flag(
                {"f": ["g", "h"], "p": []}, "z", "default"
            ),
            "default",
        )

    def test_default_empty_values(self) -> None:
        self.assertEqual(
            cl_utils.last_value_of_dict_flag(
                {"f": ["g", "h"], "p": []}, "p", "boring"
            ),
            "boring",
        )

    def test_last_value(self) -> None:
        self.assertEqual(
            cl_utils.last_value_of_dict_flag(
                {"f": ["g", "h"], "p": []}, "f", "boring"
            ),
            "h",
        )


class ExpandPathsFromFilesTests(unittest.TestCase):
    def test_basic(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            paths1 = ["foo/bar1.txt", "bar/foo1.txt"]
            paths2 = ["foo/bar2.txt", "bar/foo2.txt"]
            list1 = tdp / "list1.rsp"
            list2 = tdp / "list2.rsp"
            list1.write_text("\n".join(paths1) + "\n")
            list2.write_text("\n".join(paths2) + "\n")
            all_paths = list(cl_utils.expand_paths_from_files([list1, list2]))
            self.assertEqual(all_paths, [Path(p) for p in paths1 + paths2])

    def test_escaped_spaces(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            paths1 = ["foo\\ spacey1.txt", "bar\\ spacer.txt"]
            paths2 = ["foo\\ spacey2.txt", "bar\\ spaced.txt"]
            list1 = tdp / "list1.rsp"
            list2 = tdp / "list2.rsp"
            list1.write_text("\n".join(paths1) + "\n")
            list2.write_text("\n".join(paths2) + "\n")
            all_paths = list(cl_utils.expand_paths_from_files([list1, list2]))
            self.assertEqual(all_paths, [Path(p) for p in paths1 + paths2])

    def test_multiple_paths_per_line(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            paths1 = ["foo/bar1.txt", "bar/foo1.txt"]
            paths2 = ["foo/bar2.txt", "bar/foo2.txt"]
            list1 = tdp / "list1.rsp"
            list2 = tdp / "list2.rsp"
            list1.write_text(" ".join(paths1) + "\n")
            list2.write_text(" ".join(paths2) + "\n")
            all_paths = list(cl_utils.expand_paths_from_files([list1, list2]))
            self.assertEqual(all_paths, [Path(p) for p in paths1 + paths2])


class FilterOutOptionWithArgTests(unittest.TestCase):
    def test_no_change(self) -> None:
        actual = list(
            cl_utils.filter_out_option_with_arg(
                ["keep", "--all"], "--delete-me"
            )
        )
        self.assertEqual(actual, ["keep", "--all"])

    def test_remove_fused_optarg(self) -> None:
        actual = list(
            cl_utils.filter_out_option_with_arg(
                ["sleep", "--delete-me=--all", "foo"], "--delete-me"
            )
        )
        self.assertEqual(actual, ["sleep", "foo"])

    def test_remove_separate_optarg(self) -> None:
        actual = list(
            cl_utils.filter_out_option_with_arg(
                ["creep", "--erase-me", "--foo=baz", "--bar"], "--erase-me"
            )
        )
        self.assertEqual(actual, ["creep", "--bar"])


class StripOptionPrefixTests(unittest.TestCase):
    def test_no_change(self) -> None:
        actual = list(
            cl_utils.strip_option_prefix(["keep", "--all"], "--keep-me")
        )
        self.assertEqual(actual, ["keep", "--all"])

    def test_remove_fused_optarg(self) -> None:
        actual = list(
            cl_utils.strip_option_prefix(
                ["sleep", "--keep-me=--all", "foo"], "--keep-me"
            )
        )
        self.assertEqual(actual, ["sleep", "--all", "foo"])

    def test_remove_separate_optarg(self) -> None:
        actual = list(
            cl_utils.strip_option_prefix(
                ["creep", "--keep-me", "--foo=baz", "--bar"], "--keep-me"
            )
        )
        self.assertEqual(actual, ["creep", "--foo=baz", "--bar"])


class FlagForwarderTests(unittest.TestCase):
    def test_no_transform(self) -> None:
        f = cl_utils.FlagForwarder([])
        command = ["a", "b", "-c", "d", "--e", "f", "--g=h"]
        forwarded, filtered = f.sift(command)
        self.assertEqual(forwarded, [])
        self.assertEqual(filtered, command)

    def test_renamed_no_optarg(self) -> None:
        f = cl_utils.FlagForwarder(
            [
                cl_utils.ForwardedFlag(
                    name="--old", has_optarg=False, mapped_name="--new"
                )
            ]
        )
        command = ["a", "b", "--old", "d", "--old", "f", "--g=h"]
        forwarded, filtered = f.sift(command)
        self.assertEqual(forwarded, ["--new", "--new"])
        self.assertEqual(filtered, ["a", "b", "d", "f", "--g=h"])

    def test_renamed_with_optarg(self) -> None:
        f = cl_utils.FlagForwarder(
            [
                cl_utils.ForwardedFlag(
                    name="--old", has_optarg=True, mapped_name="--new"
                )
            ]
        )
        command = ["a", "b", "--old", "d", "--old=f", "--g=h"]
        forwarded, filtered = f.sift(command)
        self.assertEqual(forwarded, ["--new", "d", "--new=f"])
        self.assertEqual(filtered, ["a", "b", "--g=h"])

    def test_deleted_no_optarg(self) -> None:
        f = cl_utils.FlagForwarder(
            [
                cl_utils.ForwardedFlag(
                    name="--old", has_optarg=False, mapped_name=""
                )
            ]
        )
        command = ["a", "b", "--old", "d", "--old", "f", "--g=h"]
        forwarded, filtered = f.sift(command)
        self.assertEqual(forwarded, [])
        self.assertEqual(filtered, ["a", "b", "d", "f", "--g=h"])

    def test_deleted_with_optarg(self) -> None:
        f = cl_utils.FlagForwarder(
            [
                cl_utils.ForwardedFlag(
                    name="--old", has_optarg=True, mapped_name=""
                )
            ]
        )
        command = ["a", "b", "--old", "--eek", "--old=-f=z", "--g=h"]
        forwarded, filtered = f.sift(command)
        self.assertEqual(forwarded, ["--eek", "-f=z"])
        self.assertEqual(filtered, ["a", "b", "--g=h"])

    def test_multiple_transforms(self) -> None:
        f = cl_utils.FlagForwarder(
            [
                cl_utils.ForwardedFlag(
                    name="--bad", has_optarg=True, mapped_name="--ugly"
                ),
                cl_utils.ForwardedFlag(
                    name="--old", has_optarg=True, mapped_name=""
                ),
            ]
        )
        command = ["a", "b", "--old", "d", "--bad=f", "--g=h"]
        forwarded, filtered = f.sift(command)
        self.assertEqual(forwarded, ["d", "--ugly=f"])
        self.assertEqual(filtered, ["a", "b", "--g=h"])


class RelpathTests(unittest.TestCase):
    def test_identity(self) -> None:
        self.assertEqual(cl_utils.relpath(Path("a"), Path("a")), Path("."))

    def test_sibling(self) -> None:
        self.assertEqual(cl_utils.relpath(Path("a"), Path("b")), Path("../a"))

    def test_ancestor(self) -> None:
        self.assertEqual(cl_utils.relpath(Path("a"), Path("a/c")), Path(".."))

    def test_subdir(self) -> None:
        self.assertEqual(cl_utils.relpath(Path("a/d"), Path("a")), Path("d"))

    def test_distant(self) -> None:
        self.assertEqual(
            cl_utils.relpath(Path("a/b/c"), Path("x/y/z")),
            Path("../../../a/b/c"),
        )

    def test_common_parent(self) -> None:
        self.assertEqual(
            cl_utils.relpath(Path("a/b/c"), Path("a/y/z")), Path("../../b/c")
        )


def _readlink(path: Path) -> str:
    # Path.readlink() is only available in Python 3.9+
    return os.readlink(str(path))


class SymlinkRelativeTests(unittest.TestCase):
    def test_same_dir(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            dest = Path(td) / "dest.txt"  # doesn't exist
            src = Path(td) / "src.link"
            cl_utils.symlink_relative(dest, src)
            self.assertTrue(src.is_symlink())
            self.assertEqual(_readlink(src), "dest.txt")  # relative
            # Need dest.resolve() on Mac OS where tempdirs can be symlinks
            self.assertEqual(src.resolve(), dest.resolve())

    def test_dest_in_subdir(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            destdir = Path(td) / "must" / "go" / "deeper"
            dest = destdir / "log.txt"  # doesn't exist
            src = Path(td) / "log.link"
            cl_utils.symlink_relative(dest, src)
            self.assertTrue(src.is_symlink())
            self.assertEqual(
                _readlink(src), "must/go/deeper/log.txt"
            )  # relative
            self.assertEqual(src.resolve(), dest.resolve())

    def test_dest_in_parent(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            dest = Path(td) / "log.txt"  # doesn't exist
            srcdir = Path(td) / "must" / "go" / "deeper"  # doesn't exist yet
            src = srcdir / "log.link"
            cl_utils.symlink_relative(dest, src)
            self.assertTrue(src.is_symlink())
            self.assertEqual(_readlink(src), "../../../log.txt")  # relative
            self.assertEqual(src.resolve(), dest.resolve())

    def test_common_parent_srcdir_does_not_exist_yet(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            # td is the common parent to both src and dest
            destdir = Path(td) / "trash" / "bin"
            dest = destdir / "garbage.txt"  # doesn't exist
            srcdir = Path(td) / "must" / "go" / "deeper"  # doesn't exist yet
            src = srcdir / "log.link"
            cl_utils.symlink_relative(dest, src)
            self.assertTrue(src.is_symlink())
            self.assertEqual(
                _readlink(src), "../../../trash/bin/garbage.txt"
            )  # relative
            self.assertEqual(src.resolve(), dest.resolve())

    def test_common_parent_srcdir_already_exists(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            # td is the common parent to both src and dest
            destdir = Path(td) / "trash" / "bin"
            dest = destdir / "garbage.txt"  # doesn't exist
            srcdir = Path(td) / "must" / "go" / "deeper"
            srcdir.mkdir(
                parents=True
            )  # srcdir exists ahead of symlink_relative
            src = srcdir / "log.link"
            cl_utils.symlink_relative(dest, src)
            self.assertTrue(src.is_symlink())
            self.assertEqual(
                _readlink(src), "../../../trash/bin/garbage.txt"
            )  # relative
            self.assertEqual(src.resolve(), dest.resolve())

    def test_link_over_existing_link_dest_does_not_exist(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            dest = Path(td) / "dest.txt"  # doesn't exist
            src = Path(td) / "src.link"
            # note: dest does not actually exist
            cl_utils.symlink_relative(dest, src)
            cl_utils.symlink_relative(dest, src)  # yes, link twice
            self.assertTrue(src.is_symlink())
            self.assertEqual(_readlink(src), "dest.txt")  # relative
            # Need dest.resolve() on Mac OS where tempdirs can be symlinks
            self.assertEqual(src.resolve(), dest.resolve())

    def test_link_replaces_file(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            dest = Path(td) / "dest.txt"  # doesn't exist
            src = Path(td) / "src.link"
            # note: dest does not actually exist
            with open(src, "w") as f:
                f.write("\t\n")
            cl_utils.symlink_relative(dest, src)  # overwrite file
            self.assertTrue(src.is_symlink())
            self.assertEqual(_readlink(src), "dest.txt")  # relative
            # Need dest.resolve() on Mac OS where tempdirs can be symlinks
            self.assertEqual(src.resolve(), dest.resolve())

    def test_link_replaces_dir(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            dest = Path(td) / "dest.txt"  # doesn't exist
            src = Path(td) / "src.link"
            # note: dest does not actually exist
            src.mkdir(parents=True, exist_ok=True)
            cl_utils.symlink_relative(dest, src)  # overwrite empty dir
            self.assertTrue(src.is_symlink())
            self.assertEqual(_readlink(src), "dest.txt")  # relative
            # Need dest.resolve() on Mac OS where tempdirs can be symlinks
            self.assertEqual(src.resolve(), dest.resolve())


class QualifyToolPathTests(unittest.TestCase):
    def test_absolute(self) -> None:
        path = Path("/foo/bar.exe")
        self.assertEqual(cl_utils.qualify_tool_path(path), str(path))

    def test_relative_subdir(self) -> None:
        path = Path("foo/bar.exe")
        self.assertEqual(cl_utils.qualify_tool_path(path), str(path))

    def test_relative_up_and_down(self) -> None:
        path = Path("../../foo/bar.exe")
        self.assertEqual(cl_utils.qualify_tool_path(path), str(path))

    def test_unqualified(self) -> None:
        path = Path("bar.exe")
        self.assertEqual(cl_utils.qualify_tool_path(path), "./bar.exe")

    def test_unqualified_redundant(self) -> None:
        path = Path("./bar.exe")
        self.assertEqual(cl_utils.qualify_tool_path(path), "./bar.exe")


class ExecRelaunchTests(unittest.TestCase):
    def go_away(self) -> None:
        cl_utils.exec_relaunch(["/my/handy/tool"])

    def test_mock_launch(self) -> None:
        """Example of how to mock exec_relaunch()."""
        with mock.patch.object(
            cl_utils, "exec_relaunch", side_effect=ImmediateExit
        ) as mock_launch:
            with self.assertRaises(ImmediateExit):
                self.go_away()

    def test_mock_call(self) -> None:
        exit_code = 21
        with mock.patch.object(
            subprocess, "call", return_value=exit_code
        ) as mock_call:
            with mock.patch.object(
                sys, "exit", side_effect=ImmediateExit
            ) as mock_exit:
                with self.assertRaises(ImmediateExit):
                    self.go_away()
        mock_call.assert_called_once()
        mock_exit.assert_called_with(exit_code)


def increment_file(file: Path) -> int:
    if not file.exists():
        # First writer creates with value 1
        file.write_text("1")
        return 1
    else:
        count = int(file.read_text()) + 1
        file.write_text(f"{count}")
        return count


def locked_increment_file(file: Path) -> int:
    lockfile = file.with_suffix(".lock")
    with cl_utils.BlockingFileLock(lockfile) as lock:
        return increment_file(file)


class BlockingFileLockTests(unittest.TestCase):
    def test_exclusion(self) -> None:
        N = 50
        with tempfile.TemporaryDirectory() as td:
            count_file = Path(td) / "count"
            try:
                with multiprocessing.Pool() as pool:
                    counts = pool.map(locked_increment_file, [count_file] * N)
            except OSError:  # in case /dev/shm is not writeable (required)
                counts = list(map(locked_increment_file, [count_file] * N))

            self.assertEqual(sorted(counts), list(range(1, N + 1)))

            # Run a second batch, same lock file
            try:
                with multiprocessing.Pool() as pool:
                    counts = pool.map(locked_increment_file, [count_file] * N)
            except OSError:  # in case /dev/shm is not writeable (required)
                counts = list(map(locked_increment_file, [count_file] * N))

            self.assertEqual(sorted(counts), list(range(N + 1, 2 * N + 1)))


class SubprocessResultTests(unittest.TestCase):
    def test_defaults(self) -> None:
        result = cl_utils.SubprocessResult(3)
        self.assertEqual(result.returncode, 3)
        self.assertEqual(result.stdout, [])
        self.assertEqual(result.stderr, [])
        self.assertEqual(result.stdout_text, "")
        self.assertEqual(result.stderr_text, "")

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(result.verbose_returncode("process"), 3)
        self.assertEqual(output.getvalue(), "")

    def test_with_output(self) -> None:
        stdout = ["foo", "bar"]
        stderr = ["baz"]
        result = cl_utils.SubprocessResult(1, stdout=stdout, stderr=stderr)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, stdout)
        self.assertEqual(result.stderr, stderr)
        self.assertEqual(result.stdout_text, "foo\nbar")
        self.assertEqual(result.stderr_text, "baz")

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(result.verbose_returncode("process"), 1)
        printed_lines = output.getvalue().splitlines()
        important_lines = [l for l in printed_lines if "----" not in l]
        self.assertEqual(important_lines, stdout + stderr)


class SubprocessCallTests(unittest.TestCase):
    def test_success(self) -> None:
        result = cl_utils.subprocess_call(["echo", "hello"])
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, ["hello"])
        self.assertEqual(result.stderr, [])
        self.assertGreater(result.pid, 0)

    def test_success_quiet(self) -> None:
        result = cl_utils.subprocess_call(["echo", "hello"], quiet=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, ["hello"])  # still captured
        self.assertEqual(result.stderr, [])
        self.assertGreater(result.pid, 0)

    def test_failure(self) -> None:
        result = cl_utils.subprocess_call(["false"])
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, [])
        self.assertEqual(result.stderr, [])
        self.assertGreater(result.pid, 0)

    def test_error(self) -> None:
        result = cl_utils.subprocess_call(["ls", "/does/not/exist"])
        # error code is 2 on linux, 1 on darwin
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, [])
        self.assertIn("No such file or directory", result.stderr[0])
        self.assertIn("/does/not/exist", result.stderr[0])
        self.assertGreater(result.pid, 0)


class SubprocessCommunicateTests(unittest.TestCase):
    def test_cat(self) -> None:
        input = "echo\n"
        result = cl_utils.subprocess_communicate(["cat"], input)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, ["echo"])
        self.assertEqual(result.stderr, [])

    def test_sed(self) -> None:
        input = "aaabbbccc\n"
        result = cl_utils.subprocess_communicate(
            ["sed", "-e", "s|b|B|g"], input
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, ["aaaBBBccc"])
        self.assertEqual(result.stderr, [])

    def test_failure(self) -> None:
        result = cl_utils.subprocess_communicate(
            [sys.executable, "-c", "import sys; sys.exit(1)"], "ignored_text"
        )
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, [])
        self.assertEqual(result.stderr, [])
        self.assertGreater(result.pid, 0)


if __name__ == "__main__":
    unittest.main()
