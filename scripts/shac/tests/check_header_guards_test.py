#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
import io
import pathlib
import sys
import tempfile
import unittest
from collections.abc import Collection
from unittest import mock

# Add parent directory to path so we can import check_header_guards
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import check_header_guards


class TestCheckHeaderGuards(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = self.enterContext(tempfile.TemporaryDirectory())
        self.root = pathlib.Path(self.temp_dir)

    def test_header_guard_from_path_standard(self) -> None:
        path = self.root / "src" / "lib" / "foo" / "bar.h"
        guard = check_header_guards._header_guard_from_path(path, self.root)
        self.assertEqual(guard, "SRC_LIB_FOO_BAR_H_")

    def test_header_guard_from_path_special_chars(self) -> None:
        path = self.root / "src" / "foo-bar.baz" / "qux.h"
        guard = check_header_guards._header_guard_from_path(path, self.root)
        self.assertEqual(guard, "SRC_FOO_BAR_BAZ_QUX_H_")

    def test_header_guard_from_path_public_prefix(self) -> None:
        path = (
            self.root
            / "sdk"
            / "lib"
            / "fdio"
            / "include"
            / "lib"
            / "fdio"
            / "spawn.h"
        )
        guard = check_header_guards._header_guard_from_path(path, self.root)
        self.assertEqual(guard, "LIB_FDIO_SPAWN_H_")

    def test_header_guard_from_path_sysroot_prefix(self) -> None:
        path = (
            self.root
            / "zircon"
            / "third_party"
            / "ulib"
            / "musl"
            / "include"
            / "stdio.h"
        )
        guard = check_header_guards._header_guard_from_path(path, self.root)
        self.assertEqual(guard, "SYSROOT_STDIO_H_")

    def test_header_guard_from_path_outside_root(self) -> None:
        path = pathlib.Path("/other/path/outside/root.h")
        with self.assertRaises(check_header_guards.HeaderGuardError):
            check_header_guards._header_guard_from_path(path, self.root)

    def test_generate_fixed_content_empty(self) -> None:
        fixed = check_header_guards._generate_fixed_content("", "FOO_BAR_H_")
        expected = (
            "#ifndef FOO_BAR_H_\n#define FOO_BAR_H_\n\n#endif  // FOO_BAR_H_\n"
        )
        self.assertEqual(fixed, expected)

    def test_generate_fixed_content_pragma_once(self) -> None:
        content = "// Copyright notice\n\n#pragma once\n\nvoid foo();\n"
        fixed = check_header_guards._generate_fixed_content(
            content, "FOO_BAR_H_"
        )
        expected = (
            "// Copyright notice\n\n"
            "#ifndef FOO_BAR_H_\n#define FOO_BAR_H_\n\n"
            "void foo();\n\n"
            "#endif  // FOO_BAR_H_\n"
        )
        self.assertEqual(fixed, expected)

    def test_generate_fixed_content_multiple_pragma_once_raises(self) -> None:
        content = "#pragma once\n#pragma once\n"
        with self.assertRaises(check_header_guards.HeaderGuardFixError):
            check_header_guards._generate_fixed_content(content, "FOO_H_")

    def test_generate_fixed_content_ifndef_after_endif_raises(self) -> None:
        content = "#endif\n#ifndef FOO_H_\n#define FOO_H_\n"
        with self.assertRaises(check_header_guards.HeaderGuardFixError):
            check_header_guards._generate_fixed_content(content, "FOO_H_")
        with self.assertRaises(ValueError):
            check_header_guards._generate_fixed_content(content, "FOO_H_")

    def test_generate_fixed_content_unmatched_ifndef_raises(self) -> None:
        content = "#ifndef FOO_H_\n#define FOO_H_\nvoid foo();\n"
        with self.assertRaises(check_header_guards.HeaderGuardFixError):
            check_header_guards._generate_fixed_content(content, "FOO_H_")
        with self.assertRaises(ValueError):
            check_header_guards._generate_fixed_content(content, "FOO_H_")

    def test_generate_fixed_content_unmatched_endif_raises(self) -> None:
        content = "void foo();\n#endif  // FOO_H_\n"
        with self.assertRaises(check_header_guards.HeaderGuardFixError):
            check_header_guards._generate_fixed_content(content, "FOO_H_")
        with self.assertRaises(ValueError):
            check_header_guards._generate_fixed_content(content, "FOO_H_")

    def test_generate_fixed_content_replaces_outer_guards(self) -> None:
        content = (
            "// Header comment\n"
            "#ifndef OLD_GUARD_H_\n"
            "#define OLD_GUARD_H_\n\n"
            "#ifndef __cplusplus\n"
            "#define __cplusplus\n"
            "#endif\n\n"
            "#endif  // OLD_GUARD_H_\n"
        )
        fixed = check_header_guards._generate_fixed_content(
            content, "NEW_GUARD_H_"
        )
        expected = (
            "// Header comment\n"
            "#ifndef NEW_GUARD_H_\n"
            "#define NEW_GUARD_H_\n\n"
            "#ifndef __cplusplus\n"
            "#define __cplusplus\n"
            "#endif\n\n"
            "#endif  // NEW_GUARD_H_\n"
        )
        self.assertEqual(fixed, expected)

    def test_generate_fixed_content_insert_after_comment(self) -> None:
        content = (
            "// Leading license comment\n" "// line 2\n\n" "void function();\n"
        )
        fixed = check_header_guards._generate_fixed_content(
            content, "NEW_GUARD_H_"
        )
        expected = (
            "// Leading license comment\n"
            "// line 2\n\n"
            "#ifndef NEW_GUARD_H_\n#define NEW_GUARD_H_\n\n"
            "void function();\n\n"
            "#endif  // NEW_GUARD_H_\n"
        )
        self.assertEqual(fixed, expected)

    def test_check_file_valid_guard(self) -> None:
        h_file = self.root / "src" / "test.h"
        h_file.parent.mkdir(parents=True, exist_ok=True)
        h_file.write_text(
            "#ifndef SRC_TEST_H_\n#define SRC_TEST_H_\n\n#endif  // SRC_TEST_H_\n",
            encoding="utf-8",
        )
        tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(list)
        check_header_guards._check_file(
            h_file,
            self.root,
            fix_guards=False,
            emit=False,
            collision_tracker=tracker,
        )
        self.assertEqual(tracker["SRC_TEST_H_"], [h_file])

    def test_check_file_fix_atomic(self) -> None:
        h_file = self.root / "src" / "wrong.h"
        h_file.parent.mkdir(parents=True, exist_ok=True)
        h_file.write_text("#pragma once\nvoid foo();\n", encoding="utf-8")
        tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(list)
        with mock.patch.object(sys, "stdout", io.StringIO()):
            check_header_guards._check_file(
                h_file,
                self.root,
                fix_guards=True,
                emit=False,
                collision_tracker=tracker,
            )
        updated_content = h_file.read_text(encoding="utf-8")
        self.assertIn("#ifndef SRC_WRONG_H_", updated_content)
        self.assertIn("#define SRC_WRONG_H_", updated_content)
        self.assertIn("#endif  // SRC_WRONG_H_", updated_content)

    def test_check_collisions(self) -> None:
        tracker = {
            "GUARD_H_": [
                pathlib.Path("a/foo.h"),
                pathlib.Path("b/foo.h"),
            ]
        }
        with self.assertRaises(check_header_guards.HeaderGuardError):
            check_header_guards._check_collisions(tracker)

        tracker_no_collision = {"GUARD_H_": [pathlib.Path("a/foo.h")]}
        check_header_guards._check_collisions(tracker_no_collision)

    def test_generate_fixed_content_insert_after_block_comment(self) -> None:
        content = (
            "/*\n"
            " * Copyright 2026 The Fuchsia Authors.\n"
            " */\n\n"
            "void function();\n"
        )
        fixed = check_header_guards._generate_fixed_content(
            content, "NEW_GUARD_H_"
        )
        expected = (
            "/*\n"
            " * Copyright 2026 The Fuchsia Authors.\n"
            " */\n\n"
            "#ifndef NEW_GUARD_H_\n#define NEW_GUARD_H_\n\n"
            "void function();\n\n"
            "#endif  // NEW_GUARD_H_\n"
        )
        self.assertEqual(fixed, expected)

    def test_check_file_emit_stdout(self) -> None:
        h_file = self.root / "src" / "emit.h"
        h_file.parent.mkdir(parents=True, exist_ok=True)
        h_file.write_text("// Comment\n#pragma once\n", encoding="utf-8")
        tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(list)

        stdout_buf = io.StringIO()
        with mock.patch.object(sys, "stdout", stdout_buf):
            check_header_guards._check_file(
                h_file,
                self.root,
                fix_guards=False,
                emit=True,
                collision_tracker=tracker,
            )
        output = stdout_buf.getvalue()
        self.assertIn("#ifndef SRC_EMIT_H_", output)
        self.assertIn("#define SRC_EMIT_H_", output)
        self.assertIn("#endif  // SRC_EMIT_H_", output)

    def test_check_file_non_header(self) -> None:
        cc_file = self.root / "src" / "foo.cc"
        tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(list)
        check_header_guards._check_file(
            cc_file,
            self.root,
            fix_guards=False,
            emit=False,
            collision_tracker=tracker,
        )
        self.assertEqual(len(tracker), 0)

    def test_check_file_malformed_multiple_directives(self) -> None:
        h_file = self.root / "src" / "multi.h"
        h_file.parent.mkdir(parents=True, exist_ok=True)
        h_file.write_text(
            "#ifndef SRC_MULTI_H_\n#ifndef SRC_MULTI_H_\n#define SRC_MULTI_H_\n#endif  // SRC_MULTI_H_\n",
            encoding="utf-8",
        )
        tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(list)
        with self.assertRaises(check_header_guards.HeaderGuardError):
            check_header_guards._check_file(
                h_file,
                self.root,
                fix_guards=False,
                emit=False,
                collision_tracker=tracker,
            )

    def test_find_header_files_prunes_third_party_and_hidden(self) -> None:
        src_dir = self.root / "src"
        src_dir.mkdir(parents=True, exist_ok=True)
        good_h = src_dir / "good.h"
        good_h.write_text(
            "#ifndef SRC_GOOD_H_\n#define SRC_GOOD_H_\n\n#endif  // SRC_GOOD_H_\n",
            encoding="utf-8",
        )

        tp_dir = self.root / "third_party" / "ignored"
        tp_dir.mkdir(parents=True, exist_ok=True)
        (tp_dir / "bad.h").write_text("INVALID CONTENT", encoding="utf-8")

        git_dir = self.root / ".git" / "ignored"
        git_dir.mkdir(parents=True, exist_ok=True)
        (git_dir / "bad.h").write_text("INVALID CONTENT", encoding="utf-8")

        headers = check_header_guards._find_header_files(self.root)
        self.assertIsInstance(headers, Collection)
        self.assertEqual(tuple(headers), (good_h,))

    def test_main_cli_modes(self) -> None:
        invalid_h = self.root / "src" / "invalid.h"
        invalid_h.parent.mkdir(parents=True, exist_ok=True)
        invalid_h.write_text("bad content\n", encoding="utf-8")

        # In check mode without --fix or --emit on a file needing a fix
        with mock.patch.object(
            sys,
            "argv",
            [
                "check_header_guards.py",
                "--root",
                str(self.root),
                str(invalid_h),
            ],
        ), mock.patch.object(sys, "stderr", io.StringIO()):
            ret = check_header_guards.main()
            self.assertEqual(ret, 1)

        # In --fix mode
        with mock.patch.object(
            sys,
            "argv",
            [
                "check_header_guards.py",
                "--root",
                str(self.root),
                "--fix",
                str(invalid_h),
            ],
        ), mock.patch.object(sys, "stdout", io.StringIO()), mock.patch.object(
            sys, "stderr", io.StringIO()
        ):
            ret = check_header_guards.main()
            self.assertEqual(ret, 0)

        fixed_text = invalid_h.read_text(encoding="utf-8")
        self.assertIn("#ifndef SRC_INVALID_H_", fixed_text)

    def test_check_file_conflicting_pragma_and_guards(self) -> None:
        h_file = self.root / "src" / "conflict.h"
        h_file.parent.mkdir(parents=True, exist_ok=True)
        h_file.write_text(
            "#pragma once\n#ifndef SRC_CONFLICT_H_\n#define SRC_CONFLICT_H_\n#endif  // SRC_CONFLICT_H_\n",
            encoding="utf-8",
        )
        tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(list)
        with self.assertRaises(check_header_guards.HeaderGuardError):
            check_header_guards._check_file(
                h_file,
                self.root,
                fix_guards=False,
                emit=False,
                collision_tracker=tracker,
            )

    def test_check_file_missing_endif_comment(self) -> None:
        h_file = self.root / "src" / "missing_comment.h"
        h_file.parent.mkdir(parents=True, exist_ok=True)
        h_file.write_text(
            "#ifndef SRC_MISSING_COMMENT_H_\n#define SRC_MISSING_COMMENT_H_\n\n#endif\n",
            encoding="utf-8",
        )
        tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(list)
        with self.assertRaises(check_header_guards.HeaderGuardError):
            check_header_guards._check_file(
                h_file,
                self.root,
                fix_guards=False,
                emit=False,
                collision_tracker=tracker,
            )

    def test_main_cli_nonexistent_path(self) -> None:
        nonexistent = self.root / "src" / "does_not_exist.h"
        with mock.patch.object(
            sys,
            "argv",
            [
                "check_header_guards.py",
                "--root",
                str(self.root),
                str(nonexistent),
            ],
        ), mock.patch.object(sys, "stderr", io.StringIO()):
            ret = check_header_guards.main()
            self.assertEqual(ret, 1)


if __name__ == "__main__":
    unittest.main()
