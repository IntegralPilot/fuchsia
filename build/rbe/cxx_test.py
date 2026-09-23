#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import sys
import unittest
from collections.abc import Sequence
from pathlib import Path
from typing import Any
from unittest import mock

import cxx


def _strs(items: Sequence[Any]) -> list[str]:
    return [str(i) for i in items]


class CxxActionTests(unittest.TestCase):
    def test_help_unwanted(self) -> None:
        source = Path("hello.cc")
        output = Path("hello.o")
        for opt in (
            "-h",
            "--help",
            "-hwasan-record-stack-history=libcall",
            "-hello-world-is-ignored",
        ):
            with mock.patch.object(sys, "exit") as mock_exit:
                c = cxx.CxxAction(
                    _strs(["clang++", opt, "-c", source, "-o", output])
                )
                preprocess, compile = c.split_preprocessing()
            mock_exit.assert_not_called()

    def test_simple_clang_cxx(self) -> None:
        source = Path("hello.cc")
        ii_file = Path("hello.ii")
        output = Path("hello.o")
        clang = Path("clang++")
        c = cxx.CxxAction(_strs([clang, "-c", source, "-o", output]))
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=clang, type=cxx.Compiler.CLANG),
        )
        self.assertFalse(c.save_temps)
        self.assertTrue(c.compiler_is_clang)
        self.assertFalse(c.compiler_is_gcc)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.CXX)]
        )
        self.assertTrue(c.dialect_is_cxx)
        self.assertFalse(c.dialect_is_c)
        self.assertIsNone(c.use_ld)
        self.assertIsNone(c.lto)
        self.assertIsNone(c.rtlib)
        self.assertIsNone(c.unwindlib)
        self.assertFalse(c.static_libstdcxx)
        self.assertEqual(c.linker_driver_flags, [])
        self.assertIsNone(c.linker_retain_symbols_file)
        self.assertIsNone(c.linker_version_script)
        self.assertIsNone(c.linker_just_symbols)
        self.assertIsNone(c.depfile)
        self.assertIsNone(c.sysroot)
        self.assertEqual(c.libdirs, [])
        self.assertEqual(c.libs, [])
        self.assertIsNone(c.profile_list)
        self.assertIsNone(c.profile_generate)
        self.assertIsNone(c.profile_instr_generate)
        self.assertFalse(c.shared)
        self.assertEqual(c.sanitizers, set())
        self.assertFalse(c.using_asan)
        self.assertFalse(c.using_ubsan)
        self.assertEqual(c.uninterpreted_args, _strs([clang, "-c", source]))
        self.assertEqual(list(c.input_files()), [source])
        self.assertEqual(list(c.output_files()), [output])
        self.assertEqual(list(c.output_dirs()), [])
        self.assertFalse(c.uses_macos_sdk)
        self.assertIsNone(c.crash_diagnostics_dir)
        self.assertEqual(c.preprocessed_output, ii_file)
        preprocess, compile = c.split_preprocessing()
        self.assertEqual(
            preprocess,
            _strs(
                ["clang++", "-c", source, "-o", ii_file, "-E", "-fno-blocks"]
            ),
        )
        self.assertEqual(
            compile,
            _strs(["clang++", "-c", ii_file, "-o", output]),
        )

    def test_all_unknown_flags_ignored(self) -> None:
        command = ["clang", "--yolo", "foobar", "-q", "-k", "-quack"]
        c = cxx.CxxAction(command + ["-o", "required.o"])
        self.assertEqual(c.uninterpreted_args, command)

    def test_output_flag_not_matched(self) -> None:
        command_pieces = (
            ["clang"],
            ["-octopus"],
        )  # not to be interpreted as "-o"
        output = Path("required.o")
        c = cxx.CxxAction(
            command_pieces[0] + _strs(["-o", output]) + command_pieces[1]
        )
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.uninterpreted_args, command_pieces[0] + command_pieces[1]
        )

    def test_clang_cxx_save_temps(self) -> None:
        source = Path("hello.cc")
        ii_file = Path("obj/path/to/hello.cc.ii")
        output = Path("obj/path/to/hello.cc.o")
        c = cxx.CxxAction(
            _strs(["clang++", "-c", source, "-o", output, "--save-temps"])
        )
        self.assertTrue(c.save_temps)
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=Path("clang++"), type=cxx.Compiler.CLANG),
        )
        self.assertTrue(c.compiler_is_clang)
        self.assertFalse(c.compiler_is_gcc)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.CXX)]
        )
        self.assertTrue(c.dialect_is_cxx)
        self.assertFalse(c.dialect_is_c)
        self.assertIsNone(c.depfile)
        self.assertIsNone(c.sysroot)
        self.assertIsNone(c.profile_list)
        self.assertEqual(list(c.input_files()), [source])
        self.assertEqual(
            list(c.output_files()),
            [
                output,
                # This .ii file is implicitly output by the compiler,
                # but not the same as what we explicitly choose with ii_file.
                Path("hello.ii"),
                Path("hello.bc"),
                Path("hello.s"),
            ],
        )
        self.assertEqual(list(c.output_dirs()), [])
        self.assertFalse(c.uses_macos_sdk)
        self.assertIsNone(c.crash_diagnostics_dir)
        self.assertEqual(c.preprocessed_output, ii_file)
        preprocess, compile = c.split_preprocessing()
        self.assertEqual(
            preprocess,
            _strs(
                ["clang++", "-c", source, "-o", ii_file, "-E", "-fno-blocks"]
            ),
        )
        self.assertEqual(
            compile,
            _strs(["clang++", "-c", ii_file, "-o", output, "--save-temps"]),
        )

    def test_simple_clang_c(self) -> None:
        source = Path("hello.c")
        i_file = Path("hello.i")
        output = Path("hello.o")
        c = cxx.CxxAction(_strs(["clang", "-c", source, "-o", output]))
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=Path("clang"), type=cxx.Compiler.CLANG),
        )
        self.assertTrue(c.compiler_is_clang)
        self.assertFalse(c.compiler_is_gcc)
        self.assertFalse(c.save_temps)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.C)]
        )
        self.assertFalse(c.dialect_is_cxx)
        self.assertTrue(c.dialect_is_c)
        self.assertIsNone(c.depfile)
        self.assertIsNone(c.sysroot)
        self.assertIsNone(c.profile_list)
        self.assertEqual(list(c.input_files()), [source])
        self.assertEqual(list(c.output_files()), [output])
        self.assertEqual(list(c.output_dirs()), [])
        self.assertIsNone(c.crash_diagnostics_dir)
        self.assertEqual(c.preprocessed_output, i_file)
        preprocess, compile = c.split_preprocessing()
        self.assertEqual(
            preprocess,
            _strs(["clang", "-c", source, "-o", i_file, "-E", "-fno-blocks"]),
        )
        self.assertEqual(
            compile,
            _strs(["clang", "-c", i_file, "-o", output]),
        )

    def test_clang_c_save_temps(self) -> None:
        source = Path("hello.c")
        i_file = Path("obj/path/to/hello.i")
        output = Path("obj/path/to/hello.o")
        c = cxx.CxxAction(
            _strs(["clang", "-c", source, "-o", output, "--save-temps"])
        )
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=Path("clang"), type=cxx.Compiler.CLANG),
        )
        self.assertTrue(c.compiler_is_clang)
        self.assertFalse(c.compiler_is_gcc)
        self.assertTrue(c.save_temps)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.C)]
        )
        self.assertFalse(c.dialect_is_cxx)
        self.assertTrue(c.dialect_is_c)
        self.assertIsNone(c.depfile)
        self.assertIsNone(c.sysroot)
        self.assertIsNone(c.profile_list)
        self.assertEqual(list(c.input_files()), [source])
        self.assertEqual(
            list(c.output_files()),
            [
                output,
                # This .i file is implicitly output by the compiler,
                # but not the same as what we explicitly choose with ii_file.
                Path("hello.i"),
                Path("hello.bc"),
                Path("hello.s"),
            ],
        )
        self.assertEqual(list(c.output_dirs()), [])
        self.assertIsNone(c.crash_diagnostics_dir)
        self.assertEqual(c.preprocessed_output, i_file)
        preprocess, compile = c.split_preprocessing()
        self.assertEqual(
            preprocess,
            _strs(["clang", "-c", source, "-o", i_file, "-E", "-fno-blocks"]),
        )
        self.assertEqual(
            compile,
            _strs(["clang", "-c", i_file, "-o", output, "--save-temps"]),
        )

    def test_simple_clang_asm(self) -> None:
        source = Path("hello.s")
        output = Path("hello.o")
        c = cxx.CxxAction(_strs(["clang", "-c", source, "-o", output]))
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=Path("clang"), type=cxx.Compiler.CLANG),
        )
        self.assertTrue(c.compiler_is_clang)
        self.assertFalse(c.compiler_is_gcc)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.ASM)]
        )
        self.assertFalse(c.dialect_is_cxx)
        self.assertFalse(c.dialect_is_c)
        self.assertIsNone(c.crash_diagnostics_dir)

    def test_simple_clang_asm_pp(self) -> None:
        source = Path("hello.S")
        output = Path("hello.o")
        c = cxx.CxxAction(_strs(["clang", "-c", source, "-o", output]))
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=Path("clang"), type=cxx.Compiler.CLANG),
        )
        self.assertTrue(c.compiler_is_clang)
        self.assertFalse(c.compiler_is_gcc)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.ASM)]
        )
        self.assertFalse(c.dialect_is_cxx)
        self.assertFalse(c.dialect_is_c)
        self.assertIsNone(c.crash_diagnostics_dir)

    def test_shared(self) -> None:
        source = Path("hello.o")
        output = Path("hello.so")
        c = cxx.CxxAction(_strs(["clang++", "-shared", source, "-o", output]))
        self.assertTrue(c.shared)
        self.assertEqual(c.linker_inputs, [source])

    def test_linker_script(self) -> None:
        source = Path("hello.ld")
        output = Path("hello.so")
        c = cxx.CxxAction(_strs(["clang++", "-shared", source, "-o", output]))
        self.assertTrue(c.shared)
        self.assertEqual(c.linker_inputs, [source])

    def test_linker_script_flag_direct(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("hello.ld")
        c = cxx.CxxAction(
            _strs(["clang++", "-T", script, source, "-o", output])
        )
        self.assertIn(script, list(c.linker_inputs_from_flags()))

    def test_linker_script_flag_fused(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("hello.ld")
        c = cxx.CxxAction(
            _strs(["clang++", f"-T{script}", source, "-o", output])
        )
        self.assertIn(script, list(c.linker_inputs_from_flags()))

    def test_linker_script_flag_no_ext(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("hello_script")
        c = cxx.CxxAction(
            _strs(["clang++", "-T", script, source, "-o", output])
        )
        self.assertIn(script, list(c.linker_inputs_from_flags()))

    def test_linker_script_via_Wl_T(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("hello.ld")
        c = cxx.CxxAction(
            _strs(["clang++", f"-Wl,-T,{script}", source, "-o", output])
        )
        self.assertIn(script, list(c.linker_inputs_from_flags()))

    def test_linker_script_via_Wl_script(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("hello.ld")
        c = cxx.CxxAction(
            _strs(["clang++", f"-Wl,--script={script}", source, "-o", output])
        )
        self.assertIn(script, list(c.linker_inputs_from_flags()))

    def test_linker_script_symbol_assignment(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("hello.ld")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-T",
                    "foo=0x123",
                    "-T",
                    script,
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(list(c.linker_inputs_from_flags()), [script])

    def test_complex_Wl_flag(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("my_script.ld")
        flag = f"-Wl,-T{script},-defsym=foo=bar"
        c = cxx.CxxAction(_strs(["clang++", flag, source, "-o", output]))
        self.assertIn(script, list(c.linker_inputs_from_flags()))

    def test_complex_Wl_flag_not_first(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        script = Path("my_script.ld")
        flag = f"-Wl,-defsym=foo=bar,-T{script}"
        c = cxx.CxxAction(_strs(["clang++", flag, source, "-o", output]))
        self.assertIn(script, list(c.linker_inputs_from_flags()))

    def test_flto_default(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(_strs(["clang++", "-flto", source, "-o", output]))
        self.assertEqual(c.lto, "full")

    def test_flto_full(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-flto=full", source, "-o", output])
        )
        self.assertEqual(c.lto, "full")

    def test_flto_thin(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-flto=thin", source, "-o", output])
        )
        self.assertEqual(c.lto, "thin")

    def test_fuse_ld(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        linker = "lld"
        c = cxx.CxxAction(
            _strs(["clang++", f"-fuse-ld={linker}", source, "-o", output])
        )
        self.assertEqual(c.use_ld, linker)
        self.assertEqual(c.linker_inputs, [source])
        self.assertEqual(c.clang_linker_executable, "ld.lld")

    def test_fuse_ld_windows(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        linker = "lld"
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "--target=x64_64-windows-msvc",
                    f"-fuse-ld={linker}",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.use_ld, linker)
        self.assertEqual(c.linker_inputs, [source])
        self.assertEqual(c.clang_linker_executable, "lld-link")

    def test_fuse_ld_uefi(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        linker = "lld"
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "--target=x64_64-unknown-uefi",
                    f"-fuse-ld={linker}",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.use_ld, linker)
        self.assertEqual(c.linker_inputs, [source])
        self.assertEqual(c.clang_linker_executable, "lld-link")

    def test_asan(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-fsanitize=address", source, "-o", output])
        )
        self.assertEqual(c.sanitizers, {"address"})
        self.assertTrue(c.using_asan)
        self.assertFalse(c.using_ubsan)
        self.assertEqual(c.linker_inputs, [source])

    def test_no_sanitize(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-fsanitize=address",
                    "-fno-sanitize=address",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.sanitizers, set())
        self.assertFalse(c.using_asan)
        self.assertFalse(c.using_ubsan)
        self.assertEqual(c.linker_inputs, [source])

    def test_ubsan(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-fsanitize=undefined", source, "-o", output])
        )
        self.assertEqual(c.sanitizers, {"undefined"})
        self.assertFalse(c.using_asan)
        self.assertTrue(c.using_ubsan)
        self.assertEqual(c.linker_inputs, [source])

    def test_libdirs(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(
                ["clang++", "-L../foo/foo", "-Lbar/bar", source, "-o", output]
            )
        )
        self.assertEqual(c.libdirs, [Path("../foo/foo"), Path("bar/bar")])

    def test_libs(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-lmath",
                    "-lscience",
                    source,
                    "-o",
                    output,
                    "-liberty",
                ]
            )
        )
        self.assertEqual(c.libs, ["math", "science", "iberty"])

    def test_unwindlib_equals_arg(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-unwindlib=libunwind", source, "-o", output])
        )
        self.assertEqual(c.unwindlib, "libunwind")

    def test_unwindlib_separate_arg(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-unwindlib", "libunwind", source, "-o", output])
        )
        self.assertEqual(c.unwindlib, "libunwind")

    def test_not_a_prefix_for_unwindlib(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        for unknown_flag in ("-u", "--u", "--un", "-un", "-uu"):
            c = cxx.CxxAction(
                _strs(["clang++", unknown_flag, source, "-o", output])
            )
            self.assertIsNone(c.unwindlib)
            self.assertIn(unknown_flag, c.uninterpreted_args)

    def test_not_a_prefix_for_rtlib(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        for unknown_flag in ("-r", "--r", "--rt", "-rt", "-rx"):
            c = cxx.CxxAction(
                _strs(["clang++", unknown_flag, source, "-o", output])
            )
            self.assertIsNone(c.rtlib)
            self.assertIn(unknown_flag, c.uninterpreted_args)

    def test_rtlib_equals_arg(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-rtlib=compiler-rt", source, "-o", output])
        )
        self.assertEqual(c.rtlib, "compiler-rt")

    def test_rtlib_separate_arg(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-rtlib",
                    "different-compiler-rt",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.rtlib, "different-compiler-rt")

    def test_static_libstdcxx(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        c = cxx.CxxAction(
            _strs(["clang++", "-static-libstdc++", source, "-o", output])
        )
        self.assertTrue(c.static_libstdcxx)

    def test_unknown_linker_driver_flag(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        flag = "--unknown-flag=foobar"
        c = cxx.CxxAction(
            _strs(["clang++", f"-Wl,{flag}", source, "-o", output])
        )
        self.assertEqual(c.linker_driver_flags, [flag])

    def test_linker_mapfile_double_dash(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        mapfile = Path("hello.map")
        c = cxx.CxxAction(
            _strs(["clang++", f"-Wl,--Map={mapfile}", source, "-o", output])
        )
        self.assertEqual(c.linker_mapfile, mapfile)
        self.assertEqual(list(c.linker_output_files()), [output, mapfile])

    def test_linker_mapfile_single_dash(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        mapfile = Path("hello.map")
        c = cxx.CxxAction(
            _strs(["clang++", f"-Wl,-Map={mapfile}", source, "-o", output])
        )
        self.assertEqual(c.linker_mapfile, mapfile)
        self.assertEqual(list(c.linker_output_files()), [output, mapfile])

    def test_linker_depfile(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        depfile = Path("hello.d")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    f"-Wl,--dependency-file={depfile}",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.linker_depfile, depfile)
        self.assertEqual(list(c.linker_output_files()), [output, depfile])

    def test_linker_pdb_output(self) -> None:
        source = Path("hello.o")
        output = Path("hello.efi")
        pdb = Path("hello.pdb")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "--target=x86_x64-windows-msvc",
                    f"-Wl,/debug:full",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.pdb, pdb)
        self.assertEqual(list(c.linker_output_files()), [output, pdb])

    def test_linker_pdb_output_uefi(self) -> None:
        source = Path("hello.o")
        output = Path("hello.efi")
        pdb = Path("hello.pdb")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "--target=x86_x64-unknown-uefi",
                    f"-Wl,/debug:full",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.pdb, pdb)
        self.assertEqual(list(c.linker_output_files()), [output, pdb])

    def test_linker_retain_symbols_file(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        retain_file = Path("hello.allowlist")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    f"-Wl,--retain-symbols-file={retain_file}",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.linker_retain_symbols_file, retain_file)
        self.assertEqual(list(c.linker_inputs_from_flags()), [retain_file])

    def test_linker_version_script(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        version_script = Path("hello.ld")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    f"-Wl,--version-script={version_script}",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.linker_version_script, version_script)
        self.assertEqual(list(c.linker_inputs_from_flags()), [version_script])

    def test_linker_version_script_with_response_file(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        version_script = Path("hello.ld")
        rsp_file = Path("hello.rsp")
        with mock.patch.object(
            Path, "read_text", return_value=str(version_script)
        ) as mock_read:
            c = cxx.CxxAction(
                _strs(
                    [
                        "clang++",
                        f"-Wl,--version-script,@{rsp_file}",
                        source,
                        "-o",
                        output,
                    ]
                )
            )
        self.assertEqual(c.linker_version_script, version_script)
        self.assertEqual(list(c.linker_inputs_from_flags()), [version_script])
        self.assertEqual(c.linker_response_files, [rsp_file])

    def test_linker_just_symbols(self) -> None:
        source = Path("hello.o")
        output = Path("hello")
        symbols = Path("hello.elf")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    f"-Wl,--just-symbols={symbols}",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.linker_just_symbols, symbols)
        self.assertEqual(list(c.linker_inputs_from_flags()), [symbols])

    def test_simple_gcc_cxx(self) -> None:
        source = Path("hello.cc")
        output = Path("hello.o")
        ii_file = Path("hello.ii")
        c = cxx.CxxAction(_strs(["g++", "-c", source, "-o", output]))
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=Path("g++"), type=cxx.Compiler.GCC),
        )
        self.assertFalse(c.compiler_is_clang)
        self.assertTrue(c.compiler_is_gcc)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.CXX)]
        )
        self.assertTrue(c.dialect_is_cxx)
        self.assertFalse(c.dialect_is_c)
        self.assertIsNone(c.depfile)
        self.assertIsNone(c.crash_diagnostics_dir)
        self.assertEqual(c.preprocessed_output, ii_file)
        preprocess, compile = c.split_preprocessing()
        self.assertEqual(
            preprocess,
            _strs(["g++", "-c", source, "-o", ii_file, "-E"]),
        )
        self.assertEqual(
            compile,
            _strs(["g++", "-c", ii_file, "-o", output]),
        )

    def test_simple_gcc_c(self) -> None:
        source = Path("hello.c")
        i_file = Path("hello.i")
        output = Path("hello.o")
        c = cxx.CxxAction(_strs(["gcc", "-c", source, "-o", output]))
        self.assertEqual(c.output_file, output)
        self.assertEqual(
            c.compiler,
            cxx.CompilerTool(tool=Path("gcc"), type=cxx.Compiler.GCC),
        )
        self.assertFalse(c.compiler_is_clang)
        self.assertTrue(c.compiler_is_gcc)
        self.assertEqual(c.target, "")
        self.assertEqual(
            c.sources, [cxx.Source(file=source, dialect=cxx.SourceLanguage.C)]
        )
        self.assertFalse(c.dialect_is_cxx)
        self.assertTrue(c.dialect_is_c)
        self.assertIsNone(c.depfile)
        self.assertIsNone(c.crash_diagnostics_dir)
        self.assertEqual(c.preprocessed_output, i_file)
        preprocess, compile = c.split_preprocessing()
        self.assertEqual(
            preprocess,
            _strs(["gcc", "-c", source, "-o", i_file, "-E"]),
        )
        self.assertEqual(
            compile,
            _strs(["gcc", "-c", i_file, "-o", output]),
        )

    def test_clang_target(self) -> None:
        c = cxx.CxxAction(
            [
                "clang++",
                "--target=powerpc-apple-darwin8",
                "-c",
                "hello.cc",
                "-o",
                "hello.o",
            ]
        )
        self.assertEqual(c.target, "powerpc-apple-darwin8")

    def test_clang_crash_diagnostics_dir(self) -> None:
        crash_dir = Path("/tmp/graveyard")
        c = cxx.CxxAction(
            [
                "clang++",
                f"-fcrash-diagnostics-dir={crash_dir}",
                "-c",
                "hello.cc",
                "-o",
                "hello.o",
            ]
        )
        self.assertEqual(c.crash_diagnostics_dir, crash_dir)
        self.assertEqual(set(c.output_dirs()), {crash_dir})

    def test_profile_list(self) -> None:
        source = Path("hello.cc")
        output = Path("hello.o")
        profile = Path("my/online/profile.list")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-c",
                    source,
                    "-o",
                    output,
                    f"-fprofile-list={profile}",
                ]
            )
        )
        self.assertEqual(c.profile_list, profile)
        self.assertEqual(set(c.input_files()), {source, profile})

    def test_profile_generate(self) -> None:
        source = Path("hello.cc")
        output = Path("hello.o")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-c",
                    source,
                    "-o",
                    output,
                    "-fprofile-generate",
                ]
            )
        )
        self.assertEqual(c.profile_generate, Path("."))

    def test_profile_generate_with_dir(self) -> None:
        source = Path("hello.cc")
        output = Path("hello.o")
        pdir = Path("my/pg/dir")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-c",
                    source,
                    "-o",
                    output,
                    f"-fprofile-generate={pdir}",
                ]
            )
        )
        self.assertEqual(c.profile_generate, pdir)

    def test_profile_instr_generate(self) -> None:
        source = Path("hello.cc")
        output = Path("hello.o")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-c",
                    source,
                    "-o",
                    output,
                    "-fprofile-instr-generate",
                ]
            )
        )
        self.assertEqual(c.profile_instr_generate, Path("default.profraw"))

    def test_profile_instr_generate_with_file(self) -> None:
        source = Path("hello.cc")
        output = Path("hello.o")
        pfile = Path("pg/path/to/file.profraw")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-c",
                    source,
                    "-o",
                    output,
                    f"-fprofile-instr-generate={pfile}",
                ]
            )
        )
        self.assertEqual(c.profile_instr_generate, pfile)

    def test_uses_macos_sdk(self) -> None:
        sysroot = Path("/Library/Developer/blah")
        c = cxx.CxxAction(
            [
                "clang++",
                f"--sysroot={sysroot}",
                "-c",
                "hello.cc",
                "-o",
                "hello.o",
            ]
        )
        self.assertEqual(c.sysroot, sysroot)
        self.assertTrue(c.uses_macos_sdk)

    def test_split_preprocessing(self) -> None:
        source = Path("hello.cc")
        ii_file = Path("hello.ii")
        output = Path("hello.o")
        depfile = Path("hello.d")
        c = cxx.CxxAction(
            _strs(
                [
                    "clang++",
                    "-DNDEBUG",
                    "-I/opt/include",
                    "-stdlib=libc++",
                    "-M",
                    "-MF",
                    depfile,
                    "-c",
                    source,
                    "-o",
                    output,
                ]
            )
        )
        self.assertEqual(c.depfile, depfile)
        preprocess, compile = c.split_preprocessing()
        self.assertEqual(
            preprocess,
            _strs(
                [
                    "clang++",
                    "-DNDEBUG",
                    "-I/opt/include",
                    "-stdlib=libc++",
                    "-M",
                    "-MF",
                    depfile,
                    "-c",
                    source,
                    "-o",
                    ii_file,
                    "-E",
                    "-fno-blocks",
                ]
            ),
        )
        self.assertEqual(
            compile,
            _strs(["clang++", "-c", ii_file, "-o", output]),
        )


if __name__ == "__main__":
    unittest.main()
