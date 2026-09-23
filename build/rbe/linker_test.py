#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import tempfile
import unittest
from pathlib import Path
from unittest import mock

import cl_utils
import linker


class TryLinkerScriptTextTests(unittest.TestCase):
    def test_empty_text(self) -> None:
        text = ""
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            text_file = tdp / "foo.so"
            text_file.write_text(text)
            self.assertEqual(linker.try_linker_script_text(text_file), text)

    def test_nonempty_text(self) -> None:
        text = "INPUT(libfoo.so.4)\n"
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            text_file = tdp / "libbar.so"
            text_file.write_text(text)
            self.assertEqual(linker.try_linker_script_text(text_file), text)

    def test_binary_file(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            bin_file = tdp / "libbar.so"
            bin_file.write_bytes(b"\xd0\xff\xfe\x07")
            self.assertIsNone(linker.try_linker_script_text(bin_file))

    def test_archive_header(self) -> None:
        text = b"!<arch>xyzxyzxyz"
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            text_file = tdp / "libarxiv.a"
            text_file.write_bytes(text)
            self.assertIsNone(linker.try_linker_script_text(text_file))

    def test_elf_header(self) -> None:
        text = b"\x7fELF-on-the-shelf"
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            text_file = tdp / "libbar.so"
            text_file.write_bytes(text)
            self.assertIsNone(linker.try_linker_script_text(text_file))

    def test_macho_header(self) -> None:
        text = b"\xca\xfe\xba\xbe\x01\x02"
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            text_file = tdp / "libmar.dylib"
            text_file.write_bytes(text)
            self.assertIsNone(linker.try_linker_script_text(text_file))

    def test_dll_header(self) -> None:
        text = b"\x5a\x4d\xee\xaa\xee\xaa"
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            text_file = tdp / "libzzz.dll"
            text_file.write_bytes(text)
            self.assertIsNone(linker.try_linker_script_text(text_file))


class LinkerScriptParseTests(unittest.TestCase):
    def test_empty(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        self.assertEqual(link.search_paths, [])
        self.assertEqual(link.l_libs, [])
        self.assertEqual(link.direct_files, [])
        self.assertIsNone(link.sysroot)

        expanded = list(link.expand_linker_script(""))

        self.assertEqual(link.search_paths, [])
        self.assertEqual(link.l_libs, [])
        self.assertEqual(link.direct_files, [])
        self.assertIsNone(link.sysroot)
        self.assertEqual(expanded, [])

    def test_nothing_but_space(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("\n  \n    \n      \n"))
        self.assertEqual(expanded, [])

    def test_unscanned_direct_files(self) -> None:
        text = "THIS IS NOT A VALID LINKER SCRIPT AND SHOULD CAUSE A PARSE ERROR IF SCANNED"
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            script = tdp / "bad.ld"
            script.write_text(text)
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, unscanned_direct_files=[script]
            )
            # expand_all should yield the script but not fail parsing it
            expanded = list(link.expand_all())
            self.assertEqual(expanded, [script])

    def test_rodso_content_fails_parsing(self) -> None:
        text = """SECTIONS { . = 0; }"""
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))
            with self.assertRaises(linker.ParseError):
                list(link.expand_linker_script(text))

    def test_rodso_content_unscanned(self) -> None:
        text = """SECTIONS { . = 0; }"""
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            script = tdp / "rodso.ld"
            script.write_text(text)
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, unscanned_direct_files=[script]
            )
            expanded = list(link.expand_all())
            self.assertEqual(expanded, [script])

    def test_comment_one_line(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("/* single-line */\n"))
        self.assertEqual(expanded, [])

    def test_comment_multi_line(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("/* multi\n\nline */\n"))
        self.assertEqual(expanded, [])

    def test_output_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("OUTPUT(libignored.a)\n"))
        self.assertEqual(expanded, [])

    def test_target_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("TARGET(acquired)\n"))
        self.assertEqual(expanded, [])

    def test_extern_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(
            link.expand_linker_script("EXTERN(__sym1__ __sym2__)\n")
        )
        self.assertEqual(expanded, [])

    def test_hidden_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("HIDDEN(_secret_ = .);"))
        self.assertEqual(expanded, [])

    def test_provide_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("PROVIDE(symbol = expr);"))
        self.assertEqual(expanded, [])

    def test_segment_start_inside_provide_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(
            link.expand_linker_script(
                'PROVIDE(symbol = SEGMENT_START("text", 0x40000));'
            )
        )
        self.assertEqual(expanded, [])

    def test_provide_hidden_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(
            link.expand_linker_script("PROVIDE_HIDDEN(__foo = .);\n")
        )
        self.assertEqual(expanded, [])

    def test_output_format_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(link.expand_linker_script("OUTPUT_FORMAT(half-elf)\n"))
        self.assertEqual(expanded, [])

    def test_search_dir(self) -> None:
        dir1 = Path("/some/where/here")
        dir2 = Path("/some/where/there")
        with tempfile.TemporaryDirectory() as td:
            link = linker.LinkerInvocation(working_dir_abs=Path(td))

        expanded = list(
            link.expand_linker_script(f"SEARCH_DIR({dir1})\nSEARCH_DIR({dir2})")
        )
        self.assertEqual(expanded, [])
        self.assertEqual(link.search_paths, [dir1, dir2])  # kept in-order

    def test_one_input_with_extension(self) -> None:
        libdir = Path("baz/lib/foo")
        lib = Path("libbar.so.1")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(working_dir_abs=tdp)
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_path",
                return_value=libdir / lib,
            ) as mock_resolve:
                expanded = list(link.expand_linker_script(f"INPUT({lib})\n"))

        mock_resolve.assert_called_with(lib, check_sysroot=True)
        self.assertEqual(expanded, [libdir / lib])

    def test_one_input_with_extension_space_insensitive(self) -> None:
        libdir = Path("baz/lib/foo")
        lib = Path("libbar.so.1")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(working_dir_abs=tdp)
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_path",
                return_value=libdir / lib,
            ) as mock_resolve:
                expanded = list(
                    link.expand_linker_script(f"INPUT (\n   {lib}\n)\n")
                )

        mock_resolve.assert_called_with(lib, check_sysroot=True)
        self.assertEqual(expanded, [libdir / lib])

    def test_one_input_without_extension(self) -> None:
        libdir = Path("baz/lib/foo")
        lib = Path("libbar.so")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(working_dir_abs=tdp)
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_lib",
                return_value=libdir / lib,
            ) as mock_resolve:
                expanded = list(link.expand_linker_script(f"INPUT( -lbar )\n"))

        mock_resolve.assert_called_with("bar")
        self.assertEqual(expanded, [libdir / lib])

    def test_input_multple_with_extension_with_comma(self) -> None:
        libdir1 = Path("baz/lib/foo")
        lib1 = Path("libbar.so.1")
        libdir2 = Path("qqq/rarlib")
        lib2 = Path("libzzz.so")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, search_paths=[libdir1, libdir2]
            )
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_path",
                side_effect=[libdir1 / lib1, libdir2 / lib2],
            ) as mock_resolve:
                # comma is optional
                expanded = list(
                    link.expand_linker_script(f"INPUT({lib1}, {lib2})\n")
                )

        mock_resolve.assert_has_calls(
            [
                mock.call(lib1, check_sysroot=True),
                mock.call(lib2, check_sysroot=True),
            ]
        )
        self.assertEqual(expanded, [libdir1 / lib1, libdir2 / lib2])

    def test_input_multple_with_extension_without_comma(self) -> None:
        libdir1 = Path("baz/lib/foo")
        lib1 = Path("libbar.so.1")
        libdir2 = Path("qqq/rarlib")
        lib2 = Path("libzzz.so")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, search_paths=[libdir1, libdir2]
            )
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_path",
                side_effect=[libdir1 / lib1, libdir2 / lib2],
            ) as mock_resolve:
                # comma is optional
                expanded = list(
                    link.expand_linker_script(f"INPUT( {lib1} {lib2} )\n")
                )

        mock_resolve.assert_has_calls(
            [
                mock.call(lib1, check_sysroot=True),
                mock.call(lib2, check_sysroot=True),
            ]
        )
        self.assertEqual(expanded, [libdir1 / lib1, libdir2 / lib2])

    def test_group_input_with_extension(self) -> None:
        libdir = Path("baz/lib")
        lib = Path("libfar.so.1")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(working_dir_abs=tdp)
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_path",
                return_value=libdir / lib,
            ) as mock_resolve:
                expanded = list(link.expand_linker_script(f"GROUP({lib})\n"))

        mock_resolve.assert_called_with(lib, check_sysroot=True)
        self.assertEqual(expanded, [libdir / lib])

    def test_as_needed_with_extension(self) -> None:
        libdir = Path("baz/lib/foo")
        lib = Path("libbar.so.1")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, search_paths=[libdir]
            )
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_path",
                return_value=libdir / lib,  # not necessarily normalized
            ) as mock_resolve:
                expanded = list(
                    link.expand_linker_script(f"INPUT( AS_NEEDED({lib}) )\n")
                )

        mock_resolve.assert_called_with(lib, check_sysroot=True)
        self.assertEqual(expanded, [libdir / lib])

    def test_as_needed_multple_with_extension_without_comma(self) -> None:
        libdir1 = Path("baz/lib/foo")
        lib1 = Path("libbar.so.1")
        libdir2 = Path("qqq/rarlib")
        lib2 = Path("libzzz.so")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, search_paths=[libdir1, libdir2]
            )
            with mock.patch.object(
                linker.LinkerInvocation,
                "resolve_path",
                side_effect=[libdir1 / lib1, libdir2 / lib2],
            ) as mock_resolve:
                # comma is optional
                expanded = list(
                    link.expand_linker_script(
                        f"INPUT( AS_NEEDED({lib1}) AS_NEEDED({lib2}) )\n"
                    )
                )

        mock_resolve.assert_has_calls(
            [
                mock.call(lib1, check_sysroot=True),
                mock.call(lib2, check_sysroot=True),
            ]
        )
        self.assertEqual(expanded, [libdir1 / lib1, libdir2 / lib2])

    def test_include_empty(self) -> None:
        libdir = Path("baz/lib/foo")
        lib = Path("libincludeme.so")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / libdir).mkdir(parents=True, exist_ok=True)
            (tdp / libdir / lib).write_text(f"/* empty */\n")
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, search_paths=[libdir]
            )
            expanded = list(link.expand_linker_script(f"INCLUDE {lib}\n"))

        self.assertEqual(expanded, [libdir / lib])

    def test_include_with_lib(self) -> None:
        libdir = Path("baz/lib/foo")
        lib1 = Path("libincludeme.so")
        lib2 = Path("libbar.so.1")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / libdir).mkdir(parents=True, exist_ok=True)
            (tdp / libdir / lib1).write_text(f"INPUT({lib2})\n")
            (tdp / libdir / lib2).touch()
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, search_paths=[libdir]
            )
            expanded = list(link.expand_linker_script(f"INCLUDE {lib1}\n"))

        self.assertEqual(expanded, [libdir / lib1, libdir / lib2])

    def test_include_nested_with_lib(self) -> None:
        libdir = Path("baz/lib/foo")
        lib1 = Path("libincludeme.so")
        lib2 = Path("libforward.so")
        lib3 = Path("libbar.so.1")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / libdir).mkdir(parents=True, exist_ok=True)
            (tdp / libdir / lib1).write_text(f"INCLUDE {lib2}\n")
            (tdp / libdir / lib2).write_text(f"INPUT ( {lib3} )\n")
            (tdp / libdir / lib3).touch()
            link = linker.LinkerInvocation(
                working_dir_abs=tdp, search_paths=[libdir]
            )
            expanded = list(link.expand_linker_script(f"INCLUDE {lib1}\n"))

        self.assertEqual(
            expanded, [libdir / lib1, libdir / lib2, libdir / lib3]
        )


