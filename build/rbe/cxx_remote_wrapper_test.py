#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Sequence
from pathlib import Path
from typing import Any
from unittest import mock

import cl_utils
import cxx
import cxx_remote_wrapper
import fuchsia
import remote_action

_temp_dir = None
_orig_cwd = None


def setUpModule() -> None:
    # When running as a build-time test, relative path writes (such as generating
    # auxiliary '.inputs' files) would otherwise write to the build output directory
    # and violate GN action hermeticity.
    # To prevent this, we run all tests in this module inside a temporary directory.
    global _temp_dir, _orig_cwd
    _orig_cwd = os.getcwd()
    _temp_dir = tempfile.TemporaryDirectory()
    os.chdir(_temp_dir.name)


def tearDownModule() -> None:
    # Restore the original working directory and clean up the temporary directory.
    global _temp_dir, _orig_cwd
    if _orig_cwd:
        os.chdir(_orig_cwd)
    if _temp_dir:
        _temp_dir.cleanup()


class ImmediateExit(Exception):
    """For mocking functions that do not return."""


def _strs(items: Sequence[Any]) -> Sequence[str]:
    return [str(i) for i in items]


def _paths(items: Sequence[Any]) -> Sequence[Path] | set[Path]:
    if isinstance(items, list):
        return [Path(i) for i in items]
    elif isinstance(items, set):
        return {Path(i) for i in items}
    elif isinstance(items, tuple):
        return tuple(Path(i) for i in items)

    t = type(items)
    raise TypeError(f"Unhandled sequence type: {t}")


class CheckMissingRemoteToolsTests(unittest.TestCase):
    _PROJECT_ROOT = Path("..", "..", "fake", "project", "root")

    def test_clang_exists(self) -> None:
        with mock.patch.object(Path, "is_dir", return_value=True):
            self.assertEqual(
                list(
                    cxx_remote_wrapper.check_missing_remote_tools(
                        cxx.Compiler.CLANG, self._PROJECT_ROOT
                    )
                ),
                [],
            )

    def test_clang_not_exists(self) -> None:
        with mock.patch.object(Path, "is_dir", return_value=False):
            self.assertEqual(
                list(
                    cxx_remote_wrapper.check_missing_remote_tools(
                        cxx.Compiler.CLANG, self._PROJECT_ROOT
                    )
                ),
                [fuchsia.REMOTE_CLANG_SUBDIR],
            )

    def test_gcc_exists(self) -> None:
        with mock.patch.object(Path, "is_dir", return_value=True):
            self.assertEqual(
                list(
                    cxx_remote_wrapper.check_missing_remote_tools(
                        cxx.Compiler.GCC, self._PROJECT_ROOT
                    )
                ),
                [],
            )

    def test_gcc_not_exists(self) -> None:
        with mock.patch.object(Path, "is_dir", return_value=False):
            self.assertEqual(
                list(
                    cxx_remote_wrapper.check_missing_remote_tools(
                        cxx.Compiler.GCC, self._PROJECT_ROOT
                    )
                ),
                [fuchsia.REMOTE_GCC_SUBDIR],
            )


