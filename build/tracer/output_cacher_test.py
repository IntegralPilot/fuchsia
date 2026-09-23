#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import filecmp
import os
import shutil
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

import output_cacher


class TempFileTransformTests(unittest.TestCase):
    def test_invalid(self):
        self.assertFalse(output_cacher.TempFileTransform().valid)

    def test_temp_dir_only(self):
        self.assertTrue(
            output_cacher.TempFileTransform(temp_dir="my_tmp").valid
        )
        self.assertTrue(
            output_cacher.TempFileTransform(temp_dir="/my_tmp").valid
        )

    def test_basename_prefix_only(self):
        self.assertTrue(
            output_cacher.TempFileTransform(basename_prefix="temp-").valid
        )

    def test_suffix_only(self):
        self.assertTrue(output_cacher.TempFileTransform(suffix=".tmp").valid)

    def test_temp_dir_and_suffix(self):
        self.assertTrue(
            output_cacher.TempFileTransform(
                temp_dir="throw/away", suffix=".tmp"
            ).valid
        )

    def test_transform_suffix_only(self):
        self.assertEqual(
            output_cacher.TempFileTransform(suffix=".tmp").transform(
                Path("foo/bar.o")
            ),
            Path("foo/bar.o.tmp"),
        )

    def test_transform_temp_dir_only(self):
        self.assertEqual(
            output_cacher.TempFileTransform(temp_dir=Path("t/m/p")).transform(
                Path("foo/bar.o")
            ),
            Path("t/m/p/foo/bar.o"),
        )

    def test_transform_suffix_and_temp_dir(self):
        self.assertEqual(
            output_cacher.TempFileTransform(
                temp_dir=Path("/t/m/p"), suffix=".foo"
            ).transform(Path("foo/bar.o")),
            Path("/t/m/p/foo/bar.o.foo"),
        )

    def test_transform_basename_prefix_only(self):
        self.assertEqual(
            output_cacher.TempFileTransform(basename_prefix="fake.").transform(
                Path("bar.o")
            ),
            Path("fake.bar.o"),
        )
        self.assertEqual(
            output_cacher.TempFileTransform(basename_prefix="fake.").transform(
                Path("foo/bar.o")
            ),
            Path("foo/fake.bar.o"),
        )

    def test_transform_basename_prefix_and_temp_dir(self):
        self.assertEqual(
            output_cacher.TempFileTransform(
                temp_dir="/t/m/p", basename_prefix="xyz-"
            ).transform(Path("foo/bar.o")),
            Path("/t/m/p/foo/xyz-bar.o"),
        )


class SplitTransformJoinTest(unittest.TestCase):
    def test_no_change(self):
        self.assertEqual(
            output_cacher.split_transform_join("text", "=", lambda x: x), "text"
        )

    def test_repeat(self):
        self.assertEqual(
            output_cacher.split_transform_join("text", "=", lambda x: x + x),
            "texttext",
        )

    def test_with_split(self):
        self.assertEqual(
            output_cacher.split_transform_join("a=b", "=", lambda x: x + x),
            "aa=bb",
        )

    def test_with_split_recorded(self):
        renamed_tokens = {}

        def recorded_transform(x):
            new_text = x + x
            renamed_tokens[x] = new_text
            return new_text

        self.assertEqual(
            output_cacher.split_transform_join("a=b", "=", recorded_transform),
            "aa=bb",
        )
        self.assertEqual(renamed_tokens, {"a": "aa", "b": "bb"})


class OutputSubstitutionTest(unittest.TestCase):
    def test_no_prev_opt(self):
        subst = output_cacher.OutputSubstitution("foo")
        self.assertEqual(subst.output_name, "foo")
        self.assertEqual(subst.match_previous_option, "")

    def test_with_prev_opt(self):
        subst = output_cacher.OutputSubstitution("substitute_after:--out:foo")
        self.assertEqual(subst.output_name, "foo")
        self.assertEqual(subst.match_previous_option, "--out")

    def test_malformed_prev_opt(self):
        with self.assertRaises(ValueError):
            output_cacher.OutputSubstitution("substitute_after:--outfoo")


