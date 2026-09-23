#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Wraps a Rust compile command for remote execution (reclient, RBE).

Given a Rust compile command, this script
1) identifies all inputs needed to execute the command hermetically (remotely).
2) identifies all outputs to be retrieved from remote execution.
3) composes a `rewrapper` (reclient) command to request remote execution.
   This includes downloading remotely produced artifacts.
4) forwards stdout/stderr back to the local environment

This script was ported over from build/rbe/rustc-remote-wrapper.sh.

For full usage, see `rustc_remote_wrapper.py -h`.
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
import depfile
import fuchsia
import linker
import remote_action
import rustc

_SCRIPT_BASENAME = Path(__file__).name

# standard '755' executable permissions
_EXEC_PERMS = (
    stat.S_IRWXU | (stat.S_IRGRP | stat.S_IXGRP) | (stat.S_IROTH | stat.S_IXOTH)
)

# This is a list of known dylibs built in Fuchsia.
_KNOWN_FUCHSIA_DYLIBS = ("libvfs_rust",)


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


# string.removeprefix() only appeared in python 3.9
def remove_prefix(text: str, prefix: str) -> str:
    if text.startswith(prefix):
        return text[len(prefix) :]
    return text


# This is needed in some places to workaround b/203540556 (reclient).
def remove_dot_slash_prefix(text: str) -> str:
    return remove_prefix(text, "./")


# string.removesuffix() only appeared in python 3.9
def remove_suffix(text: str, suffix: str) -> str:
    if text.endswith(suffix):
        return text[: -len(suffix)]
    return text


# Defined for convenient mocking.
def _remove_files(files: Iterable[Path]) -> None:
    for f in files:
        f.unlink(missing_ok=True)  # does remove or rmdir


# Defined for convenient mocking.
def _readlines_from_file(path: Path) -> Sequence[str]:
    return path.read_text().splitlines(keepends=True)


# Defined for convenient mocking.
def _make_local_depfile(command: Sequence[str]) -> int:
    return subprocess.call(command)


# Defined for convenient mocking.
def _tool_is_executable(tool: Path) -> bool:
    return os.access(tool, os.X_OK)


# Defined for convenient mocking.
def _libcxx_isfile(libcxx: Path) -> bool:
    return libcxx.is_file()


# Defined for convenient mocking.
def _env_file_exists(path: Path) -> bool:
    return path.exists()


def _flatten_comma_list_to_paths(items: Iterable[str]) -> Iterable[Path]:
    for item in cl_utils.flatten_comma_list(items):
        yield Path(item)


def _main_arg_parser() -> argparse.ArgumentParser:
    """Construct the argument parser, called by main()."""
    parser = argparse.ArgumentParser(
        description="Prepares a Rust compile command for remote execution.",
        argument_default=[],
    )
    remote_action.inherit_main_arg_parser_flags(parser)

    # When using a custom Rust target, the build rules have to plumb this
    # explicitly.
    parser.add_argument(
        "--clang-target",
        default=None,
        help="Clang --target switch corresponding to Rust --target in use",
    )

    # There could be multiple variants of the standard C++ library to choose
    # from.  Use the one that corresponds to the compiler options.
    parser.add_argument(
        "--cxx-stdlibdir",
        type=Path,
        default=None,
        help="Where to find libc++ (from clang)",
    )

    # Use a GN-generated file for the transitive rlib and rmeta deps, instead
    # of running rustc to scan for them.
    parser.add_argument(
        "--rust-compilation-deps-path",
        type=Path,
        default=None,
        help="File of rlib, rmeta, and proc_macro paths needed for compilation",
    )
    # Use a GN-generated file for the set of source files needed instead
    # of running rustc to scan for them.
    parser.add_argument(
        "--rust-sources-path",
        type=Path,
        default=None,
        help="File of rust source files needed for compilation",
    )
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def relativize_paths(paths: Iterable[Path], start: Path) -> Iterable[Path]:
    for p in paths:
        if p.is_absolute():
            yield cl_utils.relpath(p, start=start)
        else:
            yield p  # Paths are already normalized upon construction


def accompany_rlib_with_so(deps: Iterable[Path]) -> Iterable[Path]:
    """Expand list to include .so files.

    Some Rust libraries come with both .rlib and .so (like libstd), however,
    the depfile generator fails to list the .so file in some cases,
    which causes the build to silently fallback to static linking when
    dynamic linking is requested and intended.  This can result in a mismatch
    between local and remote building.
    See https://github.com/rust-lang/rust/issues/90106
    Workaround (https://fxbug.dev/42167956): check for existence of .so and include it.

    Yields:
      original sequence, plus potentially additional .so libs.
    """
    for dep in deps:
        yield dep
        if dep.suffix == ".rlib":
            so_file = dep.with_suffix(".so")  # replaces .rlib with .so
            if so_file.is_file():
                yield so_file


