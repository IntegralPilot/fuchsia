#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""C++ compilation commands and attributes.
"""

import argparse
import dataclasses
import enum
import os
import tempfile
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import AbstractSet

import cl_utils

_RUSTC_FUSED_FLAGS = ["-C", "-L", "-Z"]


def _remove_prefix(base: str, prefix: str) -> str:
    # str.removeprefix is only available in Python 3.9+
    if base.startswith(prefix):
        return base[len(prefix) :]
    return base


def _remove_suffix(base: str, suffix: str) -> str:
    # str.removesuffix is only available in Python 3.9+
    if base.endswith(suffix):
        return base[: -len(suffix)]
    return base


def _rustc_command_scanner() -> argparse.ArgumentParser:
    """Analyze rustc command attributes.

    Flags should already be canonicalized through `expand_fused_flags()`.
    """
    parser = argparse.ArgumentParser(
        description="Detects Rust compilation attributes",
        argument_default=None,
        add_help=False,
    )
    parser.add_argument(
        "-o",  # required
        type=Path,
        dest="output",
        default=None,
        metavar="FILE",
        help="compiler output",
    )
    parser.add_argument(
        "--crate-type",
        type=str,
        default=None,
        metavar="TYPE",
        help="Rust compiler output crate type",
    )
    parser.add_argument(
        "--sysroot",
        type=Path,
        default=None,
        help="compiler sysroot, where target-specific rustlibs can be found",
    )
    parser.add_argument(
        "--target",
        type=str,
        default=None,
        help="target platform",
    )
    parser.add_argument(
        "--emit",
        action="append",
        default=[],
        help="Types of outputs to produce.  Values can be 'key' or 'key=value' form.",
    )
    parser.add_argument(
        "--extern",
        action="append",
        default=[],
        metavar="LIB:PATH",
        help="Specify where transitive dependencies can be found",
    )
    for f in _RUSTC_FUSED_FLAGS:
        parser.add_argument(
            f,
            type=str,
            action="append",
            dest="{}_flags".format(f.lstrip("-")),
            default=[],
            help=f"All {f}* flags",
        )
    return parser


_RUSTC_COMMAND_SCANNER = _rustc_command_scanner()


class CrateType(enum.Enum):
    UNKNOWN = 0
    RLIB = 1  # "rlib"
    BINARY = 2  # "bin"
    CDYLIB = 3  # "cdylib"
    DYLIB = 4  # "dylib"
    PROC_MACRO = 5  # "proc-macro"
    STATICLIB = 6  # "staticlib"


def parse_crate_type(crate_type: str) -> CrateType:
    return {
        "rlib": CrateType.RLIB,
        "bin": CrateType.BINARY,
        "cdylib": CrateType.CDYLIB,
        "dylib": CrateType.DYLIB,
        "proc-macro": CrateType.PROC_MACRO,
        "staticlib": CrateType.STATICLIB,
    }.get(crate_type, CrateType.UNKNOWN)


class InputType(enum.Enum):
    SOURCE = 0
    LINKABLE = 1  # Any of: .a, .so, .dylib


@dataclasses.dataclass
class RustcInput(object):
    file: Path
    type: InputType


def is_linkable(f: str) -> bool:
    # f might not be a Path
    return any(
        f.endswith(suffix)
        for suffix in (".a", ".o", ".so", ".dylib", ".so.debug", ".ld")
    )


def find_direct_inputs(command: Iterable[str]) -> Iterable[RustcInput]:
    for tok in command:
        if tok.endswith(".rs"):
            yield RustcInput(file=Path(tok), type=InputType.SOURCE)
        elif tok.startswith("@"):
            yield RustcInput(file=Path(tok[1:]), type=InputType.SOURCE)
        elif is_linkable(tok):
            yield RustcInput(file=Path(tok), type=InputType.LINKABLE)


def find_compiler_from_command(command: Iterable[str]) -> Path | None:
    for tok in command:
        if "rustc" in tok:
            return Path(tok)
    return None  # or raise error


class _ScanDepsCommandTransformer(object):
    """Implementation detail of rustc_dep_only_command()."""

    def __init__(self, emit_metadata: bool, depfile_name: str):
        self._emit_metadata = emit_metadata
        self._depfile_name = depfile_name
        self._replaced_emit = False
        self._aux_rspfiles: list[Path] = []

    @property
    def emit_metadata(self) -> bool:
        return self._emit_metadata

    @property
    def depfile_name(self) -> str:
        return self._depfile_name

    @property
    def _replacement_emit_args(self) -> Sequence[str]:
        # Remove outputs like emit=link.
        if self.emit_metadata:
            # Need to tell the compiler driver that we are only interested
            # in the deps needed for generating metadata.
            return [
                f"--emit=metadata,dep-info={self.depfile_name}",
                "-Z",
                "binary-dep-depinfo",
            ]
        else:
            return [
                f"--emit=dep-info={self.depfile_name}",
                "-Z",
                "binary-dep-depinfo",
            ]

    def _rustc_dep_only_command_impl(
        self, command_tokens: Iterable[str]
    ) -> Iterable[str]:
        """Generate a command that only produces a depfile.

        This also accumulates a set of temporary transformed rspfiles
        that the caller is responsible for cleaning up.

        Yields:
          possibly transformed command token
        """

        handle_optarg = None
        # Use the original command (without response files expanded)
        # to avoid command length limits.
        # Because of this, this transformation on --emit only works
        # when the --emit argument is not buried inside a response file.
        # When targeting metadata, for --externs, replace .rlib with
        # corresponding .rmeta if it exists (not required).
        for tok in command_tokens:
            if handle_optarg == "--extern":
                handle_optarg = None
                if not self.emit_metadata:
                    yield tok
                    continue

                lib, sep, path = tok.partition("=")
                if sep != "=":  # --extern foo (without path)
                    yield tok
                    continue

                # Assume .so files are proc_macros, which are needed for
                # dep scanning; rmeta alone won't suffice.
                if path.endswith(".so"):
                    yield tok
                    continue

                rmeta = Path(path).with_suffix(".rmeta")
                # evaluating metadata only requires other .rmeta files
                if rmeta.exists():
                    yield f"{lib}={rmeta}"
                else:
                    yield tok

                continue

            if handle_optarg == "-o":
                handle_optarg = None
                if self.emit_metadata:
                    yield str(Path(tok).with_suffix(".rmeta"))
                else:
                    yield tok
                continue

            # Replace the first --emit encountered in the original command tokens
            if tok.startswith("--emit"):
                if self._replaced_emit:
                    pass
                else:
                    self._replaced_emit = True
                    for arg in self._replacement_emit_args:
                        yield arg
                continue

            if tok in {"--extern", "-o"}:
                handle_optarg = tok
                yield tok
                continue

            # Check for response files, recursively descend into them.
            prefix_matched = False
            for prefix in ("@shell:", "@"):
                # Apparently shell-argfiles do not want a trailing newline,
                # because that will result in an '' filename arg.
                if tok.startswith(prefix):
                    prefix_matched = True
                    # recursively descend into response files, and rewrite them
                    # into new copies as needed.
                    orig_rspfile = Path(tok.removeprefix(prefix))

                    yield self._transform_rspfile_arg(prefix, orig_rspfile)
                    break

            if prefix_matched:
                continue

            # else
            yield tok

    def _transform_rspfile_arg(self, prefix: str, orig_rspfile: Path) -> str:
        """Transform the contents of a @rspfile into a new one.

        Args:
          prefix: either "@" or "@shell:" part of the response file argument.
          orig_rspfile: the original response file referenced.

        Returns:
          Possibly modified command-line argument for a response file.
        """
        rspfile_contents = orig_rspfile.read_text()
        if not rspfile_contents:
            # Don't bother changing empty files.
            # This also avoids accidentally adding a blank line
            # which the compiler may interpret as ''.
            return f"{prefix}{orig_rspfile}"

        new_rspfile_lines = list(
            self._rustc_dep_only_command_rspfile_lines(
                rspfile_contents.splitlines()
            )
        )

        # Use a temporary file with a random suffix, in case
        # multiple concurrent invocations reference the same rspfile.
        # It is the caller's responsibility to clean up each of these
        # tempfiles.
        _, new_rspfile = tempfile.mkstemp(
            dir=str(orig_rspfile.parent),
            prefix=orig_rspfile.name + ".aux.",
            text=True,
        )

        # Keep absolute/relative path consistent with original.
        new_rspfile_path = Path(new_rspfile)
        if not orig_rspfile.is_absolute():
            new_rspfile_path = cl_utils.relpath(new_rspfile_path, Path("."))
        new_rspfile_path.write_text("\n".join(new_rspfile_lines) + "\n")

        # Tell the caller to clean-up this transformed rspfile.
        self._aux_rspfiles.append(new_rspfile_path)
        return f"{prefix}{new_rspfile_path}"

    def _rustc_dep_only_command_rspfile_lines(
        self,
        rspfile_lines: Iterable[str],
    ) -> Iterable[str]:
        # transform rspfile one line at a time
        for line in rspfile_lines:
            yield self._rustc_dep_only_command_rspfile_line(line)

    def _rustc_dep_only_command_rspfile_line(
        self,
        rspfile_line: str,
    ) -> str:
        """Transforms a single line of a rspfile for a dep-scanning command."""
        # Note: use space split instead of shlex.split because quotes
        # are to be interpreted literally.
        return " ".join(
            self._rustc_dep_only_command_impl(
                command_tokens=rspfile_line.split(" ")
            )
        )


def rustc_dep_only_command(
    command_tokens: Iterable[str],
    emit_metadata: bool,
    depfile_name: str,
) -> tuple[Sequence[str], Sequence[Path]]:
    """Generate a command that only produces a depfile.

    This implementation handles response files recursively,
    by transforming them as they are encountered into new auxiliary
    response files referenced in the new command.

    Args:
      command_tokens: original rustc command
      emit_metadata: if True, command is expected to generate
        separate metadata.
      depfile_name: name of the new depfile to emit (different
        from the depfile in the original command).

    Returns:
      * transformed command tokens, for dep-scanning
      * paths of rewritten temporary response files (containing
        possibly transformed command tokens), that the caller should
        clean up automatically.
    """
    transformer = _ScanDepsCommandTransformer(
        emit_metadata=emit_metadata,
        depfile_name=depfile_name,
    )
    aux_toks = list(transformer._rustc_dep_only_command_impl(command_tokens))
    aux_rspfiles = transformer._aux_rspfiles

    if not transformer._replaced_emit:
        return [*aux_toks, *transformer._replacement_emit_args], aux_rspfiles

    return aux_toks, aux_rspfiles


class RustAction(object):
    """This is a rustc compile action."""

    def __init__(self, command: Sequence[str], working_dir: Path | None = None):
        # keep a copy of the original command
        self._original_command = command
        self._working_dir = (working_dir or Path(os.curdir)).absolute()

        # expand response files
        rsp_files: list[Path] = []
        self._rsp_expanded_command = list(
            cl_utils.expand_response_files(self.original_command, rsp_files)
        )
        self._response_files = set(rsp_files)

        # analyze response-file-expanded command using canonical expanded-flag form
        (
            self._attributes,
            remaining_args,
        ) = _RUSTC_COMMAND_SCANNER.parse_known_args(
            list(
                cl_utils.expand_fused_flags(
                    self.rsp_expanded_command, _RUSTC_FUSED_FLAGS
                )
            )
        )
        self._compiler = find_compiler_from_command(remaining_args)
        compiler_arg_index = remaining_args.index(str(self._compiler))
        self._env = [
            tok for tok in remaining_args[:compiler_arg_index] if "=" in tok
        ]
        self._crate_type = parse_crate_type(self._attributes.crate_type)
        self._direct_inputs = list(
            find_direct_inputs(remaining_args[compiler_arg_index:])
        )
        self._C_flags = cl_utils.keyed_flags_to_values_dict(
            self._attributes.C_flags
        )
        self._Z_flags = cl_utils.keyed_flags_to_values_dict(
            self._attributes.Z_flags
        )
        self._L_flags = cl_utils.keyed_flags_to_values_dict(
            self._attributes.L_flags
        )
        self._emit = cl_utils.keyed_flags_to_values_dict(
            cl_utils.flatten_comma_list(self._attributes.emit)
        )
        raw_extern = cl_utils.keyed_flags_to_values_dict(
            cl_utils.flatten_comma_list(self._attributes.extern),
            convert_type=Path,
        )
        self._extern: dict[str, Path] = {
            k: Path(v[-1])  # last value wins
            for k, v in raw_extern.items()
            if v  # ignore empty lists
        }

        # post-process some flags
        self._c_sysroot: Path | None = None
        self._use_ld: Path | None = None
        self._link_reproducer: Path | None = None
        self._want_sysroot_libgcc = False
        self._link_arg_files: Sequence[Path] = []
        self._linker_scripts: Sequence[Path] = []
        link_arg_iter = iter(self._link_arg_flags)
        for arg in link_arg_iter:
            if arg == "-lgcc":
                self._want_sysroot_libgcc = True
                continue
            left, sep, right = arg.partition("=")
            if left == "--sysroot":
                self._c_sysroot = Path(right)
                continue
            if left == "-fuse-ld":
                self._use_ld = Path(right)
                continue
            if left == "--soname":
                # `-Clink-arg=--soname=foo.so` is intended to set the DT_SONAME
                # for the shared object and is not an input or output file that
                # needs to be relativized.
                continue
            if left == "--reproduce":
                self._link_reproducer = Path(right)
                continue

            # Linker scripts can be passed via -T or --script.
            # They can be fused (-Tpath) or separate (-T path).
            script_path = None
            if arg == "-T" or arg == "--script":
                try:
                    script_path = next(link_arg_iter)
                except StopIteration:
                    # Trailing -T without arg, ignore for now
                    pass
            elif arg.startswith("-T"):
                script_path = arg[2:]
            elif arg.startswith("--script="):
                script_path = arg[len("--script=") :]

            if script_path:
                # Filter out symbol assignments like -Tfoo=0x1234.
                # Same logic as C++ wrapper.
                if "=" not in script_path:
                    self._linker_scripts.append(Path(script_path))
                continue

            if is_linkable(arg):
                self._link_arg_files.append(Path(arg))

    @property
    def env(self) -> Sequence[str]:
        return self._env

    @property
    def original_command(self) -> Sequence[str]:
        return self._original_command

    @property
    def rsp_expanded_command(self) -> Sequence[str]:
        return self._rsp_expanded_command

    @property
    def working_dir(self) -> Path:
        return self._working_dir

    @property
    def output_file(self) -> Path | None:
        return self._attributes.output  # usually this is the -o file

    @property
    def compiler(self) -> Path | None:
        return self._compiler

    @property
    def crate_type(self) -> CrateType:
        return self._crate_type

    @property
    def needs_linker(self) -> bool:
        return self.crate_type in {
            CrateType.BINARY,
            CrateType.PROC_MACRO,
            CrateType.DYLIB,
            CrateType.CDYLIB,
        }

    @property
    def main_output_is_executable(self) -> bool:
        return self.crate_type in {
            CrateType.BINARY,
            CrateType.DYLIB,
            CrateType.CDYLIB,
        }

    @property
    def want_sysroot_libgcc(self) -> bool:
        return self._want_sysroot_libgcc

    @property
    def link_reproducer(self) -> Path | None:
        return self._link_reproducer

    @property
    def target(self) -> str | None:
        return self._attributes.target

    @property
    def direct_sources(self) -> Sequence[Path]:
        return [
            s.file for s in self._direct_inputs if s.type == InputType.SOURCE
        ]

    @property
    def response_files(self) -> AbstractSet[Path]:
        return self._response_files

    @property
    def emit(self) -> Mapping[str, Sequence[str]]:
        return self._emit

    @property
    def emit_llvm_ir(self) -> bool:
        return "llvm-ir" in self.emit

    @property
    def emit_llvm_bc(self) -> bool:
        return "llvm-bc" in self.emit

    @property
    def emit_metadata(self) -> bool:
        return "metadata" in self.emit

    @property
    def save_analysis(self) -> bool:
        return (
            cl_utils.last_value_of_dict_flag(
                self._Z_flags, "save-analysis", "no"
            )
            == "yes"
        )

    @property
    def llvm_time_trace(self) -> bool:
        return "llvm-time-trace" in self._Z_flags

    @property
    def extra_filename(self) -> str:
        return cl_utils.last_value_of_dict_flag(self._C_flags, "extra-filename")

    @property
    def depfile(self) -> Path | None:
        d = cl_utils.last_value_of_dict_flag(self.emit, "dep-info", "")
        return Path(d) if d else None

    @property
    def rmeta(self) -> Path | None:
        if self.emit_metadata and self.output_file:
            return Path(
                cl_utils.last_value_of_dict_flag(
                    self.emit, "metadata", self._output_file_base + ".rmeta"
                )
            )
        return None

    @property
    def use_ld(self) -> Path | None:
        return self._use_ld

    @property
    def linker(self) -> Path | None:
        d = cl_utils.last_value_of_dict_flag(self._C_flags, "linker")
        return Path(d) if d else None

    @property
    def linker_scripts(self) -> Sequence[Path]:
        return [Path(p) for p in self._linker_scripts]

    @property
    def _link_arg_flags(self) -> Sequence[str]:
        return self._C_flags.get("link-arg", [])

    @property
    def link_arg_files(self) -> Sequence[Path]:
        return [Path(p) for p in self._link_arg_files]

    @property
    def link_map_output(self) -> Path | None:
        # The linker can produce a .map output file.
        for arg in self._C_flags.get("link-args", []):
            if arg.startswith("--Map="):
                return Path(_remove_prefix(arg, "--Map="))
        return None

    def default_rust_sysroot(self) -> Path:
        """This is the relative location of rust sysroot, when unspecified."""
        command = [str(self.compiler), "--print", "sysroot"]
        result = cl_utils.subprocess_call(
            command, cwd=self.working_dir, quiet=True
        )
        if result.returncode != 0:
            raise RuntimeError("Error: unable to infer default rust sysroot")
        # expect one line with the absolute path to the sysroot
        sysroot_abs = Path(result.stdout[0].strip())
        sysroot_rel = cl_utils.relpath(sysroot_abs, start=self.working_dir)
        return sysroot_rel

    @property
    def explicit_rust_sysroot(self) -> Path | None:
        """The sysroot specified on the command-line, if any."""
        return self._attributes.sysroot

    @property
    def ensure_rust_sysroot(self) -> Path:
        """This is where the target rustlibs for all platforms live."""
        return self.explicit_rust_sysroot or self.default_rust_sysroot()

    @property
    def c_sysroot(self) -> Path | None:
        return self._c_sysroot

    @property
    def native(self) -> Sequence[Path]:
        return [Path(p) for p in self._L_flags.get("native", [])]

    @property
    def native_link_arg_files(self) -> Iterable[Path]:
        for path in self.native:
            if path.is_dir():
                # TODO: debug print
                pass
            elif path.is_file():
                yield path
                # caller might need to prepend $build_dir

    @property
    def explicit_link_arg_files(self) -> Sequence[Path]:
        return [
            s.file for s in self._direct_inputs if s.type == InputType.LINKABLE
        ]

    @property
    def externs(self) -> dict[str, Path]:
        return self._extern

    def extern_paths(self) -> Iterable[Path]:
        yield from self.externs.values()

    @property
    def _output_file_base(self) -> str:
        """Removes any .rlib or .exe suffix to get the stem name."""
        # Returning str instead of Path because caller most likely
        # wants to append something to the result to form a Path name.
        if not self.output_file:
            raise RuntimeError(
                "Cannot infer stem name without a named -o output file"
            )
        return str(self.output_file.parent / self.output_file.stem)

    @property
    def _auxiliary_output_path(self) -> str:
        # Returning str instead of Path because caller most likely
        # wants to append something to the result to form a Path name.
        return self._output_file_base + self.extra_filename

    def extra_output_files(self) -> Iterable[Path]:
        base = self._auxiliary_output_path
        if self.emit_metadata and self.rmeta:
            yield self.rmeta
        if self.emit_llvm_ir:
            yield Path(base + ".ll")
        if self.emit_llvm_bc:
            yield Path(base + ".bc")
        if self.save_analysis:
            # Path() construction already normalizes away any leading './'
            analysis_file = Path(
                "save-analysis-temp",
                Path(self._auxiliary_output_path + ".json").name,
            )
            yield analysis_file
        if self.llvm_time_trace:
            trace_file = Path(self._output_file_base + ".llvm_timings.json")
            yield trace_file
        link_map = self.link_map_output
        if link_map:
            yield link_map
        link_repro = self.link_reproducer
        if link_repro:
            yield link_repro

    def dep_only_command_with_rspfiles(
        self, depfile_name: str
    ) -> tuple[Sequence[str], Sequence[Path]]:
        """Generate a command that only produces a depfile.

        Returns:
          modified command
          rewritten response files (which appear in the modified command)
        """
        # In case the original command contains additional levels of wrapping,
        # interpret '--' as a wrapping separator, and take the innermost (last)
        # subsequence as the relevant rustc command, for the purpose of
        # dependency scanning.
        innermost_command = list(
            cl_utils.split_into_subsequences(self.original_command, "--")
        )[-1]
        return rustc_dep_only_command(
            command_tokens=innermost_command,
            emit_metadata=self.emit_metadata,
            depfile_name=depfile_name,
        )


# TODO: write a main() routine just for printing debug info about a compile
# command