class LexicallyRewriteTokenTest(unittest.TestCase):
    def test_repeat_text(self):
        self.assertEqual(
            output_cacher.lexically_rewrite_token("foo", lambda x: x + x),
            "foofoo",
        )

    def test_delimters_only(self):
        self.assertEqual(
            output_cacher.lexically_rewrite_token(",,==,=,=,", lambda x: x + x),
            ",,==,=,=,",
        )

    def test_flag_with_value(self):
        def transform(x):
            if x.startswith("file"):
                return "tmp-" + x
            else:
                return x

        self.assertEqual(
            output_cacher.lexically_rewrite_token("--foo=file1", transform),
            "--foo=tmp-file1",
        )
        self.assertEqual(
            output_cacher.lexically_rewrite_token(
                "notfile,file1,file2,notfile", transform
            ),
            "notfile,tmp-file1,tmp-file2,notfile",
        )
        self.assertEqual(
            output_cacher.lexically_rewrite_token(
                "--foo=file1,file2", transform
            ),
            "--foo=tmp-file1,tmp-file2",
        )


class EnsureFileExists(unittest.TestCase):
    def test_file_exists(self):
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            output_cacher.ensure_file_exists(Path("some/file.txt"))
        mock_exists.assert_called()
        # just make sure no exception raised

    def test_file_not_exists(self):
        with mock.patch.object(
            Path, "exists", return_value=False
        ) as mock_exists:
            with mock.patch.object(time, "sleep") as mock_sleep:
                with self.assertRaises(FileNotFoundError):
                    output_cacher.ensure_file_exists(Path("not/a/file.txt"))
        mock_exists.assert_called()

    def test_file_exists_later(self):
        with mock.patch.object(
            Path, "exists", side_effect=[False, True]
        ) as mock_exists:
            with mock.patch.object(time, "sleep") as mock_sleep:
                output_cacher.ensure_file_exists(Path("late/file.txt"))
        mock_exists.assert_called()
        mock_sleep.assert_called()
        # just make sure no exception raised


@contextlib.contextmanager
def chdir_cm(d: Path):
    """FIXME: replace with contextlib.chdir(), once Python 3.11 is default."""
    save_dir = os.getcwd()
    os.chdir(d)  # could raise OSError
    try:
        yield
    finally:
        os.chdir(save_dir)


class CopyPreserveSubpathTests(unittest.TestCase):
    def test_subpath(self):
        with tempfile.TemporaryDirectory() as td1:
            tdp1 = Path(td1)
            dest_dir = tdp1 / "backups"
            with chdir_cm(tdp1):  # working directory
                srcdir = Path("aa/bb")
                srcdir.mkdir(parents=True, exist_ok=True)
                src_file = srcdir / "c.txt"
                src_file.write_text("hello\n")
                output_cacher.copy_preserve_subpath(src_file, dest_dir)
                dest_file = dest_dir / src_file
                self.assertTrue(filecmp.cmp(src_file, dest_file, shallow=False))


class DefaultDiffActionTests(unittest.TestCase):
    def test_no_copy(self):
        file1 = Path("srcdir-x/file1.bin")
        file2 = Path("srcdir-y/file2.bin")
        with mock.patch.object(output_cacher, "detail_diff") as mock_diff:
            with mock.patch.object(
                output_cacher, "copy_preserve_subpath"
            ) as mock_copy:
                output_cacher.default_diff_action(file1, file2)

        mock_diff.assert_called_with(file1, file2)
        mock_copy.assert_not_called()

    def test_with_copy(self):
        file1 = Path("srcdir-x/file1.bin")
        file2 = Path("srcdir-y/file2.bin")
        export_dir = Path("bad/diffs")
        with mock.patch.object(output_cacher, "detail_diff") as mock_diff:
            with mock.patch.object(
                output_cacher, "copy_preserve_subpath"
            ) as mock_copy:
                output_cacher.default_diff_action(
                    file1, file2, miscomparison_export_dir=export_dir
                )

        mock_diff.assert_called_with(file1, file2)
        mock_copy.assert_has_calls(
            [
                mock.call(file1, export_dir),
                mock.call(file2, export_dir),
            ],
            any_order=True,
        )