def expand_deps_for_rlib_compile(paths: Iterable[Path]) -> Iterable[Path]:
    """Expand a list of rmeta deps to be suitable for full (rlib) compilation.

    Depending on the rustc options used for scanning for dependencies,
    the deps may contain list the .rmeta metadata file for some
    entries (because only metadata is needed for fast operations like
    dep-scanning).  However, for the purposes of compiling a real .rlib
    (or other rust artifact) we need to interpret this as requiring the
    corresponding .rlib.
    The implied .rlib need not necessarily exist on disk with real
    contents; it is permitted to be a download stub from an earlier
    remote build.

    Yields:
      possibly modified deps.
    """
    for path in paths:
        yield path
        if path.name.endswith(".rmeta"):
            f = path.with_suffix(".rlib")
            if f.exists():  # ok to exist as download stub
                yield f
                continue

            # .so proc macros already appear as deps in the depfile
            # so there is no need to repeat them here.


class RemoteInputProcessingError(RuntimeError):
    def __init__(self, message: str):
        super().__init__(message)


def _filter_local_command(args: Iterable[str]) -> Iterable[str]:
    yield from cl_utils.strip_option_prefix(args, "--local-only")


def _filter_remote_command(args: Iterable[str]) -> Iterable[str]:
    yield from cl_utils.strip_option_prefix(args, "--remote-only")


def _remove_local_command(args: Iterable[str]) -> Iterable[str]:
    yield from cl_utils.filter_out_option_with_arg(args, "--local-only")


def _remove_remote_command(args: Iterable[str]) -> Iterable[str]:
    yield from cl_utils.filter_out_option_with_arg(args, "--remote-only")