class LinkerInvocationResolveTests(unittest.TestCase):
    def test_resolve_path_failure(self) -> None:
        libdir = Path("baz/lib/foo")
        sysroot = Path("quuz/sysroot")
        lib = Path("libbar.so.1")
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
                search_paths=[libdir],
                sysroot=sysroot,
            )
            resolved = link.resolve_path(lib, check_sysroot=True)

        self.assertIsNone(resolved)

    def test_resolve_path_success_in_search_path(self) -> None:
        libdir = Path("raz/lib/foo")
        sysroot = Path("quuz/sysroot")
        lib = Path("libbar.so.1")

        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / libdir).mkdir(parents=True, exist_ok=True)
            (tdp / libdir / lib).touch()

            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
                search_paths=[libdir],
                sysroot=sysroot,
            )
            resolved = link.resolve_path(lib, check_sysroot=True)

        self.assertEqual(resolved, libdir / lib)

    def test_resolve_path_relative_to_linker_script(self) -> None:
        libdir = Path("raz/lib/foo")
        lib = Path("libbar.ld")
        libdir2 = Path("raz/lib")
        lib2 = Path("quux.o")

        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / libdir).mkdir(parents=True, exist_ok=True)
            (tdp / libdir / lib).write_text("INPUT(../quux.o)\n")
            (tdp / libdir2).mkdir(parents=True, exist_ok=True)
            (tdp / libdir2 / lib2).touch()

            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
            )
            expanded = list(link.expand_possible_linker_script(libdir / lib))

        self.assertEqual(expanded, [libdir / lib, libdir / ".." / lib2])

    def test_resolve_path_success_in_sysroot(self) -> None:
        libdir = Path("raz/lib/foo")
        sysroot = Path("quuz/sysroot")
        lib = Path("libbar.so.1")

        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / sysroot).mkdir(parents=True, exist_ok=True)
            (tdp / sysroot / lib).touch()

            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
                search_paths=[libdir],
                sysroot=sysroot,
            )
            resolved = link.resolve_path(lib, check_sysroot=True)

        self.assertEqual(resolved, sysroot / lib)

    def test_resolve_path_avoiding_sysroot(self) -> None:
        libdir = Path("raz/lib/foo")
        sysroot = Path("quuz/sysroot")
        lib = Path("libbar.so.1")

        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / sysroot).mkdir(parents=True, exist_ok=True)
            (tdp / sysroot / lib).touch()

            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
                search_paths=[libdir],
                sysroot=sysroot,
            )
            resolved = link.resolve_path(lib, check_sysroot=False)

        self.assertIsNone(resolved)

    def test_resolve_lib_failure(self) -> None:
        libdir = Path("snaz/lib")
        sysroot = Path("bar/root")

        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / libdir).mkdir(parents=True, exist_ok=True)

            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
                search_paths=[libdir],
                sysroot=sysroot,
            )
            resolved = link.resolve_lib("foo")

        self.assertIsNone(resolved)

    def test_resolve_lib_success_in_search_path(self) -> None:
        libdir = Path("snaz/lib")
        sysroot = Path("bar/root")
        lib = Path("libfoo.a")

        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / libdir).mkdir(parents=True, exist_ok=True)
            (tdp / libdir / lib).touch()

            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
                search_paths=[libdir],
                sysroot=sysroot,
            )
            resolved = link.resolve_lib("foo")

        self.assertEqual(resolved, libdir / lib)

    def test_resolve_lib_success_in_sysroot(self) -> None:
        libdir = Path("snaz/lib")
        sysroot = Path("bar/root")
        lib = Path("libzoo.a")

        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            (tdp / sysroot).mkdir(parents=True, exist_ok=True)
            (tdp / sysroot / lib).touch()

            link = linker.LinkerInvocation(
                working_dir_abs=tdp,
                search_paths=[libdir],
                sysroot=sysroot,
            )
            resolved = link.resolve_lib("zoo")

        self.assertEqual(resolved, sysroot / lib)

    def test_expand_using_lld(self) -> None:
        depfile_text = """/dev/null: \\
  libfoo.so \\
  libfoo.so.1

# phony deps (to be ignored)
libfoo.so:

libfoo.so.1:

"""
        link = linker.LinkerInvocation()  # not really using parameters
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(
                0, stdout=depfile_text.splitlines()
            ),
        ) as mock_call:
            deps = list(
                link.expand_using_lld(
                    lld=Path("/opt/bin/ld.lld"), inputs=[Path("libfoo.so")]
                )
            )

        self.assertEqual(deps, [Path("libfoo.so"), Path("libfoo.so.1")])


if __name__ == "__main__":
    unittest.main()