class CxxRemoteActionTests(unittest.TestCase):
    def test_clang_cxx(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/really-not-default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        source = Path("hello.cc")
        output = Path("hello.o")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                "-c",
                source,
                "-o",
                output,
            ]
        )
        c = cxx_remote_wrapper.CxxRemoteAction(
            ["--", *command],
            exec_root=fake_root,
            working_dir=fake_cwd,
            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
            auto_reproxy=False,
        )
        self.assertFalse(c.verbose)
        self.assertFalse(c.dry_run)
        self.assertEqual(c.cxx_action.compiler.tool, compiler)
        self.assertTrue(c.cxx_action.compiler_is_clang)
        self.assertEqual(c.cxx_action.output_file, output)
        self.assertEqual(c.cxx_action.target, "riscv64-apple-darwin21")
        self.assertEqual(c.cpp_strategy, "integrated")
        self.assertEqual(c.original_compile_command, command)
        self.assertFalse(c.local_only)

        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(
                fuchsia,
                "remote_clang_compiler_toolchain_inputs",
                return_value=iter([]),
            ) as mock_tc_inputs:
                self.assertEqual(c.prepare(), 0)
        self.assertEqual(
            c.remote_action.inputs_relative_to_project_root,
            [fake_builddir / source],
        )
        with mock.patch.object(
            cxx_remote_wrapper.CxxRemoteAction,
            "_run_remote_action",
            return_value=0,
        ) as mock_call:
            exit_code = c.run()
        self.assertEqual(exit_code, 0)
        mock_call.assert_called_once()

    def test_pp_asm_local_only(self) -> None:
        compiler = Path("clang++")
        source = Path("hello.S")
        output = Path("hello.o")
        exec_root = Path("/exec/root")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                "-c",
                source,
                "-o",
                output,
            ]
        )
        c = cxx_remote_wrapper.CxxRemoteAction(
            ["--", *command],
            auto_reproxy=False,
            exec_root=exec_root,
            working_dir=exec_root / "work",
        )
        self.assertTrue(c.local_only)
        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(
                subprocess, "call", return_value=0
            ) as mock_call:
                exit_code = c.run()
        self.assertEqual(exit_code, 0)
        mock_call.assert_called_with(command, cwd=c.working_dir)  # ran locally

    def test_objc_local_only(self) -> None:
        compiler = Path("clang++")
        source = Path("hello.mm")
        output = Path("hello.o")
        exec_root = Path("/exec/root")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                "-c",
                source,
                "-o",
                output,
            ]
        )
        c = cxx_remote_wrapper.CxxRemoteAction(
            ["--", *command],
            auto_reproxy=False,
            exec_root=exec_root,
            working_dir=exec_root / "work",
        )
        self.assertTrue(c.local_only)
        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(
                subprocess, "call", return_value=0
            ) as mock_call:
                exit_code = c.run()
        self.assertEqual(exit_code, 0)
        mock_call.assert_called_with(command, cwd=c.working_dir)  # ran locally

    def test_remote_action_paths(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/not-default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        source = Path("hello.cc")
        output = Path("hello.o")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                "-c",
                source,
                "-o",
                output,
            ]
        )
        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(
                fuchsia,
                "remote_clang_compiler_toolchain_inputs",
                return_value=iter([]),
            ) as mock_tc_inputs:
                with mock.patch.object(os, "curdir", fake_cwd):
                    with mock.patch.object(
                        remote_action, "PROJECT_ROOT", fake_root
                    ):
                        c = cxx_remote_wrapper.CxxRemoteAction(
                            ["--", *command],
                            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
                            auto_reproxy=False,
                        )
                        self.assertEqual(c.prepare(), 0)
                        self.assertEqual(c.remote_action.exec_root, fake_root)
                        self.assertEqual(
                            c.remote_action.build_subdir, fake_builddir
                        )

    def test_clang_crash_diagnostics_dir(self) -> None:
        fake_root = Path("/usr/project")
        fake_builddir = Path("build-it")
        fake_cwd = fake_root / fake_builddir
        crash_dir = Path("boom/b00m")
        compiler = Path("clang++")
        source = Path("hello.cc")
        output = Path("hello.o")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                f"-fcrash-diagnostics-dir={crash_dir}",
                "-c",
                source,
                "-o",
                output,
            ]
        )
        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(remote_action, "PROJECT_ROOT", fake_root):
                c = cxx_remote_wrapper.CxxRemoteAction(
                    ["--", *command],
                    working_dir=fake_cwd,
                    host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
                    auto_reproxy=False,
                )
                with mock.patch.object(
                    fuchsia,
                    "remote_clang_compiler_toolchain_inputs",
                    return_value=iter([]),
                ) as mock_tc_inputs:
                    self.assertEqual(c.prepare(), 0)

                self.assertTrue(c.cxx_action.compiler_is_clang)
                self.assertEqual(c.remote_action.exec_root, fake_root)
                self.assertEqual(c.remote_action.build_subdir, fake_builddir)
                self.assertEqual(
                    c.remote_action.output_dirs_relative_to_working_dir,
                    [crash_dir],
                )
                self.assertEqual(
                    c.remote_action.output_dirs_relative_to_project_root,
                    [fake_builddir / crash_dir],
                )
                mock_tc_inputs.assert_called_once()

    def test_remote_flag_back_propagating(self) -> None:
        compiler = Path("clang++")
        source = Path("hello.cc")
        output = Path("hello.o")
        flag = "--foo-bar"
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                f"--remote-flag={flag}",
                "-c",
                source,
                "-o",
                output,
            ]
        )
        filtered_command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                "-c",
                source,
                "-o",
                output,
            ]
        )

        c = cxx_remote_wrapper.CxxRemoteAction(
            ["--", *command],
            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
            auto_reproxy=False,
        )

        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(
                fuchsia,
                "remote_clang_compiler_toolchain_inputs",
                return_value=iter([]),
            ) as mock_tc_inputs:
                self.assertEqual(c.prepare(), 0)
        # check that rewrapper option sees --foo=bar
        remote_action_command = c.remote_action.launch_command
        prefix, sep, wrapped_command = cl_utils.partition_sequence(
            remote_action_command, "--"
        )
        self.assertIn(flag, prefix)
        self.assertEqual(wrapped_command, filtered_command)

    def test_gcc_cxx(self) -> None:
        fake_root = remote_action.PROJECT_ROOT
        fake_builddir = Path("make-it-so")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("g++")
        source = Path("hello.cc")
        output = Path("hello.o")
        depfile = Path("hello.d")
        command = _strs([compiler, "-MF", depfile, "-c", source, "-o", output])
        c = cxx_remote_wrapper.CxxRemoteAction(
            ["--", *command],
            working_dir=fake_cwd,
            exec_root=fake_root,
            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
            auto_reproxy=False,
        )
        self.assertFalse(c.verbose)
        self.assertFalse(c.dry_run)
        self.assertEqual(c.cxx_action.compiler.tool, compiler)
        self.assertTrue(c.cxx_action.compiler_is_gcc)
        self.assertEqual(c.cxx_action.output_file, output)
        self.assertEqual(c.cpp_strategy, "integrated")
        self.assertEqual(c.original_compile_command, command)
        self.assertFalse(c.local_only)
        self.assertEqual(c.depfile, depfile)

        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(
                fuchsia, "gcc_support_tools", return_value=iter([])
            ) as mock_tools:
                self.assertEqual(c.prepare(), 0)
        mock_tools.assert_called_with(
            c.compiler_path, parser=True, assembler=True
        )
        self.assertEqual(
            c.remote_action.inputs_relative_to_project_root,
            [fake_builddir / source],
        )
        self.assertEqual(
            c.remote_action.output_files_relative_to_project_root,
            [fake_builddir / output, fake_builddir / depfile],
        )
        self.assertEqual(
            set(c.remote_action.expected_downloads), {output, depfile}
        )

        with mock.patch.object(
            remote_action.RemoteAction,
            "_run_maybe_remotely",
            return_value=cl_utils.SubprocessResult(0),
        ) as mock_remote:
            with mock.patch.object(
                remote_action.RemoteAction,
                "_write_output_file_hash_xattrs",
            ) as mock_write_xattrs:
                exit_code = c.run()

        self.assertEqual(exit_code, 0)

    def test_rewrite_remote_depfile(self) -> None:
        compiler = Path("ppc-macho-g++")
        source = Path("hello.cc")
        output = Path("hello.o")
        depfile = Path("hello.d")
        with tempfile.TemporaryDirectory() as td:
            fake_root = Path(td)
            fake_builddir = Path("make-it-so")
            fake_cwd = fake_root / fake_builddir
            command = _strs(
                [compiler, "-MF", depfile, "-c", source, "-o", output]
            )
            c = cxx_remote_wrapper.CxxRemoteAction(
                # For this test, make the remote/local working dirs match
                ["--canonicalize_working_dir=false", "--", *command],
                working_dir=fake_cwd,
                exec_root=fake_root,
                host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
                auto_reproxy=False,
            )

            remote_cwd = remote_action._REMOTE_PROJECT_ROOT / fake_builddir
            fake_cwd.mkdir(parents=True, exist_ok=True)
            (fake_cwd / depfile).write_text(
                f"{remote_cwd}/lib/bar.a: {remote_cwd}/obj/foo.o\n",
            )

            with mock.patch.object(
                cxx_remote_wrapper, "check_missing_remote_tools"
            ) as mock_check:
                # create the remote action
                self.assertEqual(c.prepare(), 0)

            mock_check.assert_called_once()
            self.assertEqual(
                set(c.remote_action.expected_downloads), {output, depfile}
            )
            self.assertEqual(c.remote_action.remote_working_dir, remote_cwd)
            c._rewrite_remote_depfile()
            new_depfile = (fake_cwd / depfile).read_text()
            self.assertEqual(new_depfile, "lib/bar.a: obj/foo.o\n")

    def test_fsatrace_compare(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/really-not-default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        source = Path("hello.cc")
        output = Path("hello.o")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin22",
                "-c",
                source,
                "-o",
                output,
            ]
        )
        c = cxx_remote_wrapper.CxxRemoteAction(
            ["--compare", "--fsatrace-path=path/to/fsatrace", "--", *command],
            exec_root=fake_root,
            working_dir=fake_cwd,
            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
            auto_reproxy=False,
        )
        self.assertFalse(c.verbose)
        self.assertFalse(c.dry_run)
        self.assertEqual(c.cxx_action.compiler.tool, compiler)
        self.assertTrue(c.cxx_action.compiler_is_clang)
        self.assertEqual(c.cxx_action.output_file, output)
        self.assertEqual(c.cxx_action.target, "riscv64-apple-darwin22")
        self.assertEqual(c.cpp_strategy, "integrated")
        self.assertEqual(c.original_compile_command, command)
        self.assertFalse(c.local_only)

        with mock.patch.object(
            cxx_remote_wrapper, "check_missing_remote_tools"
        ) as mock_check:
            with mock.patch.object(
                fuchsia,
                "remote_clang_compiler_toolchain_inputs",
                return_value=iter([]),
            ) as mock_tc_inputs:
                self.assertEqual(c.prepare(), 0)
        # C++: cannot use "prefix" method of remote fsatrace-ing.
        self.assertEqual(c.remote_action._remote_fsatrace_method, "wrap")
        self.assertIn(
            fake_builddir / source,
            c.remote_action.inputs_relative_to_project_root,
            # includes other fsatrace support files
        )
        with mock.patch.object(
            cxx_remote_wrapper.CxxRemoteAction,
            "_run_remote_action",
            return_value=0,
        ) as mock_call:
            exit_code = c.run()
        self.assertEqual(exit_code, 0)
        mock_call.assert_called_once()