class RustRemoteAction(object):
    def __init__(
        self,
        argv: Sequence[str],
        exec_root: Path | None = None,
        working_dir: Path | None = None,
        host_platform: str | None = None,
        auto_reproxy: bool = True,  # Ok to disable during unit-tests
    ):
        self._remote_action: (
            remote_action.RemoteAction
        )  # will be set by prepare() step
        self._working_dir = (working_dir or Path(os.curdir)).absolute()
        self._exec_root = (exec_root or remote_action.PROJECT_ROOT).absolute()
        self._host_platform = host_platform or fuchsia.HOST_PREBUILT_PLATFORM

        # Propagate --remote-flag=... options to the remote prefix,
        # as if they appeared before '--'.
        # Forwarded rewrapper options with values must be written as '--flag=value',
        # not '--flag value' because argparse doesn't know what unhandled flags
        # expect values.
        main_argv, filtered_command = remote_action.forward_remote_flags(argv)

        # Forward all unknown flags to rewrapper
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

        self._rust_action = rustc.RustAction(
            command=filtered_command,
            working_dir=self.working_dir,
        )

        # TODO: check for missing required flags, like --target

        self._local_only = self._main_args.local or self._remote_disqualified()

        self._cleanup_files: list[Path] = []
        self._prepare_status: int | None = (
            None  # 0 means success, like an exit code
        )

    @property
    def _c_sysroot_is_outside_exec_root(self) -> bool:
        c_sysroot_dir = self.c_sysroot
        if not c_sysroot_dir:
            return False  # not applicable

        if not c_sysroot_dir.is_absolute():
            # C sysroot is relative to the working directory
            return False

        # Check all absolute path parents.
        return self.exec_root not in c_sysroot_dir.parents

    def _remote_disqualified(self) -> bool:
        """Detects when the action cannot run remotely for various reasons."""
        # If the C sysroot is outside of exec_root, (e.g. an absolute path
        # like /Library/Developer/... for Mac OS SDKs) then the command
        # will only work locally.
        if self.needs_linker:
            if self._c_sysroot_is_outside_exec_root:
                self.vmsg(
                    f"Forcing local execution because C sysroot ({self.c_sysroot}) is outside of exec_root ({self.exec_root})."
                )
                return True

        # TODO: procedural macros need to target the host AND the remote
        # platform to be usable both locally and remotely.
        # For now, only build with procedural macros locally.
        if any(
            path.suffix == ".dylib"
            for path in self._rust_action.externs.values()
        ):
            self.vmsg(
                f"Forcing local execution because one of the externs points to a .dylib, which does not work in the remote environment."
            )
            return True

        return False

    @property
    def verbose(self) -> bool:
        return self._main_args.verbose

    def vmsg(self, text: str) -> None:
        if self.verbose:
            msg(text)

    @property
    def dry_run(self) -> bool:
        return self._main_args.dry_run

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
    def clang_cxx_stdlibdir(self) -> Path | None:
        return self._main_args.cxx_stdlibdir

    @property
    def clang_target(self) -> str | None:
        return self._main_args.clang_target

    @property
    def local_only(self) -> bool:
        """If the conditions are not right for remote execution, disable,
        regardless of configuration.
        """
        return self._local_only

    @property
    def label(self) -> str:
        return self._main_args.label

    def value_verbose(self, desc: str, value: Path) -> Path:
        """In verbose mode, print and forward value."""
        self.vmsg(f"{desc}: {value}")
        return value

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

    @property
    def working_dir(self) -> Path:
        return self._working_dir

    @property
    def exec_root(self) -> Path:
        return self._exec_root

    @property
    def host_platform(self) -> str:
        return self._host_platform

    @property
    def exec_root_rel(self) -> Path:
        # relpath can handle cases that Path.relative_to() cannot.
        return cl_utils.relpath(self.exec_root, start=self.working_dir)

    @property
    def build_subdir(self) -> Path:  # relative
        """This is the relative path from the exec_root to the current working dir."""
        return self.working_dir.relative_to(self.exec_root)

    @property
    def crate_type(self) -> rustc.CrateType:
        return self._rust_action.crate_type

    @property
    def needs_linker(self) -> bool:
        return self._rust_action.needs_linker

    @property
    def target(self) -> str | None:
        return self._rust_action.target

    @property
    def ensure_rust_sysroot(self) -> Path:
        return self._rust_action.ensure_rust_sysroot

    @property
    def c_sysroot(self) -> Path | None:
        return self._rust_action.c_sysroot

    @property
    def linker(self) -> Path | None:
        return self._rust_action.linker

    @property
    def local_clang_toolchain_dir(self) -> Path:
        """Infer the clang toolchain dir based on the linker location.

        Returns:
          Path to a toolchain dir, relative to the working dir.
        """
        linker = self.linker  # a relative path
        if not linker:
            return self.exec_root_rel / fuchsia.REMOTE_CLANG_SUBDIR
        # We want TOOLDIR from a path like TOOLDIR/bin/TOOL.
        # This follows a typical clang install structure.
        return linker.parent.parent

    @property
    def remote_clang_toolchain_dir(self) -> Path:  # relative
        return fuchsia.remote_executable(self.local_clang_toolchain_dir)

    @property
    def depfile(self) -> Path | None:
        return self._rust_action.depfile

    @property
    def primary_output(self) -> Path | None:
        return self._rust_action.output_file

    @property
    def host_compiler(self) -> Path | None:
        return self._rust_action.compiler

    @property
    def remote_compiler(self) -> Path | None:
        if not self.host_compiler:
            return None
        return fuchsia.remote_executable(self.host_compiler)

    @property
    def host_matches_remote(self) -> bool:
        return self.remote_compiler == self.host_compiler

    @property
    def original_command(self) -> Sequence[str]:
        return self._rust_action.original_command

    def _replace_with_remote_compiler(self, tok: str) -> str | None:
        if tok == str(self.host_compiler):
            return str(self.remote_compiler)
        return None

    @staticmethod
    def _replace_with_remote_linker(tok: str) -> str | None:
        """Replace the host platform linker with the remote platform one."""
        return cl_utils.match_prefix_transform_suffix(
            tok, "-Clinker=", lambda x: str(fuchsia.remote_executable(Path(x)))
        )

    @staticmethod
    def _normalize_libcxx(tok: str) -> str | None:
        # A path like ".../bin/../lib/libc++a" needs to be normalized
        # so that the remote linker does not fail when looking for a
        # non-existent "bin" part of the path.
        prefix = "-Clink-arg="
        if tok.startswith(prefix) and tok.endswith(".a") and ".." in tok:
            right = tok[len(prefix) :]
            # We do not want the Path.resolve() behavior of following
            # symlinks and checking for existence; we truly want simple
            # os.path.normpath() behavior.
            normpath = os.path.normpath(right)
            return prefix + normpath
        return None

    @property
    def local_depfile(self) -> Path:
        return Path(str(self.depfile) + ".nolink")

    @property
    def dep_only_command_with_rspfiles(
        self,
    ) -> tuple[Sequence[str], Sequence[Path]]:
        command, aux_files = self._rust_action.dep_only_command_with_rspfiles(
            str(self.local_depfile)
        )
        local_filtered = _filter_local_command(command)
        remote_removed = _remove_remote_command(local_filtered)
        return (
            cl_utils.auto_env_prefix_command(list(remote_removed)),
            aux_files,
        )

    def local_compile_command(self) -> Iterable[str]:
        yield from _remove_remote_command(
            _filter_local_command(self.original_command)
        )

    def remote_compile_command(self) -> Iterable[str]:
        """Transforms a local command into the remotely executed command.

        Note: that response files are preserved, so tokens inside response files
        cannot be modified.
        """
        for tok in _remove_local_command(
            _filter_remote_command(self.original_command)
        ):
            # Apply the first matching transform on each token.
            replacement = self._replace_with_remote_compiler(tok)
            if replacement is not None:
                yield replacement
                continue

            replacement = self._replace_with_remote_linker(tok)
            if replacement is not None:
                yield replacement
                continue

            replacement = self._normalize_libcxx(tok)
            if replacement is not None:
                yield replacement
                continue

            # When using -fuse-ld, also hint at the full path to the
            # linker tool that is expected, without having to prepend
            # PATH in the remote environment.
            # This is only needed for remote cross-compilation.
            if (
                tok.startswith("-Clink-arg=-fuse-ld=")
                and not self.host_matches_remote
            ):
                yield tok
                yield f"-Clink-arg=--ld-path={self.remote_ld_path}"
                continue

            # else
            yield tok

        if not self._rust_action.explicit_rust_sysroot:
            # TODO(https://fxbug.dev/42057067): relocate rust sysroot to
            # be independent of host-platform to make remote commands
            # match for better caching.
            # The rust sysroot is home to the standard libraries for
            # all target platforms.
            # Fuchsia currently uses the default sysroot location which is
            # based on the *host* compiler path, but the remote compiler's
            # default sysroot will be different.
            # Inform the remote compiler to use the location of the sysroot
            # of the *host* compiler.
            fuchsia_use_host_rust_sysroot = (
                self._rust_action.default_rust_sysroot()
            )
            yield f"--sysroot={fuchsia_use_host_rust_sysroot}"

    def _cleanup(self) -> None:
        _remove_files(self._cleanup_files)

    def _rust_compilation_deps_inputs(self) -> Iterable[Path]:
        """
        Returns the list of compilation deps passed via the file given by --rust-compilation-deps-path
        """
        if self._main_args.rust_compilation_deps_path:
            for (
                p
            ) in (
                self._main_args.rust_compilation_deps_path.read_text().splitlines()
            ):
                yield Path(p)

    def _rust_source_file_inputs(self) -> Iterable[Path]:
        """
        Returns the list of source files passed via the file given by --rust-sources-path
        """
        if self._main_args.rust_sources_path:
            for p in self._main_args.rust_sources_path.read_text().splitlines():
                yield Path(p)

    def _local_depfile_inputs(self) -> Iterable[Path]:
        # Generate a local depfile for the purposes of discovering
        # all transitive inputs.
        self._cleanup_files.append(self.local_depfile)

        dep_only_command, aux_rspfiles = self.dep_only_command_with_rspfiles
        cmd_str = cl_utils.command_quoted_str(dep_only_command)

        self.vmsg(f"scan-deps-only command: {cmd_str}")
        with cl_utils.timer_cm("_make_local_depfile"):
            dep_status = _make_local_depfile(dep_only_command)
        if dep_status != 0:
            self._prepare_status = dep_status
            if self.verbose:
                # This is really a lot of information, which is only interesting
                # in limited circumstances. So we don't print it unless requested.
                raise RemoteInputProcessingError(
                    f"Error: Local generation of depfile failed (exit={dep_status}): {cmd_str}"
                )
            else:
                raise RemoteInputProcessingError(
                    f"Error: Local generation of depfile failed (exit={dep_status}), use --verbose to see the command line."
                )

        # There is a phony dep for each input that is needed.
        deps = list(
            depfile.parse_lines(_readlines_from_file(self.local_depfile))
        )
        target_paths = [dep.target_paths for dep in deps if dep.is_phony]
        remote_depfile_inputs = [
            target for paths in target_paths for target in paths
        ]

        # TODO(https://fxbug.dev/395778407): binary-dep-depinfo only emits
        # .rmeta for Fuchsia dylibs, figure out a better way to include .so
        # files here, which are necessary in linking.
        for p in remote_depfile_inputs:
            libname = p.name.removesuffix(".rmeta")
            if libname in _KNOWN_FUCHSIA_DYLIBS:
                remote_depfile_inputs += [p.parent.joinpath(f"{libname}.so")]

        # Expand some .rmeta deps for .rlib compilation.
        expanded_remote_inputs = list(
            expand_deps_for_rlib_compile(remote_depfile_inputs)
        )

        # TODO: if needed, transform the rust std lib paths, depending on
        #   Fuchsia directory layout of Rust prebuilt libs.
        #   See remap_remote_rust_lib() in rustc-remote-wrapper.sh
        #   for one possible implementation.
        yield from self.yield_verbose(
            "depfile inputs",
            relativize_paths(
                accompany_rlib_with_so(expanded_remote_inputs), self.working_dir
            ),
        )

        # If everything above succeeded, clean-up the temporary files,
        # otherwise leave them to be examined.
        _remove_files(aux_rspfiles)

    def _remote_compiler_inputs(self) -> Iterable[Path]:
        # remote compiler is an input
        if self.remote_compiler is None:
            raise ValueError("self.remote_compiler is None")
        remote_compiler = self.remote_compiler
        if not _tool_is_executable(remote_compiler):
            raise RemoteInputProcessingError(
                f"Remote compilation requires {remote_compiler} to be available for uploading, but it is missing."
            )
        yield self.value_verbose("remote compiler", remote_compiler)
        yield from self._remote_rustc_shlibs()

    def _remote_rustc_shlibs(self) -> Iterable[Path]:
        """Find remote compiler shared libraries.

        Yields:
          shared library paths relative to current working dir.
        """
        if self.host_platform == fuchsia.REMOTE_PLATFORM:
            if self.remote_compiler is None:
                raise ValueError("self.remote_compiler is None")
            # remote and host execution environments match
            yield from self.yield_verbose(
                "remote compiler shlibs (detected)",
                relativize_paths(
                    remote_action.host_tool_nonsystem_shlibs(
                        self.remote_compiler
                    ),
                    self.working_dir,
                ),
            )
        else:
            yield from self._assumed_remote_rustc_shlibs()

    def _assumed_remote_rustc_shlibs(self) -> Iterable[Path]:
        # KLUDGE: the host's binutils may not be able to analyze the remote
        # executable (ELF), so hardcode what we think the shlibs are.
        yield from self.yield_verbose(
            "remote compiler shlibs (guessed)",
            fuchsia.remote_rustc_shlibs(self.exec_root_rel),
        )

    def _rust_stdlib_libunwind_inputs(self) -> Iterable[Path]:
        # The majority of stdlibs already appear in dep-info and are uploaded
        # as needed.  However, libunwind.a is not listed, but is directly
        # needed by code emitted by rustc.  Listing this here works around a
        # missing upload issue, and adheres to the guidance of listing files
        # instead of whole directories.
        if not self.target:
            return
        libunwind_a = (
            self.ensure_rust_sysroot
            / fuchsia.rust_stdlib_subdir(  # relative to self.working_dir
                target_triple=self.target
            )
            / "libunwind.a"
        )
        if libunwind_a.exists():
            yield self.value_verbose("libunwind", libunwind_a)

    def _rust_stdlib_inputs(self) -> Iterable[Path]:
        """Returns all the stdlib files

        This is more than we need, but they'll be cached and it's faster than
        running rustc to find out which specific files are needed.
        """
        if not self.target:
            return
        stdlib_dir = (
            self.ensure_rust_sysroot
            / fuchsia.rust_stdlib_subdir(  # relative to self.working_dir
                target_triple=self.target
            )
        )
        if stdlib_dir.exists():
            for p in stdlib_dir.iterdir():
                if p.is_file():
                    yield p

    def _inputs_from_env(self) -> Iterable[Path]:
        """Scan command environment variables for references to inputs files.

        If a variable value looks like a path and it points to something
        that exists, assume it is needed for remote execution.
        """
        for e in self._rust_action.env:
            key, sep, value = e.partition("=")
            if sep == "=":
                try:
                    p = Path(value)
                    if _env_file_exists(p):
                        yield cl_utils.relpath(p, start=self.working_dir)
                except ValueError:  # value is not a Path, ignore it
                    pass

    def _remote_inputs(self) -> Iterable[Path]:
        """Remote inputs are relative to current working dir."""

        _source_file_inputs = list(self._rust_source_file_inputs())

        if fuchsia.rust_source_file_listing_disabled(_source_file_inputs):
            # GN cannot provide a list of source files, so we'll need to run
            # rustc to get the list.

            with cl_utils.timer_cm("rustc scanned inputs"):
                yield from self.yield_verbose(
                    "rustc scan results",
                    self._local_depfile_inputs(),
                )

            yield from self._rust_stdlib_libunwind_inputs()

        else:
            # Use GN-provided files of dependencies and source files
            yield from self.yield_verbose(
                "source files",
                _source_file_inputs,
            )
            yield from self.yield_verbose(
                "dependencies",
                expand_deps_for_rlib_compile(
                    self._rust_compilation_deps_inputs()
                ),
            )
            yield from self.yield_verbose(
                "stdlib inputs",
                self._rust_stdlib_inputs(),
            )

        # Pass along response files without altering the original command.
        yield from self._rust_action.response_files

        with cl_utils.timer_cm("remote_compiler_inputs"):
            yield from self._remote_compiler_inputs()

        # Indirect dependencies (libraries)
        yield from self.yield_verbose(
            "extern libs", self._rust_action.extern_paths()
        )

        # Prefer to have the build system's command specify additional
        # --remote-inputs instead of trying to infer them.
        # yield from self.yield_verbose('env var files', self._inputs_from_env())

        # Link arguments like static libs are checked for existence
        # but not necessarily used until linking a binary.
        # This is why we need to includes link_arg_files unconditionally,
        # whereas the linker tools themselves can be dropped until linking binaries.
        yield from self.yield_verbose(
            "link arg files",
            list(self._rust_action.link_arg_files)
            + list(self._rust_action.linker_scripts),
        )

        if self.needs_linker:
            yield from self._remote_linker_inputs()

        yield from self.yield_verbose(
            "forwarded inputs",
            _flatten_comma_list_to_paths(self._main_args.inputs),
        )

    def _remote_output_files(self) -> Iterable[Path]:
        """Remote output files are relative to current working dir."""
        if self.primary_output:
            yield self.value_verbose("main output", self.primary_output)

        depfile = self.depfile
        if depfile:
            yield self.value_verbose("depfile", depfile)

        yield from self.yield_verbose(
            "extra compiler outputs", self._rust_action.extra_output_files()
        )

        yield from self.yield_verbose(
            "forwarded output files",
            _flatten_comma_list_to_paths(self._main_args.output_files),
        )

    def _remote_output_dirs(self) -> Iterable[Path]:
        yield from self.yield_verbose(
            "forwarded output dirs",
            _flatten_comma_list_to_paths(self._main_args.output_directories),
        )

    @property
    def remote_options(self) -> Sequence[str]:
        """rewrapper remote execution options."""
        fixed_remote_options = [
            # type=tool says we are providing a custom tool (Rust compiler), and
            #   thus, own the logic for providing explicit inputs.
            # shallow=true works around an issue where racing mode downloads
            #   incorrectly
            # toolname=rustc just helps classify the remote action type
            "--labels=type=tool,shallow=true,toolname=rustc",
            # --canonicalize_working_dir: coerce the output dir to a constant.
            #   This requires that the command be insensitive to output dir, and
            #   that its outputs do not leak the remote output dir.
            #   Ensuring that the results reproduce consistently across different
            #   build directories helps with caching.
            "--canonicalize_working_dir=true",
        ]

        # _main_remote_options should be allowed to override the
        # fixed_remote_options
        return fixed_remote_options + self._main_remote_options

    def prepare(self) -> int:
        """Setup the remote action, but do not execute it.

        Returns:
          exit code, 0 for success
        """
        # cache the preparation
        if self._prepare_status is not None:
            return self._prepare_status

        # inputs and outputs are relative to current working dir
        try:
            with cl_utils.timer_cm(
                "rustc_remote_wrapper getting remote inputs"
            ):
                remote_inputs = sorted(set(self._remote_inputs()))
        except RemoteInputProcessingError as e:
            msg(str(e))
            return 1

        remote_output_files = list(self._remote_output_files())
        remote_output_dirs = list(self._remote_output_dirs())

        # Interpret --download_outputs=false as a request to avoid
        # downloading only the primary rustc output (often the largest
        # artifact).  In other words, always download *all* other outputs,
        # including the depfile and emitted llvm-ir (if applicable).
        # The depfile *must* be downloaded because it is consumed by ninja.

        translated_remote_options: list[str] = []
        for opt in self.remote_options:
            if opt == "--download_outputs=false":
                translated_remote_options.append(
                    f"--download_regex=-{self.primary_output}$"
                )
            else:
                translated_remote_options.append(opt)

        self._remote_action = remote_action.remote_action_from_args(
            main_args=self._main_args,
            remote_options=translated_remote_options,
            command=list(self.remote_compile_command()),
            local_only_command=list(self.local_compile_command()),
            inputs=remote_inputs,
            output_files=remote_output_files,
            output_dirs=remote_output_dirs,
            working_dir=self.working_dir,
            exec_root=self.exec_root,
            post_remote_run_success_action=self._post_remote_success_action,
        )
        self._prepare_status = 0  # exit code success
        return self._prepare_status

    @property
    def remote_action(self) -> remote_action.RemoteAction:
        return self._remote_action

    @property
    def remote_linker(self) -> Path | None:
        if not self.linker:
            return None
        return fuchsia.remote_executable(self.linker)

    @property
    def target_linker_prefix(self) -> str | None:
        if not self.target:
            return None
        if "darwin" in self.target:
            return "ld64"
        return "ld"  # most cases

    @property
    def remote_ld_path(self) -> Path | None:
        ld = self._rust_action.use_ld  # e.g. "lld"
        if not ld:
            return None
        prefix = self.target_linker_prefix
        if not prefix:
            return None
        if not self.remote_linker:
            return None
        return self.remote_linker.parent / f"{prefix}.{ld}"

    def _remote_linker_executables(self) -> Iterable[Path]:
        if self.linker:
            remote_linker = self.remote_linker
            if remote_linker:
                yield self.value_verbose("remote linker", remote_linker)

            # ld.lld -> lld, but the symlink is required for the clang
            # linker driver to be able to use lld.
            # Nowadays, lld -> llvm.
            # ld.LINKER is expected to be in the remote environment PATH.
            # See ToolChain::GetLinkerPath() in llvm-project:clang/lib/Driver/ToolChain.cpp
            ld = self._rust_action.use_ld  # e.g. "lld"
            if ld and self.remote_ld_path:
                yield self.value_verbose(
                    "remote clang -fuse-ld", self.remote_ld_path
                )

    def _remote_libcxx(self, clang_lib_triple: str) -> Iterable[Path]:
        if self.clang_cxx_stdlibdir:  # override, selecting for variant
            libcxx_remote = self.clang_cxx_stdlibdir / "libc++.a"
        else:
            libcxx_remote = fuchsia.clang_libcxx_static(
                self.remote_clang_toolchain_dir, clang_lib_triple
            )
        if _libcxx_isfile(libcxx_remote):
            yield self.value_verbose("remote libc++", libcxx_remote)

    def _remote_clang_runtime_libs(
        self, clang_lib_triple: str
    ) -> Iterable[Path]:
        # clang runtime lib dir
        rt_libdir_remote = list(
            fuchsia.clang_runtime_libdirs(
                self.remote_clang_toolchain_dir, clang_lib_triple
            )
        )
        # if none found, that's ok.
        if len(rt_libdir_remote) == 1:
            yield self.value_verbose(
                "remote runtime libdir", rt_libdir_remote[0]
            )

        if len(rt_libdir_remote) > 1:
            raise RemoteInputProcessingError(
                f"Found more than one clang runtime lib dir (don't know which one to use): {rt_libdir_remote}"
            )

    def _c_sysroot_files(self) -> Iterable[Path]:
        # sysroot files
        if not self.target:
            return
        c_sysroot_dir = self.c_sysroot
        if not c_sysroot_dir:
            return
        # if sysroot points outside of exec_root, stop
        if self._c_sysroot_is_outside_exec_root:
            return
        sysroot_triple = fuchsia.rustc_target_to_sysroot_triple(self.target)
        if c_sysroot_dir:
            # Some sysroot files are linker scripts to be expanded.
            if sysroot_triple:
                search_paths = [
                    c_sysroot_dir / "usr/lib" / sysroot_triple,
                    c_sysroot_dir / "lib" / sysroot_triple,
                ]
            else:
                search_paths = [c_sysroot_dir / "lib"]

            link = linker.LinkerInvocation(
                working_dir_abs=self.working_dir, search_paths=search_paths
            )

            if not self.host_compiler:
                raise ValueError("self.host_compiler is None")
            lld = self.host_compiler.parent / "ld.lld"  # co-located with clang

            def linker_script_expander(paths: Sequence[Path]) -> Iterable[Path]:
                if lld.exists():
                    yield from link.expand_using_lld(lld=lld, inputs=paths)
                else:
                    for path in paths:
                        yield from link.expand_possible_linker_script(path)

            yield from self.yield_verbose(
                "C sysroot files",
                fuchsia.c_sysroot_files(
                    sysroot_dir=c_sysroot_dir,
                    sysroot_triple=sysroot_triple,
                    with_libgcc=self._rust_action.want_sysroot_libgcc,
                    linker_script_expander=linker_script_expander,
                ),
            )

    def _remote_linker_inputs(self) -> Iterable[Path]:
        if self.linker:
            yield from self._remote_linker_executables()

            if self.target:
                clang_lib_triple = (
                    self.clang_target
                    or fuchsia.rustc_target_to_clang_target(self.target)
                )
                yield from self._remote_libcxx(clang_lib_triple)
                yield from self._remote_clang_runtime_libs(clang_lib_triple)

        # --crate-type cdylib needs rust-lld (hard-coding this is a hack)
        # This will always be linux-x64, even when cross-compiling, because
        # that is the only RBE remote backend option available.
        if self.crate_type == rustc.CrateType.CDYLIB:
            if not self.remote_compiler:
                raise ValueError("self.remote_compiler is None")
            yield self.value_verbose(
                "remote rust-lld",
                fuchsia.remote_rustc_to_rust_lld_path(self.remote_compiler),
            )

        yield from self._c_sysroot_files()

    def _depfile_exists(self) -> bool:
        # Defined for easy mocking.
        return self.depfile is not None and self.depfile.exists()

    def _post_remote_success_action(self) -> int:
        if self._depfile_exists():
            self._rewrite_remote_or_local_depfile()

        if (
            self.remote_action.skipping_some_download
            and self._rust_action.main_output_is_executable
        ):
            # TODO(b/285030257): This is a workaround to a problem where
            # download stubs need the appropriate execution permissions
            # to be set, so that remote execution inputs get the same
            # permissions.  This is important for tools that mirror execution
            # bits from inputs to outputs, like llvm-objcopy.
            # Once re-client presents permission information in the
            # --action_log (reproxy remote_metadata), this workaround can
            # be replaced with a more generalized solution.
            if not self.primary_output:
                raise ValueError("self.primary_output is None")
            if remote_action.is_download_stub_file(self.primary_output):
                self.primary_output.chmod(_EXEC_PERMS)
        return 0

    def _rewrite_remote_or_local_depfile(self) -> None:
        """Rewrite depfile without working dir absolute paths.

        TEMPORARY WORKAROUND until upstream fix lands:
          https://github.com/pest-parser/pest/pull/522
        Rewrite the depfile if it contains any absolute paths from the remote
        build; paths should be relative to the root_build_dir.

        Assume that the output dir is two levels down from the exec_root.

        When using the `canonicalize_working_dir` rewrapper option,
        the output directory is coerced to a predictable 'set_by_reclient' constant.
        See https://source.corp.google.com/foundry-x-re-client/internal/pkg/reproxy/action.go;l=131
        It is still possible for a tool to leak absolute paths, which could
        expose that constant in returned artifacts.
        We forgive this for depfiles, but other artifacts should be verified
        separately.
        """
        # It is possible for this to run after local execution with
        # exec_strategy=local,racing,remote_local_fallback, so the logic
        # herein should accommodate both possibilities.
        # In the future, it might be possible to determine whether the local
        # or remote result was used from self.action_log.
        self.vmsg(f"Rewriting the (remote or local) depfile {self.depfile}")
        if self.depfile is None:
            raise ValueError("self.depfile is None")
        remote_action.rewrite_depfile(
            dep_file=self.working_dir / self.depfile,  # in-place
            transform=self.remote_action._relativize_remote_or_local_deps,
        )

    def run_remote(self) -> int:
        prepare_status = self.prepare()
        if prepare_status != 0:
            return prepare_status

        try:
            exit_code = self.remote_action.run_with_main_args(self._main_args)
            # On non-zero exits, we may be interested in certain debug outputs.
            if exit_code != 0:
                link_repro = self._rust_action.link_reproducer
                if link_repro:
                    self.remote_action.download_output_file(link_repro)
                    # if there was an error, diagnostics were already printed.
            return exit_code

        finally:
            self._cleanup()

    def run_local(self) -> int:
        # Even if this is running locally, some of intermediate inputs
        # have have come from remote actions that opted not to
        # download their outputs (and left only download stubs).
        # To be safe, verify the inputs and download as needed.

        prepare_status = self.prepare()
        if prepare_status != 0:
            return prepare_status

        # We know in our build system configuration that the following
        # file suffixes could have come from remote builds.
        # It is ok for this set to be conservatively inclusive.
        # Eliminate inputs that come from the project root ../..,
        # because those are sources or prebuilts.
        remote_artifact_suffixes = {".o", ".a", ".so", ".dylib", ".rlib"}
        download_statuses = self.remote_action.download_inputs(
            lambda path: path.suffix in remote_artifact_suffixes
            and not str(path).startswith(str(self.remote_action.exec_root_rel))
        )
        for path, status in download_statuses.items():
            if status.returncode != 0:
                msg(f"Failed to download input: {path}\n{status.stderr_text}")
                return status.returncode

        # don't bother with remote action preparation
        # or any of the remote action features.
        export_dir = self.miscomparison_export_dir
        command = cl_utils.auto_env_prefix_command(
            list(
                _remove_remote_command(
                    _filter_local_command(self.original_command)
                )
            )
        )
        if self.check_determinism:
            self.vmsg("Comparing two local runs of the original command.")

            output_files = list(self._remote_output_files())

            max_attempts = self.determinism_attempts
            # For b/328756843, increase repetition count for hard-to-reproduce cases.
            override_attempts = fuchsia.determinism_repetitions(output_files)
            if override_attempts is not None:
                msg(
                    f"Notice: Overriding number of determinism repetitions: {override_attempts}"
                )
                max_attempts = override_attempts

            command = fuchsia.check_determinism_command(
                exec_root=self.exec_root_rel,
                outputs=output_files,
                command=command,
                max_attempts=max_attempts,
                miscomparison_export_dir=(
                    export_dir / self.build_subdir if export_dir else None
                ),
                label=self.label,
            )

        exit_code = subprocess.call(command, cwd=self.working_dir)

        # Optional: on determinism failures, copy data.
        if exit_code != 0 and self.check_determinism and export_dir:
            # Compute the inputs that would have been used for remote execution
            if not self.remote_action:
                self.prepare()
            # Check determinism script already copies outputs, we just copy the inputs.
            with cl_utils.chdir_cm(self.exec_root):
                for f in self.remote_action.inputs_relative_to_project_root:
                    cl_utils.copy_preserve_subpath(f, export_dir)

        return exit_code

    def run(self) -> int:
        if self.local_only:
            return self.run_local()
        else:
            return self.run_remote()


def main(argv: Sequence[str]) -> int:
    rust_remote_action = RustRemoteAction(
        argv,  # [remote options] -- rustc-compile-command...
        exec_root=remote_action.PROJECT_ROOT,
        working_dir=Path(os.curdir),
        host_platform=fuchsia.HOST_PREBUILT_PLATFORM,
    )
    return rust_remote_action.run()


if __name__ == "__main__":
    remote_action.init_from_main_once()
    sys.exit(main(sys.argv[1:]))