def _detail_diff_pass(a: Path, b: Path) -> int:
    return 0


def _detail_diff_fail(a: Path, b: Path) -> int:
    return 1


class VerifyFilesMatchTests(unittest.TestCase):
    def test_no_files_to_compare_passes(self):
        self.assertEqual(
            output_cacher.verify_files_match(
                fileset={},
                diff_action=_detail_diff_pass,
                label="blah",
                renamed=True,
                keep_backups=True,
            ),
            0,
        )

    def test_one_file_matches(self):
        with mock.patch.object(
            output_cacher, "files_match", return_value=True
        ) as mock_files_match:
            self.assertEqual(
                output_cacher.verify_files_match(
                    fileset={Path("a.txt"): Path("a.txt.bkp")},
                    diff_action=_detail_diff_pass,  # doesn't matter
                    label="blah",
                    renamed=True,
                    keep_backups=True,
                ),
                0,
            )

    def test_one_file_mismatches(self):
        with mock.patch.object(
            output_cacher, "files_match", return_value=False
        ) as mock_files_match:
            self.assertEqual(
                output_cacher.verify_files_match(
                    fileset={Path("a.txt"): Path("a.txt.bkp")},
                    diff_action=_detail_diff_fail,
                    label="blah",
                    renamed=True,
                    keep_backups=True,
                ),
                1,
            )

    def test_all_files_match(self):
        with mock.patch.object(
            output_cacher, "files_match", return_value=True
        ) as mock_files_match:
            self.assertEqual(
                output_cacher.verify_files_match(
                    fileset={
                        Path("a.txt"): Path("a.txt.bkp"),
                        Path("b.txt"): Path("b.txt.bkp"),
                    },
                    diff_action=_detail_diff_pass,  # doesn't matter
                    label="blah",
                    renamed=True,
                    keep_backups=True,
                ),
                0,
            )

    def test_some_files_mismatch(self):
        with mock.patch.object(
            output_cacher, "files_match", side_effect=[True, False]
        ) as mock_files_match:
            self.assertEqual(
                output_cacher.verify_files_match(
                    fileset={
                        Path("a.txt"): Path("a.txt.bkp"),
                        Path("b.txt"): Path("b.txt.bkp"),
                    },
                    diff_action=_detail_diff_fail,
                    label="blah",
                    renamed=True,
                    keep_backups=True,
                ),
                1,
            )

    def test_some_files_mismatch_but_forgiven_by_detail_diff(self):
        with mock.patch.object(
            output_cacher, "files_match", side_effect=[True, False]
        ) as mock_files_match:
            self.assertEqual(
                output_cacher.verify_files_match(
                    fileset={
                        Path("a.txt"): Path("a.txt.bkp"),
                        Path("b.txt"): Path("b.txt.bkp"),
                    },
                    diff_action=_detail_diff_pass,
                    label="blah",
                    renamed=True,
                    keep_backups=True,
                ),
                0,
            )