class MainTests(unittest.TestCase):
    def test_help_implicit(self) -> None:
        # Just make sure help exits successfully, without any exceptions
        # due to argument parsing.
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                sys, "exit", side_effect=ImmediateExit
            ) as mock_exit:
                with self.assertRaises(ImmediateExit):
                    cxx_remote_wrapper.main([])
        mock_exit.assert_called_with(0)

    def test_help_flag(self) -> None:
        # Just make sure help exits successfully, without any exceptions
        # due to argument parsing.
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            with mock.patch.object(
                sys, "exit", side_effect=ImmediateExit
            ) as mock_exit:
                with self.assertRaises(ImmediateExit):
                    cxx_remote_wrapper.main(["--help"])
        mock_exit.assert_called_with(0)

    def test_local_mode_forced(self) -> None:
        exit_code = 24
        with mock.patch.object(
            remote_action, "auto_relaunch_with_reproxy"
        ) as mock_relaunch:
            with mock.patch.object(
                cxx_remote_wrapper.CxxRemoteAction,
                "_run_locally",
                return_value=exit_code,
            ) as mock_run:
                self.assertEqual(
                    cxx_remote_wrapper.main(
                        [
                            "--local",
                            "--",
                            "clang++",
                            "-c",
                            "foo.cc",
                            "-o",
                            "foo.o",
                        ]
                    ),
                    exit_code,
                )
        mock_relaunch.assert_called_once()
        mock_run.assert_called_with()

    def test_auto_relaunched_with_reproxy(self) -> None:
        argv = ["--", "clang++", "-c", "foo.cc", "-o", "foo.o"]
        with mock.patch.object(
            os.environ, "get", return_value=None
        ) as mock_env:
            with mock.patch.object(
                cl_utils, "exec_relaunch", side_effect=ImmediateExit
            ) as mock_relaunch:
                with self.assertRaises(ImmediateExit):
                    cxx_remote_wrapper.main(argv)
        mock_env.assert_called()
        mock_relaunch.assert_called_once()
        args, kwargs = mock_relaunch.call_args_list[0]
        relaunch_cmd = args[0]
        self.assertEqual(relaunch_cmd[0], str(fuchsia.REPROXY_WRAP))
        cmd_slices = cl_utils.split_into_subsequences(relaunch_cmd[1:], "--")
        reproxy_args, self_script, wrapped_command = cmd_slices
        self.assertEqual(reproxy_args, ["-v"])
        self.assertIn("python", self_script[0])
        self.assertTrue(self_script[-1].endswith("cxx_remote_wrapper.py"))
        self.assertEqual(wrapped_command, argv[1:])


if __name__ == "__main__":
    remote_action.init_from_main_once()
    unittest.main()
