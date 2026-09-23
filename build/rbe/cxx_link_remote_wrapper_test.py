#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import os
import sys
import tempfile
import unittest
from collections.abc import Collection, Iterable
from pathlib import Path
from typing import Any
from unittest import mock

import cl_utils
import cxx_link_remote_wrapper
import fuchsia
import linker
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


def _strs(items: Collection[Any]) -> Collection[str]:
    return [str(i) for i in items]


def _paths(items: Collection[Any]) -> Collection[Path]:
    if isinstance(items, list):
        return [Path(i) for i in items]
    elif isinstance(items, set):
        return {Path(i) for i in items}
    elif isinstance(items, tuple):
        return tuple(Path(i) for i in items)

    t = type(items)
    raise TypeError(f"Unhandled sequence type: {t}")


class CxxLinkRemoteActionTests(unittest.TestCase):
    def test_clang_cxx_link(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/really-not-default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        source = Path("hello.o")
        output = Path("hello.a")
        target = "riscv64-apple-darwin21"
        sysroot = Path("/path/to/sys/root")
        command = _strs(
            [
                compiler,
                "--sysroot",
                sysroot,
                f"--target={target}",
                source,
                "-o",
                output,
            ]
        )
        c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
            ["--", *command],
            exec_root=fake_root,
            working_dir=fake_cwd,
            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
            auto_reproxy=False,
        )
        self.assertFalse(c.verbose)
        self.assertFalse(c.dry_run)
        self.assertEqual(c.host_compiler, compiler)
        self.assertTrue(c.cxx_action.compiler_is_clang)
        self.assertEqual(c.cxx_action.output_file, output)
        self.assertEqual(c.sysroot, sysroot)
        self.assertEqual(c.target, target)
        self.assertEqual(c.original_link_command, command)
        self.assertEqual(c.primary_output, output)
        self.assertFalse(c.local_only)

        with mock.patch.object(
            fuchsia, "remote_clang_linker_toolchain_inputs", return_value=[]
        ):
            self.assertEqual(c.prepare(), 0)
        self.assertEqual(
            c.cxx_action.linker_inputs,
            [source],
        )
        self.assertEqual(
            c.remote_action.inputs_relative_to_project_root,
            # re-client will automatically collect named linker inputs,
            # so they need not be listed here.
            [fake_builddir / source],
        )
        with mock.patch.object(
            cxx_link_remote_wrapper.CxxLinkRemoteAction,
            "_run_remote_action",
            return_value=0,
        ) as mock_call:
            exit_code = c.run()
        self.assertEqual(exit_code, 0)
        mock_call.assert_called_once()

    def test_linker_direct_files(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/really-not-default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        sources = [
            Path("hello.o"),
            Path("baz.a"),
            Path("bar.so"),
            Path("foo/bar.ld"),
        ]
        output = Path("hello.a")
        target = "riscv64-apple-darwin21"
        command = _strs(
            [
                compiler,
                f"--target={target}",
                *sources,
                "-o",
                output,
            ]
        )
        c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
            ["--", *command],
            exec_root=fake_root,
            working_dir=fake_cwd,
            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
            auto_reproxy=False,
        )
        linker = c._linker_invocation()
        self.assertEqual(linker.direct_files, sources)

    def test_linker_scripts_from_flags(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        source = Path("hello.o")
        script = Path("my_script.ld")
        output = Path("hello")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                "-T",
                script,
                source,
                "-o",
                output,
            ]
        )
        c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
            ["--", *command],
            exec_root=fake_root,
            working_dir=fake_cwd,
            host_platform=fuchsia.REMOTE_PLATFORM,
            auto_reproxy=False,
        )
        linker_invoc = c._linker_invocation()
        self.assertIn(script, linker_invoc.unscanned_direct_files)

    def test_linker_scripts_from_Wl_flags(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        source = Path("hello.o")
        script = Path("my_script.ld")
        output = Path("hello")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                f"-Wl,-T,{script}",
                source,
                "-o",
                output,
            ]
        )
        c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
            ["--", *command],
            exec_root=fake_root,
            working_dir=fake_cwd,
            host_platform=fuchsia.REMOTE_PLATFORM,
            auto_reproxy=False,
        )
        linker_invoc = c._linker_invocation()
        self.assertIn(script, linker_invoc.unscanned_direct_files)

    def test_remote_action_paths(self) -> None:
        fake_root = Path("/home/project")
        fake_builddir = Path("out/not-default")
        fake_cwd = fake_root / fake_builddir
        compiler = Path("clang++")
        source = Path("hello.o")
        output = Path("hello")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                source,
                "-o",
                output,
            ]
        )
        with mock.patch.object(os, "curdir", fake_cwd):
            with mock.patch.object(remote_action, "PROJECT_ROOT", fake_root):
                c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
                    ["--", *command],
                    host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
                    auto_reproxy=False,
                )
                with mock.patch.object(
                    fuchsia,
                    "remote_clang_linker_toolchain_inputs",
                    return_value=[],
                ):
                    self.assertEqual(c.prepare(), 0)
                self.assertEqual(c.remote_action.exec_root, fake_root)
                self.assertEqual(c.remote_action.build_subdir, fake_builddir)

    def test_clang_crash_diagnostics_dir(self) -> None:
        fake_root = Path("/usr/project")
        fake_builddir = Path("build-it")
        fake_cwd = fake_root / fake_builddir
        crash_dir = Path("boom/b00m")
        compiler = Path("clang++")
        source = Path("hello.o")
        output = Path("hello")
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                f"-fcrash-diagnostics-dir={crash_dir}",
                source,
                "-o",
                output,
            ]
        )
        with mock.patch.object(remote_action, "PROJECT_ROOT", fake_root):
            c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
                ["--", *command],
                working_dir=fake_cwd,
                host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
                auto_reproxy=False,
            )
            with mock.patch.object(
                fuchsia, "remote_clang_linker_toolchain_inputs", return_value=[]
            ):
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

    def test_remote_flag_back_propagating(self) -> None:
        compiler = Path("clang++")
        source = Path("hello.o")
        output = Path("hello")
        flag = "--foo-bar"
        command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                f"--remote-flag={flag}",
                source,
                "-o",
                output,
            ]
        )
        filtered_command = _strs(
            [
                compiler,
                "--target=riscv64-apple-darwin21",
                source,
                "-o",
                output,
            ]
        )

        c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
            ["--", *command],
            host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
            auto_reproxy=False,
        )

        with mock.patch.object(
            fuchsia, "remote_clang_linker_toolchain_inputs", return_value=[]
        ):
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
        source = Path("hello.o")
        output = Path("hello")
        depfile = Path("hello.d")
        command = _strs(
            [compiler, f"-Wl,--dependency-file={depfile}", source, "-o", output]
        )
        c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
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
        self.assertEqual(c.original_link_command, command)
        self.assertFalse(c.local_only)
        self.assertEqual(c.depfile, depfile)

        with mock.patch.object(
            fuchsia, "remote_gcc_linker_toolchain_inputs", return_value=[]
        ):
            with mock.patch.object(
                fuchsia, "gcc_support_tools", return_value=iter([])
            ) as mock_tools:
                self.assertEqual(c.prepare(), 0)
        mock_tools.assert_called_with(c.compiler_path, linker=True)
        self.assertEqual(
            c.cxx_action.linker_inputs,
            [source],
        )
        self.assertEqual(
            c.remote_action.inputs_relative_to_project_root,
            # re-client will automatically collect named linker inputs,
            # so listing them here is redundant.
            [fake_builddir / source],
        )
        self.assertEqual(
            c.remote_action.output_files_relative_to_project_root,
            [
                fake_builddir / output,
                fake_builddir / depfile,
            ],
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
        source = Path("hello.o")
        output = Path("hello")
        depfile = Path("hello.d")
        with tempfile.TemporaryDirectory() as td:
            fake_root = Path(td)
            fake_builddir = Path("make-it-so")
            fake_cwd = fake_root / fake_builddir
            command = _strs(
                [
                    compiler,
                    f"-Wl,--dependency-file={depfile}",
                    source,
                    "-o",
                    output,
                ]
            )
            c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
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

            # create the remote action
            with mock.patch.object(
                fuchsia, "remote_gcc_linker_toolchain_inputs", return_value=[]
            ):
                self.assertEqual(c.prepare(), 0)

            self.assertEqual(
                set(c.remote_action.expected_downloads), {output, depfile}
            )
            self.assertEqual(c.remote_action.remote_working_dir, remote_cwd)
            c._rewrite_remote_depfile()
            new_depfile = (fake_cwd / depfile).read_text()
            self.assertEqual(new_depfile, "lib/bar.a: obj/foo.o\n")

    def test_clang_cxx_link_response_file(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            fake_root = Path("/home/project")
            fake_builddir = Path("out/really-not-default")
            fake_cwd = fake_root / fake_builddir
            compiler = Path("clang++")
            source = Path("hello.o")
            rspfile = tdp / "hello.rsp"
            rspfile.write_text(f"{source}\n")
            output = Path("hello.a")
            target = "riscv64-apple-darwin21"
            command = _strs(
                [
                    compiler,
                    f"--target={target}",
                    f"@{rspfile}",
                    "-o",
                    output,
                ]
            )
            c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
                ["--", *command],
                exec_root=fake_root,
                working_dir=fake_cwd,
                host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
                auto_reproxy=False,
            )

            with mock.patch.object(
                fuchsia, "remote_clang_linker_toolchain_inputs", return_value=[]
            ):
                self.assertEqual(c.prepare(), 0)
            self.assertEqual(
                c.cxx_action.linker_inputs,
                [source],
            )
            self.assertEqual(
                c.remote_action.inputs_relative_to_working_dir,
                [
                    source,  # from response file
                    rspfile,
                ],
            )

    def test_clang_cxx_link_scandeps(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            fake_root = Path("/home/project")
            fake_builddir = Path("out/really-not-default")
            fake_cwd = fake_root / fake_builddir
            compiler = Path("clang++")
            source = Path("hello.o")
            output = Path("hello.a")
            target = "riscv64-apple-darwin19"
            command = _strs(
                [
                    compiler,
                    f"--target={target}",
                    source,
                    "-o",
                    output,
                ]
            )
            c = cxx_link_remote_wrapper.CxxLinkRemoteAction(
                ["--scandeps", "--", *command],
                exec_root=fake_root,
                working_dir=fake_cwd,
                host_platform=fuchsia.REMOTE_PLATFORM,  # host = remote exec
                auto_reproxy=False,
            )
            self.assertTrue(c.scandeps)

            fake_scan_results = [Path("/path/to/usr/lib/libfoo.a")]
            with mock.patch.object(
                fuchsia, "remote_clang_linker_toolchain_inputs", return_value=[]
            ):
                with mock.patch.object(
                    linker.LinkerInvocation,
                    "expand_all",
                    return_value=fake_scan_results,
                ) as mock_scan:
                    self.assertEqual(c.prepare(), 0)
            self.assertEqual(
                c.cxx_action.linker_inputs,
                [source],
            )
            self.assertEqual(
                set(c.remote_action.inputs_relative_to_working_dir),
                set([source] + fake_scan_results),
            )
            mock_scan.assert_called_once_with()


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
                    cxx_link_remote_wrapper.main([])
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
                    cxx_link_remote_wrapper.main(["--help"])
        mock_exit.assert_called_with(0)

    def test_local_mode_forced(self) -> None:
        exit_code = 24
        with mock.patch.object(
            remote_action, "auto_relaunch_with_reproxy"
        ) as mock_relaunch:
            with mock.patch.object(
                cxx_link_remote_wrapper.CxxLinkRemoteAction,
                "_run_locally",
                return_value=exit_code,
            ) as mock_run:
                self.assertEqual(
                    cxx_link_remote_wrapper.main(
                        [
                            "--local",
                            "--",
                            "clang++",
                            "foo.o",
                            "-o",
                            "foo",
                        ]
                    ),
                    exit_code,
                )
        mock_relaunch.assert_called_once()
        mock_run.assert_called_with()

    def test_scandeps_flag(self) -> None:
        exit_code = 0
        with mock.patch.object(
            remote_action, "auto_relaunch_with_reproxy"
        ) as mock_relaunch:
            with mock.patch.object(
                cxx_link_remote_wrapper.CxxLinkRemoteAction,
                "_run_remote_action",
                return_value=exit_code,
            ) as mock_run:

                def fake_scan_results() -> Iterable[Path]:
                    yield from []

                with mock.patch.object(
                    cxx_link_remote_wrapper.CxxLinkRemoteAction,
                    "scan_linker_inputs",
                    side_effect=fake_scan_results,
                ) as mock_scan:
                    with mock.patch.object(
                        fuchsia,
                        "remote_clang_linker_toolchain_inputs",
                        return_value=[],
                    ):  # don't care about toolchain inputs
                        self.assertEqual(
                            cxx_link_remote_wrapper.main(
                                [
                                    "--",
                                    "clang++",
                                    "foo.o",
                                    "-o",
                                    "foo",
                                    "--target=powerpc64-apple-darwin9",
                                    # This gets sifted to the outer argparser.
                                    "--remote-flag=--scandeps",
                                ]
                            ),
                            exit_code,
                        )
        mock_relaunch.assert_called_once()
        mock_run.assert_called_once_with()
        mock_scan.assert_called_once_with()

    def test_auto_relaunched_with_reproxy(self) -> None:
        argv = ["--", "clang++", "foo.o", "-o", "foo"]
        with mock.patch.object(
            os.environ, "get", return_value=None
        ) as mock_env:
            with mock.patch.object(
                cl_utils, "exec_relaunch", side_effect=ImmediateExit
            ) as mock_relaunch:
                with self.assertRaises(ImmediateExit):
                    cxx_link_remote_wrapper.main(argv)
        mock_env.assert_called()
        mock_relaunch.assert_called_once()
        args, kwargs = mock_relaunch.call_args_list[0]
        relaunch_cmd = args[0]
        self.assertEqual(relaunch_cmd[0], str(fuchsia.REPROXY_WRAP))
        cmd_slices = cl_utils.split_into_subsequences(relaunch_cmd[1:], "--")
        reproxy_args, self_script, wrapped_command = cmd_slices
        self.assertEqual(reproxy_args, ["-v"])
        self.assertIn("python", self_script[0])
        self.assertTrue(self_script[-1].endswith("cxx_link_remote_wrapper.py"))
        self.assertEqual(wrapped_command, argv[1:])


if __name__ == "__main__":
    remote_action.init_from_main_once()
    unittest.main()