class MoveIfDifferentTests(unittest.TestCase):
    def test_nonexistent_source(self):
        with mock.patch.object(
            Path, "exists", return_value=False
        ) as mock_exists:
            with mock.patch.object(Path, "rename") as mock_move:
                with mock.patch.object(Path, "unlink") as mock_remove:
                    with mock.patch.object(time, "sleep") as mock_sleep:
                        with self.assertRaises(FileNotFoundError):
                            output_cacher.move_if_different(
                                Path("source.txt"), Path("dest.txt")
                            )
        mock_exists.assert_called_with()
        mock_move.assert_not_called()
        mock_remove.assert_not_called()

    def test_new_output(self):
        with mock.patch.object(
            Path,
            "exists",
            side_effect=[True, False],  # source.txt, dest.txt
        ) as mock_exists:
            with mock.patch.object(output_cacher, "files_match") as mock_diff:
                with mock.patch.object(Path, "rename") as mock_move:
                    with mock.patch.object(Path, "unlink") as mock_remove:
                        moved = output_cacher.move_if_different(
                            Path("source.txt"), Path("dest.txt")
                        )
        self.assertTrue(moved)
        mock_exists.assert_called_with()
        mock_diff.assert_not_called()
        mock_move.assert_called_with(Path("dest.txt"))
        mock_remove.assert_not_called()

    def test_updating_output(self):
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            with mock.patch.object(
                output_cacher, "files_match", return_value=False
            ) as mock_diff:
                with mock.patch.object(Path, "rename") as mock_move:
                    with mock.patch.object(Path, "unlink") as mock_remove:
                        moved = output_cacher.move_if_different(
                            Path("source.txt"), Path("dest.txt")
                        )
        self.assertTrue(moved)
        mock_exists.assert_called_with()
        mock_diff.assert_called_with(Path("dest.txt"), Path("source.txt"))
        mock_move.assert_called_with(Path("dest.txt"))
        mock_remove.assert_not_called()

    def test_cached_output(self):
        with mock.patch.object(
            Path, "exists", return_value=True
        ) as mock_exists:
            with mock.patch.object(
                output_cacher, "files_match", return_value=True
            ) as mock_diff:
                with mock.patch.object(Path, "rename") as mock_move:
                    with mock.patch.object(Path, "unlink") as mock_remove:
                        with mock.patch.object(
                            output_cacher, "ensure_file_exists"
                        ) as mock_exists:
                            moved = output_cacher.move_if_different(
                                Path("source.txt"), Path("dest.txt")
                            )
        self.assertFalse(moved)
        mock_exists.assert_called()
        mock_diff.assert_called()
        mock_move.assert_not_called()
        mock_remove.assert_called_with()


class SubstituteCommandTest(unittest.TestCase):
    def test_no_substitutions(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(command=["run.sh"], substitutions={})
        repl, renamed = action.substitute_command(transform)
        self.assertEqual(repl, ["run.sh"])
        self.assertEqual(renamed, {})

    def test_one_suffix(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "out.txt"], substitutions={"out.txt": ""}
        )
        repl, renamed = action.substitute_command(transform)
        self.assertEqual(repl, ["run.sh", "out.txt.tmp"])
        self.assertEqual(renamed, {Path("out.txt"): Path("out.txt.tmp")})

    def test_require_flag_match_missing_flag(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "out.txt"], substitutions={"out.txt": "-f"}
        )
        repl, renamed = action.substitute_command(transform)
        self.assertEqual(repl, ["run.sh", "out.txt"])
        self.assertEqual(renamed, {})

    def test_require_flag_match_wrong_flag(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "-g", "out.txt"], substitutions={"out.txt": "-f"}
        )
        repl, renamed = action.substitute_command(transform)
        self.assertEqual(repl, ["run.sh", "-g", "out.txt"])
        self.assertEqual(renamed, {})

    def test_require_flag_match_have_flag(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "-f", "out.txt"], substitutions={"out.txt": "-f"}
        )
        repl, renamed = action.substitute_command(transform)
        self.assertEqual(repl, ["run.sh", "-f", "out.txt.tmp"])
        self.assertEqual(renamed, {Path("out.txt"): Path("out.txt.tmp")})

    def test_match_only_one_flag(self):
        transform = output_cacher.TempFileTransform(basename_prefix="tmp-")
        action = output_cacher.Action(
            command=["run.sh", "-x", "out.txt", "-f", "out.txt"],
            substitutions={"out.txt": "-f"},
        )
        repl, renamed = action.substitute_command(transform)
        self.assertEqual(repl, ["run.sh", "-x", "out.txt", "-f", "tmp-out.txt"])
        self.assertEqual(renamed, {Path("out.txt"): Path("tmp-out.txt")})

    def test_match_only_one_flag_first_occurrence(self):
        transform = output_cacher.TempFileTransform(basename_prefix="tmp-")
        action = output_cacher.Action(
            command=["run.sh", "out.txt", "-f", "out.txt", "out.txt"],
            substitutions={"out.txt": "-f"},
        )
        repl, renamed = action.substitute_command(transform)
        self.assertEqual(
            repl, ["run.sh", "out.txt", "-f", "tmp-out.txt", "out.txt"]
        )
        self.assertEqual(renamed, {Path("out.txt"): Path("tmp-out.txt")})


