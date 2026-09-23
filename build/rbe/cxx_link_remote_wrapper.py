#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Remotely link C and C++.

This script functions as a standalone executable.

Usage:
  $0 [remote options...] -- link-comand...
"""

import argparse
import os
import stat
import subprocess
import sys
from collections.abc import Iterable, Sequence
from pathlib import Path
from typing import Any

import cl_utils
import cxx
import depfile
import fuchsia
import linker
import remote_action

_SCRIPT_BASENAME = Path(__file__).name
_SCRIPT_DIR = Path(__file__).parent

# standard '755' executable permissions
_EXEC_PERMS = (
    stat.S_IRWXU | (stat.S_IRGRP | stat.S_IXGRP) | (stat.S_IROTH | stat.S_IXOTH)
)


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


def _main_arg_parser() -> argparse.ArgumentParser:
    """Construct the argument parser, called by main()."""
    parser = argparse.ArgumentParser(
        description="Prepares a C++ command for remote execution.",
        argument_default=[],
        add_help=True,  # Want this to exit after printing --help
    )
    remote_action.inherit_main_arg_parser_flags(parser)

    group = parser.add_argument_group(
        title="link remote wrapper",
        description="linking remote action options",
    )
    group.add_argument(
        "--output-is-executable",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Expect the main output of linking to be executable.  If true, set the executable bit on the main output's download stub if needed.  See (b/285030257).",
    )
    group.add_argument(
        "--scandeps",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="""If enabled, scan for linker inputs recursively for.  This should only be needed as a potential workaround for cases not handled by reclient.""",
    )
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


class CxxLinkRemoteAction(object):
    def __init__(
        self,
        argv: Sequence[str],
        exec_root: Path | None = None,
        working_dir: Path | None = None,
        host_platform: str | None = None,
        auto_reproxy: bool = True,  # can disable for unit-testing
    ):
        self._working_dir = (working_dir or Path(os.curdir)).absolute()
        self._exec_root = (exec_root or remote_action.PROJECT_ROOT).absolute()
        self._host_platform = host_platform or fuchsia.HOST_PREBUILT_PLATFORM

        # Propagate --remote-flag=... options to the remote prefix,
        # as if they appeared before '--'.
        # Forwarded rewrapper options with values must be written as '--flag=value',
        # not '--flag value' because argparse doesn't know what unhandled flags
        # expect values.
        main_argv, filtered_command = remote_action.forward_remote_flags(argv)

        # forward all unknown flags to rewrapper
        # --help here will result in early exit()
        (
            self._main_args,
            self._main_remote_options,
        ) = _MAIN_ARG_PARSER.parse_known_args(main_argv)

        # Re-launch with reproxy if needed.
        if auto_reproxy:
            remote_action.auto_relaunch_with_reproxy(
                script=Path(__file__), argv=argv, args=self._main_args
            )

        if not filtered_command:  # there is no command, bail out early
            return
        self._cxx_action = cxx.CxxAction(command=filtered_command)

        self._local_only = self._main_args.local

        self._prepare_status: int | None = None
        self._cleanup_files: list[Path] = []
        self._remote_action: remote_action.RemoteAction

    @property
    def compiler_path(self) -> Path:
        return self.cxx_action.compiler.tool

    @property
    def compiler_type(self) -> cxx.Compiler:
        return self.cxx_action.compiler.type

    @property
    def target(self) -> str:
        return self.cxx_action.target

    @property
    def sysroot(self) -> Path | None:
        return self.cxx_action.sysroot

    @property
    def use_ld(self) -> str:
        return self.cxx_action.use_ld or "lld"  # default

    @property
    def linker_executable(self) -> str:
        return self.cxx_action.clang_linker_executable

    @property
    def unwindlib(self) -> str:
        return self.cxx_action.unwindlib or "libunwind"  # default

    @property
    def primary_output(self) -> Path | None:
        return self.cxx_action.output_file

    @property
    def depfile(self) -> Path | None:
        return self.cxx_action.linker_depfile

    def _depfile_exists(self) -> bool:
        # Defined for easy precise mocking.
        return self.depfile is not None and self.depfile.exists()

    def check_preconditions(self) -> None:
        if not self.cxx_action.target and self.cxx_action.compiler_is_clang:
            raise Exception(
                "For remote linking with clang, an explicit --target is required, but is missing."
            )

        if self._main_args.fsatrace_path:
            msg(
                "Warning: Due to https://fxbug.dev/42079382, remote fsatrace does not work with C++ mode as-is, because the fsatrace prefix confuses the re-client C++ input processor.  Automatically disabling --fsatrace-path."
            )
            self._main_args.fsatrace_path = None

        # Not bothering to check for required remote tools,
        # because that would have been covered for remote compile.

    @property
    def command_line_inputs(self) -> Sequence[Path]:
        return [
            Path(p) for p in cl_utils.flatten_comma_list(self._main_args.inputs)
        ]

    @property
    def command_line_output_files(self) -> Sequence[Path]:
        return [
            Path(p)
            for p in cl_utils.flatten_comma_list(self._main_args.output_files)
        ]

    @property
    def command_line_output_dirs(self) -> Sequence[Path]:
        return [
            Path(p)
            for p in cl_utils.flatten_comma_list(
                self._main_args.output_directories
            )
        ]

    def _post_remote_success_action(self) -> int:
        # To prevent remotely produced depfiles from containing absolute paths
        # to the remote build environment, use -no-canonical-prefixes (clang and
        # gcc).  Otherwise, you will need to self._rewrite_remote_depfile().

        # This is temporary to help debug the origins of unexpected absolute
        # paths in remote depfiles.
        if self.compiler_type == cxx.Compiler.GCC and self._depfile_exists():
            return self._verify_remote_depfile()
            # self._rewrite_remote_depfile()

        # Note: --download_outputs=false is re-interpreted so that depfiles
        # will always be downloaded.
        # Search in this file for 'translated_remote_options'.

        # TODO(b/285030257): This is a workaround to a problem where
        # download stubs need the appropriate execution permissions
        # to be set, so that remote execution inputs get the same
        # permissions.  This is important for tools that mirror execution
        # bits from inputs to outputs, like llvm-objcopy.
        # Once re-client presents permission information in the
        # --action_log (reproxy remote_metadata), this workaround can
        # be replaced with a more generalized solution.
        if (
            self.primary_output is not None
            and self.remote_action.skipping_some_download
            and self._main_args.output_is_executable
        ):
            self.primary_output.chmod(_EXEC_PERMS)

        return 0

    def _verify_remote_depfile(self) -> int:
        if self.depfile is None:
            raise ValueError("self.depfile is None")
        abspaths = depfile.absolute_paths(
            depfile.parse_lines(
                self.depfile.read_text().splitlines(keepends=True)
            )
        )
        if abspaths:
            msg(
                f"Found the following absolute paths in {self.depfile}: {abspaths}"
            )
            msg(
                "Absolute paths pointing to remote build environments will fail the ninja-no-op check."
            )
            msg(
                "Recommend: pass only relative paths and  '-no-canonical-prefixes' to the compiler."
            )
            return 1

        return 0

    def _rewrite_remote_depfile(self) -> None:
        if self.depfile is None:
            raise ValueError("self.depfile is None")
        remote_action.rewrite_depfile(
            dep_file=self.working_dir / self.depfile,  # in-place
            transform=self.remote_action._relativize_remote_or_local_deps,
        )

    def _remote_output_files(self) -> Sequence[Path]:
        return [
            *self.cxx_action.linker_output_files(),
            *self.command_line_output_files,
        ]

    def _expand_sysroot_paths(self) -> Iterable[Path]:
        # sysroot files
        if not self.target:
            return
        sysroot_dir = self.sysroot
        if not sysroot_dir:
            return

        # if sysroot points outside of exec_root, stop
        if self._sysroot_is_outside_exec_root:
            return

        sysroot_triple = fuchsia.clang_target_to_sysroot_triple(self.target)
        if sysroot_dir:
            # Some sysroot files are linker scripts to be expanded.
            if sysroot_triple:
                yield sysroot_dir / "usr/lib" / sysroot_triple
                yield sysroot_dir / "lib" / sysroot_triple
            else:
                yield sysroot_dir / "lib"

    def _linker_invocation(self) -> linker.LinkerInvocation:
        """Creates a model linker invocation, for dep-scanning."""
        search_paths = list(self.cxx_action.libdirs)
        search_paths.extend(self._expand_sysroot_paths())
        return linker.LinkerInvocation(
            working_dir_abs=self.working_dir,
            search_paths=search_paths,
            l_libs=self.cxx_action.libs,
            sysroot=self.sysroot,
            direct_files=self.cxx_action.linker_inputs,
            unscanned_direct_files=list(
                self.cxx_action.linker_inputs_from_flags()
            ),
        )

    def scan_linker_inputs(self) -> Iterable[Path]:
        link = self._linker_invocation()
        yield from self.yield_verbose(
            "scanned linker inputs", link.expand_all()
        )

    @property
    def _sysroot_is_outside_exec_root(self) -> bool:
        sysroot_dir = self.sysroot
        if not sysroot_dir:
            return False  # not applicable

        if not sysroot_dir.is_absolute():
            # C sysroot is relative to the working directory
            return False

        # Check all absolute path parents.
        return self.exec_root not in sysroot_dir.parents

    def _sysroot_files(self) -> Iterable[Path]:
        sysroot_dir = self.sysroot
        if not sysroot_dir:
            return

        sysroot_paths = list(self._expand_sysroot_paths())
        if not sysroot_paths:
            return

        link = linker.LinkerInvocation(
            working_dir_abs=self.working_dir,
            search_paths=sysroot_paths,
        )

        lld = self.host_compiler.parent / self.linker_executable

        def linker_script_expander(paths: Sequence[Path]) -> Iterable[Path]:
            if lld.exists():
                yield from link.expand_using_lld(lld=lld, inputs=paths)
            else:
                for path in paths:
                    yield from link.expand_possible_linker_script(path)

        sysroot_triple = fuchsia.clang_target_to_sysroot_triple(self.target)
        yield from self.yield_verbose(
            "C sysroot files",
            fuchsia.c_sysroot_files(
                sysroot_dir=sysroot_dir,
                sysroot_triple=sysroot_triple,
                with_libgcc=False,
                linker_script_expander=linker_script_expander,
            ),
        )

    def prepare(self) -> int:
        """Setup everything ahead of remote execution.

        This may also be needed ahead of local execution to
        evaluate the set of inputs that may need to be downloaded.
        """

        if self._prepare_status is not None:
            return self._prepare_status

        self.check_preconditions()

        remote_command = self.cxx_action.command
        remote_inputs: list[Path] = []

        remote_inputs.extend(self.cxx_action.linker_inputs_from_flags())

        # re-client in link-mode is already able to process inputs inside
        # response files, however, local execution will need the same
        # information from response files to download intermediate inputs.
        # It is safe to specify remote inputs redundantly.
        remote_inputs.extend(self.cxx_action.linker_inputs)
        remote_inputs.extend(self.cxx_action.response_files)

        # TODO(b/307418630): remove the following workaround when fixed.
        remote_inputs.extend(self.cxx_action.linker_response_files)

        remote_ld = self.remote_compiler.parent / self.linker_executable
        if remote_ld.exists():
            remote_inputs.append(remote_ld)

        # built-in toolchain libraries, run-times
        if self.cxx_action.compiler_is_clang:
            remote_inputs.extend(
                fuchsia.remote_clang_linker_toolchain_inputs(
                    clang_path_rel=self.remote_compiler,
                    target=self.cxx_action.target,
                    shared=self.cxx_action.shared,
                    rtlib=self.cxx_action.rtlib,
                    unwindlib=self.unwindlib,
                    profile=self.cxx_action.any_profile,
                    sanitizers=self.cxx_action.sanitizers,
                    # Grab the entire runtime directory without trying to
                    # select the right subset based on variant.
                    want_all_libclang_rt=True,
                )
            )
        elif self.cxx_action.compiler_is_gcc:
            # Workaround b/239101612: missing gcc support libexec binaries for remote build
            remote_inputs.extend(
                list(fuchsia.gcc_support_tools(self.compiler_path, linker=True))
            )
            for libdir in self.cxx_action.libdirs:
                # Search paths that point to the source/checkout directory
                # are assumed to be stable throughout the build, and should
                # be safe to list as input directories.
                # Conservatively, include these directories contents.
                if str(libdir).startswith(str(self.exec_root_rel)):
                    remote_inputs.append(libdir)

        # sysroot libraries:
        # Currently, re-client grabs the entire --sysroot directory, which is
        # excessive.  It would be more efficient to grab only what is needed,
        # like:
        #   remote_inputs.extend(self._sysroot_files())
        # See b/306499345.

        if self.scandeps:
            # workaround b/349448459: find some missing linker inputs
            remote_inputs.extend(self.scan_linker_inputs())

        remote_inputs += self.command_line_inputs

        # Prepare remote compile action
        remote_output_dirs = [
            *self.cxx_action.output_dirs(),
            *self.command_line_output_dirs,
        ]

        remote_options = [
            "--labels=tool=clang,type=link",
            "--canonicalize_working_dir=true",
        ] + self._main_remote_options  # allow forwarded options to override defaults

        # The output file is inferred automatically by rewrapper in C++ mode,
        # but naming it explicitly here makes it easier for RemoteAction
        # to use the output file name for other auxiliary files.
        remote_output_files = self._remote_output_files()

        # Support for remote cross-compilation is missing.
        if self.host_platform != fuchsia.REMOTE_PLATFORM:
            msg("Remote cross-compilation is not supported yet.")
            self._prepare_status = 1
            return self._prepare_status

        self.vprintlist("remote inputs", remote_inputs)
        self.vprintlist("remote output files", remote_output_files)
        self.vprintlist("remote output dirs", remote_output_dirs)

        # Interpret --download_outputs=false as a request to avoid
        # downloading the main linker output, but download everything else.
        # This will always fetch the depfile, which is needed by ninja.
        translated_remote_options = []
        for opt in remote_options:
            if opt == "--download_outputs=false":
                translated_remote_options.append(
                    f"--download_regex=-{self.primary_output}$"
                )
            else:
                translated_remote_options.append(opt)

        self.vprintlist("rewrapper options", translated_remote_options)

        self._remote_action = remote_action.remote_action_from_args(
            main_args=self._main_args,
            remote_options=translated_remote_options,
            command=remote_command,
            inputs=remote_inputs,
            output_files=remote_output_files,
            output_dirs=remote_output_dirs,
            working_dir=self.working_dir,
            exec_root=self.exec_root,
            post_remote_run_success_action=self._post_remote_success_action,
        )

        self._prepare_status = 0
        return self._prepare_status

    @property
    def remote_action(self) -> remote_action.RemoteAction:
        return self._remote_action

    @property
    def working_dir(self) -> Path:
        return self._working_dir

    @property
    def exec_root(self) -> Path:
        return self._exec_root

    @property
    def exec_root_rel(self) -> Path:
        return cl_utils.relpath(self.exec_root, start=self.working_dir)

    @property
    def build_subdir(self) -> Path:  # relative
        """This is the relative path from the exec_root to the current working dir."""
        return self.working_dir.relative_to(self.exec_root)

    @property
    def host_platform(self) -> str:
        return self._host_platform

    @property
    def verbose(self) -> bool:
        return self._main_args.verbose

    @property
    def dry_run(self) -> bool:
        return self._main_args.dry_run

    def vmsg(self, text: str) -> None:
        if self.verbose:
            msg(text)

    def yield_verbose(self, desc: str, items: Iterable[Any]) -> Iterable[Any]:
        """In verbose mode, print and forward items.

        Args:
          desc: text description of what is being printed.
          items: stream of any type of object that is str-able.

        Yields:
          items, unchanged
        """
        if self.verbose:
            msg(f"{desc}: {{")
            for item in items:
                print(f"  {item}")  # one item per line
                yield item
            print(f"}}  # {desc}")
        else:
            yield from items

    def vprintlist(self, desc: str, items: Iterable[Any]) -> None:
        """In verbose mode, print elements.

        Args:
          desc: text description of what is being printed.
          items: stream of any type of object that is str-able.
        """
        if self.verbose:
            msg(f"{desc}: {{")
            for item in items:
                text = str(item)
                print(f"  {text}")
            print(f"}}  # {desc}")

    @property
    def original_link_command(self) -> Sequence[str]:
        return self.cxx_action.command

    @property
    def check_determinism(self) -> bool:
        return self._main_args.check_determinism

    @property
    def determinism_attempts(self) -> int:
        return self._main_args.determinism_attempts

    @property
    def miscomparison_export_dir(self) -> Path | None:
        if self._main_args.miscomparison_export_dir:
            return self.working_dir / self._main_args.miscomparison_export_dir
        return None

    @property
    def label(self) -> str | None:
        return self._main_args.label

    @property
    def scandeps(self) -> bool:
        return self._main_args.scandeps

    @property
    def local_only(self) -> bool:
        return self._local_only

    @property
    def cxx_action(self) -> cxx.CxxAction:
        return self._cxx_action

    @property
    def host_compiler(self) -> Path:
        return self.cxx_action.compiler.tool

    @property
    def remote_compiler(self) -> Path:
        return fuchsia.remote_executable(self.host_compiler)

    def _run_locally(self) -> int:
        # It is possible that inputs come from a previous remote action
        # whose outputs were not downloaded.  Ensure that those inputs
        # are downloaded before executing the action locally.
        prepare_status = self.prepare()
        if prepare_status != 0:
            return prepare_status

        remote_artifact_suffixes = {".o", ".a", ".so", ".dylib"}
        download_statuses = self.remote_action.download_inputs(
            lambda path: path.suffix in remote_artifact_suffixes
            and not str(path).startswith(str(self.remote_action.exec_root_rel))
        )
        for path, status in download_statuses.items():
            if status.returncode != 0:
                msg(f"Failed to download input: {path}\n{status.stderr_text}")
                return status.returncode

        command: Sequence[str]
        if self.check_determinism:
            self.vmsg(
                "Running the original link command locally twice and comparing outputs."
            )
            output_files = self._remote_output_files()

            max_attempts = self.determinism_attempts
            override_attempts = fuchsia.determinism_repetitions(output_files)
            if override_attempts is not None:
                msg(
                    f"Notice: Overriding number of determinism repetitions: {override_attempts}"
                )
                max_attempts = override_attempts

            export_dir = self.miscomparison_export_dir
            command = fuchsia.check_determinism_command(
                exec_root=self.exec_root_rel,
                outputs=output_files,
                command=self.original_link_command,
                max_attempts=max_attempts,
                miscomparison_export_dir=(
                    export_dir / self.build_subdir if export_dir else None
                ),
                label=self.label,
            )
        else:
            self.vmsg("Running the original link command locally.")
            command = self.original_link_command

        exit_code = subprocess.call(command, cwd=self.working_dir)

        # It would be nice if we could upload the set of inputs when
        # compilation fails the determinism check.
        # For C++, the complete set of input is computed by re-client's
        # input processor, and today, we don't have a way to directly access
        # that information (without actually requesting remote execution).
        # We don't have a remote action_digest either because determinism is a
        # local-only check.

        return exit_code

    def _run_remote_action(self) -> int:
        return self.remote_action.run_with_main_args(self._main_args)

    def run(self) -> int:
        if self.local_only:
            return self._run_locally()

        with cl_utils.timer_cm("CxxLinkRemoteAction.prepare()"):
            prepare_status = self.prepare()
            if prepare_status != 0:
                return prepare_status

        # Remote link C++
        try:
            return self._run_remote_action()
        # TODO: normalize absolute paths in remotely generated depfile (gcc)
        finally:
            if not self._main_args.save_temps:
                self._cleanup()

    def _cleanup(self) -> None:
        with cl_utils.timer_cm("CxxLinkRemoteAction._cleanup()"):
            for f in self._cleanup_files:
                f.unlink()


def main(argv: Sequence[str]) -> int:
    with cl_utils.timer_cm("cxx_link_remote_wrapper.main()"):
        with cl_utils.timer_cm("CxxLinkRemoteAction.__init__()"):
            cxx_link_remote_action = CxxLinkRemoteAction(
                argv,  # [remote options] -- C-link-command...
                exec_root=remote_action.PROJECT_ROOT,
                working_dir=Path(os.curdir),
                host_platform=fuchsia.HOST_PREBUILT_PLATFORM,
            )
        with cl_utils.timer_cm("CxxLinkRemoteAction.run()"):
            return cxx_link_remote_action.run()


if __name__ == "__main__":
    remote_action.init_from_main_once()
    sys.exit(main(sys.argv[1:]))