class ReplaceOutputArgsTest(unittest.TestCase):
    def test_dry_run(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(command=["run.sh"], substitutions={})
        with mock.patch.object(
            output_cacher, "execute_main_command"
        ) as mock_call:
            self.assertEqual(action.run_cached(transform, dry_run=True), 0)
        mock_call.assert_not_called()

    def test_command_failed(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(command=["run.sh"], substitutions={})
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=1
        ) as mock_call:
            with mock.patch.object(
                output_cacher, "move_if_different"
            ) as mock_update:
                self.assertEqual(action.run_cached(transform), 1)
        mock_call.assert_called_with(["run.sh"])
        mock_update.assert_not_called()

    def test_command_passed_using_suffix(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "out.put"],
            substitutions={"out.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            # This test doesn't care if move happened.
            with mock.patch.object(
                output_cacher, "move_if_different", return_value=False
            ) as mock_update:
                self.assertEqual(action.run_cached(transform), 0)
        mock_call.assert_called_with(["run.sh", "in.put", "out.put.tmp"])
        mock_update.assert_called_with(
            src=Path("out.put.tmp"), dest=Path("out.put"), verbose=False
        )

    def test_command_passed_using_relative_temp_dir(self):
        transform = output_cacher.TempFileTransform(temp_dir="temp/temp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "foo/out.put"],
            substitutions={"foo/out.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            # This test doesn't care if move happened.
            with mock.patch.object(
                output_cacher, "move_if_different", return_value=False
            ) as mock_update:
                with mock.patch.object(Path, "mkdir") as mock_mkdir:
                    self.assertEqual(action.run_cached(transform), 0)
        mock_mkdir.assert_called_with(parents=True, exist_ok=True)
        mock_call.assert_called_with(
            ["run.sh", "in.put", "temp/temp/foo/out.put"]
        )
        mock_update.assert_called_with(
            src=Path("temp/temp/foo/out.put"),
            dest=Path("foo/out.put"),
            verbose=False,
        )


def fake_detail_diff(left: Path, right: Path):
    # Don't print anything, or attempt to call a diff script
    pass


class RunRepeatedlyCompareTests(unittest.TestCase):
    def test_command_failed(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(command=["run.sh"], substitutions={})
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=1
        ) as mock_call:
            with mock.patch.object(output_cacher, "files_match") as mock_match:
                with mock.patch.object(
                    os.path, "exists", return_value=True
                ) as mock_exists:
                    with mock.patch.object(shutil, "copy2") as mock_copy:
                        with mock.patch.object(os, "makedirs") as mock_mkdir:
                            with mock.patch.object(time, "sleep") as mock_zzz:
                                self.assertEqual(
                                    action.run_repeatedly_and_compare_outputs(
                                        tempfile_transform=transform,
                                        diff_action=fake_detail_diff,
                                    ),
                                    1,
                                )
        mock_call.assert_called_once_with(["run.sh"])
        mock_match.assert_not_called()
        mock_exists.assert_not_called()
        mock_mkdir.assert_not_called()
        mock_copy.assert_not_called()

    def test_command_passed_and_rerun_matches(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "out.put"],
            substitutions={"out.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            with mock.patch.object(
                output_cacher, "files_match", return_value=True
            ) as mock_match:
                with mock.patch.object(
                    Path, "is_file", return_value=True
                ) as mock_isfile:
                    with mock.patch.object(
                        output_cacher, "remove_if_exists"
                    ) as mock_remove:
                        with mock.patch.object(Path, "mkdir") as mock_mkdir:
                            with mock.patch.object(
                                shutil, "copy2"
                            ) as mock_copy:
                                with mock.patch.object(
                                    time, "sleep"
                                ) as mock_zzz:
                                    self.assertEqual(
                                        action.run_repeatedly_and_compare_outputs(
                                            tempfile_transform=transform,
                                            diff_action=fake_detail_diff,
                                            max_attempts=2,
                                        ),
                                        0,
                                    )
        mock_call.assert_has_calls(
            [
                mock.call(["run.sh", "in.put", "out.put"]),  # first run
                mock.call(["run.sh", "in.put", "out.put"]),  # re-run 1
                mock.call(["run.sh", "in.put", "out.put"]),  # re-run 2
            ],
            any_order=True,
        )
        mock_match.assert_called_with(Path("out.put"), Path("out.put.tmp"))
        mock_remove.assert_called_with(Path("out.put.tmp"))
        mock_isfile.assert_called()
        mock_mkdir.assert_not_called()  # using suffix, not temp_dir
        mock_copy.assert_called_once_with(
            Path("out.put"), Path("out.put.tmp"), follow_symlinks=False
        )

    def test_command_passed_and_rerun_differs(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "out.put"],
            substitutions={"out.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            with mock.patch.object(
                output_cacher,
                "files_match",
                # mismatch on 2nd attempt
                side_effect=[True, False],
            ) as mock_match:
                with mock.patch.object(
                    Path, "is_file", return_value=True
                ) as mock_isfile:
                    with mock.patch.object(
                        output_cacher, "remove_if_exists"
                    ) as mock_remove:
                        with mock.patch.object(Path, "mkdir") as mock_mkdir:
                            with mock.patch.object(
                                shutil, "copy2"
                            ) as mock_copy:
                                with mock.patch.object(
                                    time, "sleep"
                                ) as mock_zzz:
                                    self.assertEqual(
                                        action.run_repeatedly_and_compare_outputs(
                                            tempfile_transform=transform,
                                            diff_action=fake_detail_diff,
                                            max_attempts=10,
                                        ),
                                        1,
                                    )
        mock_call.assert_has_calls(
            [
                mock.call(["run.sh", "in.put", "out.put"]),
                mock.call(["run.sh", "in.put", "out.put"]),
                mock.call(["run.sh", "in.put", "out.put"]),
                # Stop after first miscomparison, before max_attempts reached.
            ],
            any_order=True,
        )
        mock_match.assert_called_with(Path("out.put"), Path("out.put.tmp"))
        mock_remove.assert_not_called()  # mismatches kept
        mock_isfile.assert_called()
        mock_mkdir.assert_not_called()  # using suffix, not temp_dir
        mock_copy.assert_called_once_with(
            Path("out.put"), Path("out.put.tmp"), follow_symlinks=False
        )

    def test_command_passed_and_some_outputs_differ(self):
        def fake_match(file1: Path, file2: Path) -> bool:
            if file1 == Path("out.put"):
                return True
            elif file1 == Path("out2.put"):
                return False
            raise ValueError(f"Unhandled file name: {file1}")

        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "out.put", "out2.put"],
            substitutions={"out.put": "", "out2.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            with mock.patch.object(
                output_cacher, "files_match", wraps=fake_match
            ) as mock_match:
                with mock.patch.object(
                    Path, "is_file", return_value=True
                ) as mock_isfile:
                    with mock.patch.object(
                        output_cacher, "remove_if_exists"
                    ) as mock_remove:
                        with mock.patch.object(Path, "mkdir") as mock_mkdir:
                            with mock.patch.object(
                                shutil, "copy2"
                            ) as mock_copy:
                                with mock.patch.object(
                                    time, "sleep"
                                ) as mock_zzz:
                                    self.assertEqual(
                                        action.run_repeatedly_and_compare_outputs(
                                            tempfile_transform=transform,
                                            diff_action=fake_detail_diff,
                                        ),
                                        1,
                                    )
        mock_call.assert_has_calls(
            [
                mock.call(["run.sh", "in.put", "out.put", "out2.put"]),
                mock.call(["run.sh", "in.put", "out.put", "out2.put"]),
            ],
            any_order=True,
        )
        mock_match.assert_has_calls(
            [
                mock.call(Path("out.put"), Path("out.put.tmp")),
                mock.call(Path("out2.put"), Path("out2.put.tmp")),
            ],
            any_order=True,
        )
        mock_remove.assert_not_called()  # keep around mismatches
        mock_isfile.assert_called()
        mock_mkdir.assert_not_called()  # using suffix, not temp_dir
        mock_copy.assert_has_calls(
            [
                mock.call(
                    Path("out.put"), Path("out.put.tmp"), follow_symlinks=False
                ),
                mock.call(
                    Path("out2.put"),
                    Path("out2.put.tmp"),
                    follow_symlinks=False,
                ),
            ],
            any_order=True,
        )


class RunTwiceWithSubstitutionCompareTests(unittest.TestCase):
    def test_command_failed(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(command=["run.sh"], substitutions={})
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=1
        ) as mock_call:
            with mock.patch.object(output_cacher, "files_match") as mock_match:
                self.assertEqual(
                    action.run_twice_with_substitution_and_compare_outputs(
                        tempfile_transform=transform,
                        diff_action=fake_detail_diff,
                    ),
                    1,
                )
        mock_call.assert_called_once_with(["run.sh"])
        mock_match.assert_not_called()

    def test_command_passed_and_rerun_matches(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "out.put"],
            substitutions={"out.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            with mock.patch.object(
                output_cacher, "files_match", return_value=True
            ) as mock_match:
                with mock.patch.object(Path, "unlink") as mock_remove:
                    self.assertEqual(
                        action.run_twice_with_substitution_and_compare_outputs(
                            tempfile_transform=transform,
                            diff_action=fake_detail_diff,
                        ),
                        0,
                    )
        mock_call.assert_has_calls(
            [
                mock.call(["run.sh", "in.put", "out.put"]),
                mock.call(["run.sh", "in.put", "out.put.tmp"]),
            ],
            any_order=True,
        )
        mock_match.assert_called_with(Path("out.put"), Path("out.put.tmp"))
        mock_remove.assert_called_with()

    def test_command_passed_and_rerun_differs(self):
        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "out.put"],
            substitutions={"out.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            with mock.patch.object(
                output_cacher, "files_match", return_value=False
            ) as mock_match:
                with mock.patch.object(os, "remove") as mock_remove:
                    self.assertEqual(
                        action.run_twice_with_substitution_and_compare_outputs(
                            tempfile_transform=transform,
                            diff_action=fake_detail_diff,
                        ),
                        1,
                    )
        mock_call.assert_has_calls(
            [
                mock.call(["run.sh", "in.put", "out.put"]),
                mock.call(["run.sh", "in.put", "out.put.tmp"]),
            ],
            any_order=True,
        )
        mock_match.assert_called_with(Path("out.put"), Path("out.put.tmp"))
        mock_remove.assert_not_called()

    def test_command_passed_and_some_outptus_differ(self):
        def fake_match(file1: Path, file2: Path) -> bool:
            if file1 == Path("out.put"):
                return True
            elif file1 == Path("out2.put"):
                return False
            raise ValueError(f"Unhandled file name: {file1}")

        transform = output_cacher.TempFileTransform(suffix=".tmp")
        action = output_cacher.Action(
            command=["run.sh", "in.put", "out.put", "out2.put"],
            substitutions={"out.put": "", "out2.put": ""},
        )
        with mock.patch.object(
            output_cacher, "execute_main_command", return_value=0
        ) as mock_call:
            with mock.patch.object(
                output_cacher, "files_match", wraps=fake_match
            ) as mock_match:
                with mock.patch.object(Path, "unlink") as mock_remove:
                    self.assertEqual(
                        action.run_twice_with_substitution_and_compare_outputs(
                            tempfile_transform=transform,
                            diff_action=fake_detail_diff,
                        ),
                        1,
                    )
        mock_call.assert_has_calls(
            [
                mock.call(["run.sh", "in.put", "out.put", "out2.put"]),
                mock.call(["run.sh", "in.put", "out.put.tmp", "out2.put.tmp"]),
            ],
            any_order=True,
        )
        mock_match.assert_has_calls(
            [
                mock.call(Path("out.put"), Path("out.put.tmp")),
                mock.call(Path("out2.put"), Path("out2.put.tmp")),
            ],
            any_order=True,
        )
        mock_remove.assert_has_calls([mock.call()])


if __name__ == "__main__":
    unittest.main()
