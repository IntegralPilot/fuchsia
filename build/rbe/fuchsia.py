#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Fuchsia-specific constants, functions, conventions.

This includes information like:
  * organization of prebuilt tools
  * path conventions
"""

import os
import platform
import sys
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path
from typing import (
    AbstractSet,
)

_SCRIPT_PATH = Path(__file__)
_SCRIPT_BASENAME = _SCRIPT_PATH.name
_SCRIPT_DIR = _SCRIPT_PATH.parent
_SCRIPT_DIR_REL = Path(os.path.relpath(_SCRIPT_DIR, start=os.curdir))

_EXPECTED_ROOT_SUBDIRS = ("boards", "bundles", "prebuilt", "zircon")


def _dir_is_fuchsia_root(path: Path) -> bool:
    # Look for the presence of certain files and dirs.
    # Cannot always expect .git dir at the root, in the case of
    # a copy or unpacking from archive.
    for d in _EXPECTED_ROOT_SUBDIRS:
        if not (path / d).is_dir():
            return False
    return True


def project_root_dir() -> Path:
    """Returns the root location of the source tree.

    This works as expected when this module is loaded from the
    original location in the Fuchsia source tree.
    However, when this script is copied into a zip archive (as is done
    for `python_host_test` (GN)), it may no longer reside somewhere inside
    the Fuchsia source tree after it is unpacked.
    """
    d = _SCRIPT_DIR.absolute()
    while True:
        if d.name.endswith(".pyz"):
            # If this point is reached, we are NOT operating in the original
            # location in the source tree as intended.  Treat this like a test
            # and return a fake value.
            return Path("/FAKE/PROJECT/ROOT/FOR/TESTING")
        elif _dir_is_fuchsia_root(d):
            return d
        next = d.parent
        if next == d:
            raise Exception(
                f"Unable to find project root searching upward from {_SCRIPT_DIR}."
            )
        d = next


# This script starts/stops reproxy around a command.
REPROXY_WRAP = _SCRIPT_DIR_REL / "fuchsia-reproxy-wrap.sh"

##########################################################################
### Prebuilt tools
##########################################################################


def _prebuilt_platform_subdir() -> str:
    """Naming convention of prebuilt tools."""
    os_name = {"linux": "linux", "darwin": "mac"}[sys.platform]
    arch = {"x86_64": "x64", "arm64": "arm64", "aarch64": "arm64"}[platform.machine()]
    return f"{os_name}-{arch}"


HOST_PREBUILT_PLATFORM = _prebuilt_platform_subdir()
HOST_PREBUILT_PLATFORM_SUBDIR = HOST_PREBUILT_PLATFORM

# RBE workers are only linux-x64 at this time, so all binaries uploaded
# for remote execution should use this platform.
# This is also used as a subdir component of prebuilt tool paths.
REMOTE_PLATFORM = "linux-x64"


def _path_component_is_platform_subdir(dirname: str) -> bool:
    os, sep, arch = dirname.partition("-")
    return sep == "-" and os in {"linux", "mac"} and arch in {"x64", "arm64"}


def _remote_executable_components(host_tool: Path) -> Iterable[str]:
    """Change the path component that correpsonds to the platform."""
    for d in host_tool.parts:
        if _path_component_is_platform_subdir(d):
            yield REMOTE_PLATFORM
        else:
            yield d


def remote_executable(host_tool: Path) -> Path:
    """Locate a corresponding tool for remote execution.

    This assumes that 'linux-x64' is the only available remote execution
    platform, and that the tools are organized by platform next to each other.

    Args:
      host_tool (str): path to an executable for the host platform.

    Returns:
      path to the corresponding executable for the remote execution platform,
      which is currently only linux-x64.
    """
    return Path(*list(_remote_executable_components(host_tool)))


RECLIENT_BINDIR = Path(
    "prebuilt",
    "third_party",
    "reclient",
    HOST_PREBUILT_PLATFORM_SUBDIR,
)

REMOTE_RUSTC_SUBDIR = Path("prebuilt", "third_party", "rust", REMOTE_PLATFORM)
REMOTE_CLANG_SUBDIR = Path("prebuilt", "third_party", "clang", REMOTE_PLATFORM)
REMOTE_GCC_SUBDIR = Path("prebuilt", "third_party", "gcc", REMOTE_PLATFORM)

# TODO(https://fxbug.dev/42076379): use platform-dependent location
# Until then, this remote fsatrace only works from linux-x64 hosts.
FSATRACE_PATH = Path("prebuilt", "fsatrace", "fsatrace")

_CHECK_DETERMINISM_SCRIPT = Path("build", "tracer", "output_cacher.py")


def check_determinism_command(
    exec_root: Path,
    outputs: Sequence[Path],
    command: Iterable[str] | None = None,
    max_attempts: int | None = None,
    miscomparison_export_dir: Path | None = None,
    label: str | None = None,
) -> list[str]:
    """Returns a command that checks for output determinism.

    The check runs locally twice, moving outputs out of the way temporarily,
    and then compares.

    Args:
      exec_root: path to project root (relative or absolute).
      outputs: output files to compare.
      command: the command to execute.
      max_attempts: number of times to re-run and compare.
      miscomparison_export_dir: location to store mismatched outputs.
      label: build system identifier for diagnostics.
    """
    return list(
        [
            sys.executable,  # same Python interpreter
            "-S",
            str(exec_root / _CHECK_DETERMINISM_SCRIPT),
        ]
        + ([f"--label={label}"] if label else [])
        + [
            "--check-repeatability",
        ]
        + ([f"--max-attempts={max_attempts}"] if max_attempts else [])
        + (
            [f"--miscomparison-export-dir={miscomparison_export_dir}"]
            if miscomparison_export_dir
            else []
        )
        + [
            "--outputs",
        ]
        + [str(p) for p in outputs]
        + ["--"]
        + (list(command) if command else [])
    )


def determinism_repetitions(paths: Iterable[Path]) -> int | None:
    """Override the maximum number of determinism repetitions for specific files."""
    # For https://fxbug.dev/42080457: Increase repetition count to increase
    # chances of repro in infra.
    if any("libminfs.vnode.cc" in str(path) for path in paths):
        # Historically, this failed around 5% of the time in infra.
        return 100
    if any("libstarnix_core.rlib" in path.name for path in paths):
        return 10  # See b/328756843 for failures.
    return None


# On platforms where ELF utils are unavailable, hardcode rustc's shlibs.
def remote_rustc_shlibs(root_rel: Path) -> Iterable[Path]:
    for g in ("librustc_driver-*.so", "libstd-*.so", "libLLVM-*-rust-*.so"):
        yield from (root_rel / REMOTE_RUSTC_SUBDIR / "lib").glob(g)


def clang_runtime_libdirs(
    clang_dir_rel: Path, target_triple: str
) -> list[Path]:
    """Locate clang runtime libdir from the given toolchain directory."""
    return list(
        (clang_dir_rel / "lib" / "clang").glob(
            os.path.join(
                "*",  # a clang version number, like '14.0.0' or '17'
                "lib",
                target_triple,
            )
        )
    )


def clang_libcxx_static(clang_dir_rel: Path, clang_lib_triple: str) -> Path:
    """Location of libc++.a"""
    return clang_dir_rel / "lib" / clang_lib_triple / "libc++.a"


def gcc_support_tools(
    gcc_path: Path,
    parser: bool = False,
    assembler: bool = False,
    linker: bool = False,
) -> Iterable[Path]:
    bindir = gcc_path.parent
    # expect compiler to be named like {x64_64,aarch64}-elf-{g++,gcc}
    try:
        arch, objfmt, tool = gcc_path.name.split("-")
    except ValueError:
        raise ValueError(
            f'Expecting compiler to be named like {{arch}}-{{objfmt}}-[gcc|g++], but got "{gcc_path.name}"'
        )
    target = "-".join([arch, objfmt])
    install_root = bindir.parent
    if assembler:
        yield install_root / target / "bin/as"

    libexec_base = install_root / "libexec/gcc" / target
    libexec_dirs = list(libexec_base.glob("*"))  # dir is a version number
    if not libexec_dirs:
        return

    libexec_dir = Path(libexec_dirs[0])
    if parser:
        parsers = {"gcc": "cc1", "g++": "cc1plus"}
        yield libexec_dir / parsers[tool]

    if linker:
        yield libexec_dir / "collect2"
        yield libexec_dir / "lto-wrapper"
        yield from (install_root / target / "bin").glob("ld*")  # ld linkers

    # Workaround: gcc builds a COMPILER_PATH to its related tools with
    # non-normalized paths like:
    # ".../gcc/linux-x64/bin/../lib/gcc/x86_64-elf/12.2.1/../../../../x86_64-elf/bin"
    # The problem is that every partial path of the non-normalized path needs
    # to exist, even if nothing in the partial path is actually used.
    # Here we need the "lib/gcc/x86_64-elf/VERSION" path to exist in the
    # remote environment.  One way to achieve this is to pick an arbitrary
    # file in that directory to include as a remote input, and all of its
    # parent directories will be created in the remote environment.
    version = libexec_dir.name
    # need this dir to exist remotely:
    lib_base = install_root / "lib/gcc" / target / version
    if linker:
        yield lib_base / "libgcc.a"  # used during collect2
    else:
        yield lib_base / "crtbegin.o"  # arbitrary unused file, just to setup its dir


def rust_stdlib_subdir(target_triple: str) -> Path:
    """Location of rustlib standard libraries relative to rust --sysroot.

    This depends on where target libdirs live relative to the rustc compiler.
    It is possible that the target libdirs may move to a location separate
    from where the host tools reside.  See https://fxbug.dev/42063018.
    """
    return Path("lib") / "rustlib" / target_triple / "lib"


def rustc_target_to_sysroot_triple(target: str) -> str:
    if target.startswith("aarch64-") and "-linux" in target:
        return "aarch64-linux-gnu"
    if target.startswith("riscv64gc-") and "-linux" in target:
        return "riscv64-linux-gnu"
    if target.startswith("x86_64-") and "-linux" in target:
        return "x86_64-linux-gnu"
    if target.endswith("-fuchsia"):
        return ""
    if target.startswith("wasm32-"):
        return ""
    raise ValueError(f"unhandled case for sysroot target subdir: {target}")


def clang_target_to_sysroot_triple(target: str) -> str:
    return target.replace("-unknown-", "")


def clang_target_to_libdir(target: str) -> str:
    if "fuchsia" in target and "-unknown-" not in target:
        return target.replace("-fuchsia", "-unknown-fuchsia")
    return target


def rustc_target_to_clang_target(target: str) -> str:
    """Maps a rust target triple to a clang target triple."""
    # These mappings were determined by examining the options available
    # in the clang lib dir, and verifying against traces of libraries accessed
    # by local builds.
    if target == "aarch64-unknown-fuchsia" or (
        target.startswith("aarch64-") and target.endswith("-fuchsia")
    ):
        return "aarch64-unknown-fuchsia"
    if target == "aarch64-linux-gnu" or (
        target.startswith("aarch64-") and target.endswith("-linux-gnu")
    ):
        return "aarch64-unknown-linux-gnu"
    if target == "riscv64gc-fuchsia" or (
        target.startswith("riscv64gc-") and target.endswith("-fuchsia")
    ):
        return "riscv64-unknown-fuchsia"
    if target == "riscv64gc-linux-gnu" or (
        target.startswith("riscv64gc-") and target.endswith("-linux-gnu")
    ):
        return "riscv64-unknown-linux-gnu"
    if target == "x86_64-unknown-fuchsia" or (
        target.startswith("x86_64-") and target.endswith("-fuchsia")
    ):
        return "x86_64-unknown-fuchsia"
    if target == "x86_64-linux-gnu" or (
        target.startswith("x86_64-") and target.endswith("-linux-gnu")
    ):
        return "x86_64-unknown-linux-gnu"
    if target == "wasm32-unknown-unknown":
        return "wasm32-unknown-unknown"

    if target == "x86_64-apple-darwin":
        return "x86_64-apple-darwin"

    raise ValueError(
        f"Unhandled case for mapping to clang lib target dir: {target}"
    )


_REMOTE_RUST_LLD_RELPATH = Path(
    "../lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
)


def remote_rustc_to_rust_lld_path(rustc: Path) -> Path:
    # remote is only linux-64
    rust_lld = rustc.parent / _REMOTE_RUST_LLD_RELPATH
    return rust_lld  # already normalized by Path construction


def _versioned_libclang_dir(
    clang_path_rel: Path,
) -> Path:
    clang_root = clang_path_rel.parent.parent
    libclang_root = clang_root / "lib" / "clang"
    # Expect exactly one versioned libclang dir installed.
    return next(libclang_root.glob("*"))


def _clang_sanitizer_share_files(
    versioned_share_dir: Path,
    sanitizers: AbstractSet[str],
) -> Iterable[Path]:
    if "address" in sanitizers:
        yield versioned_share_dir / "asan_ignorelist.txt"
    if "hwaddress" in sanitizers:
        yield versioned_share_dir / "hwasan_ignorelist.txt"
    if "memory" in sanitizers:
        yield versioned_share_dir / "msan_ignorelist.txt"


def _force_target_runtime_libdirs_b354016617(
    clang_path_rel: Path,
    target: str,
) -> Iterable[Path]:
    """Force target runlib libdirs to exist remotely.

    See b/354016617.  Pick an arbitrary lib from each
    wanted directory to force that directory to exist remotely.
    The presence of these directories influences the compiler
    driver (unfortunately).

    This workaround can be removed once the referenced bug is fixed.
    """
    clang_root = clang_path_rel.parent.parent
    libclang_versioned = _versioned_libclang_dir(clang_path_rel)
    target_libdir = clang_target_to_libdir(target)
    libclang_target_dir = libclang_versioned / "lib" / target_libdir
    stdlibs_dir = clang_root / "lib" / target_libdir
    if "-windows-" in target:
        clangrt_lib = "clang_rt.builtins.lib"
        cxx_lib = "libc++.lib"
    else:
        clangrt_lib = "libclang_rt.builtins.a"
        cxx_lib = "libc++.a"

    yield libclang_target_dir / clangrt_lib
    yield from stdlibs_dir.glob(f"*/{cxx_lib}")


def remote_clang_compiler_toolchain_inputs(
    clang_path_rel: Path,
    target: str,
    sanitizers: frozenset[str],
) -> Iterable[Path]:
    """List compiler support files.

    Kludge: partially hardcode set of support files needed from the
    toolchain for certain sanitizer modes.

    Yields:
      Paths to toolchain files needed for compiling.
    """
    libclang_versioned = _versioned_libclang_dir(clang_path_rel)
    versioned_share_dir = libclang_versioned / "share"
    yield from _clang_sanitizer_share_files(versioned_share_dir, sanitizers)
    # Workaround b/354016617: create target/variant dirs remotely
    yield from _force_target_runtime_libdirs_b354016617(
        clang_path_rel=clang_path_rel,
        target=target,
    )


def remote_clang_linker_toolchain_inputs(
    clang_path_rel: Path,
    target: str,
    shared: bool,
    rtlib: str | None,
    unwindlib: str,
    profile: bool,
    sanitizers: AbstractSet[str],
    want_all_libclang_rt: bool,
) -> Iterable[Path]:
    """List linker support libraries.

    Kludge: partially hardcode built-in libraries until linker tools can
    quickly discover the needed libraries to link remotely.
    See https://fxbug.dev/42084853

    Args:
      clang_path_rel: path to clang executable
      target: target platform triple
      shared: True if building shared library
      rtlib: the compiler runtime library name
      unwindlib: unwind library name
      profile: True if profile runtime is wanted
      sanitizers: Set of sanitizers that are enabled (and their runtimes needed)
      want_all_libclang_rt: if True, grab the entire directory of libclang_rt
        runtime libraries for all variants without trying to pick based
        on options.  This ends up including more input files for remote
        execution, but is less fragile and less prone to compiler driver
        changes.

    Yields:
      Paths to libraries needed for linking.
    """
    clang_root = clang_path_rel.parent.parent
    libclang_versioned = _versioned_libclang_dir(clang_path_rel)
    target_libdir = clang_target_to_libdir(target)
    libclang_target_dir = libclang_versioned / "lib" / target_libdir
    versioned_share_dir = libclang_versioned / "share"

    if want_all_libclang_rt:
        yield versioned_share_dir
        yield libclang_target_dir
    else:  # pick subset of rt libs based on options.
        if rtlib == "compiler-rt":
            yield libclang_target_dir / "clang_rt.crtbegin.o"
            yield libclang_target_dir / "clang_rt.crtend.o"

        yield libclang_target_dir / "libclang_rt.builtins.a"

        yield from _clang_sanitizer_share_files(versioned_share_dir, sanitizers)

        # Including both static and shared libraries, because one cannot
        # deduce from the command-line alone which will be needed.
        if "address" in sanitizers:
            yield from libclang_target_dir.glob("libclang_rt.asan*")
        if "hwaddress" in sanitizers:
            yield from libclang_target_dir.glob("libclang_rt.hwasan*")
        if "leak" in sanitizers:
            yield from libclang_target_dir.glob("libclang_rt.lsan*")
        if "memory" in sanitizers:
            yield from libclang_target_dir.glob("libclang_rt.msan*")
        if "fuzzer" in sanitizers or "fuzzer-no-link" in sanitizers:
            yield from libclang_target_dir.glob("libclang_rt.fuzzer*")
        if "thread" in sanitizers:
            yield from libclang_target_dir.glob("libclang_rt.tsan*")
        if "undefined" in sanitizers:
            yield from libclang_target_dir.glob("libclang_rt.ubsan*")

        if profile:
            yield from libclang_target_dir.glob("libclang_rt.profile*")

    stdlibs_dir = clang_root / "lib" / target_libdir
    # This directory includes variants like asan, noexcept.
    # Let the toolchain pick which one it needs.
    yield stdlibs_dir  # grab the entire directory
    # yield stdlibs_dir / "libc++.a"
    # if unwindlib:
    #     yield stdlibs_dir / (unwindlib + ".a")


def remote_gcc_linker_toolchain_inputs(
    gcc_path_rel: Path,
) -> Iterable[Path]:
    return []


# Built lib/{sysroot_triple}/... files
# Entries here are not already covered by some other linker script.
_SYSROOT_LIB_FILES = (
    "libm.so.6",
    "librt.so.1",
    "libutil.so.1",
)

# Built /usr/lib/{sysroot_triple}/... files
_SYSROOT_USR_LIB_FILES = (
    "libpthread.a",
    "libm.so",
    "libm.a",
    "librt.so",
    "librt.a",
    "libdl.so",
    "libdl.a",
    "libutil.so",
    "libutil.a",
    "Scrt1.o",
    "crt1.o",
    "crti.o",
    "crtn.o",
)

# These are known linker scripts that need to be read
# to locate other underlying files needed.
_SYSROOT_USR_LIB_LINKER_SCRIPTS = (
    "libc.so",
    "libm.so",
    "libpthread.so",
    "libmvec.so",
)


def c_sysroot_files(
    sysroot_dir: Path,
    sysroot_triple: str,
    with_libgcc: bool,
    linker_script_expander: Callable[[Sequence[Path]], Iterable[Path]],
) -> Iterable[Path]:
    """Expanded list of sysroot files under the Fuchsia build output dir.

    TODO: cache this per build to avoid repeating deduction

    Args:
      sysroot_dir: path to the sysroot, relative to the working dir.
      sysroot_triple: platform-specific subdir of sysroot, based on target.
      with_libgcc: if using `-lgcc`, include additions libgcc support files.
      linker_script_expander: function that expands linkable inputs to the set
        of files referenced by it (possibly through linker scripts).

    Yields:
      paths to sysroot files needed for remote/sandboxed linking,
      all relative to the current working dir.
    """
    if sysroot_triple:
        for f in _SYSROOT_LIB_FILES:
            yield sysroot_dir / "lib" / sysroot_triple / f
        for f in _SYSROOT_USR_LIB_FILES:
            yield sysroot_dir / "usr/lib" / sysroot_triple / f

        maybe_scripts = (
            sysroot_dir / "usr/lib" / sysroot_triple / f
            for f in _SYSROOT_USR_LIB_LINKER_SCRIPTS
        )
        yield from linker_script_expander(
            [f_path for f_path in maybe_scripts if f_path.is_file()]
        )

        libmvec_path = sysroot_dir / "usr/lib" / sysroot_triple / "libmvec.a"
        if libmvec_path.is_file():
            yield libmvec_path

        if with_libgcc:
            yield from [
                sysroot_dir / "usr/lib/gcc" / sysroot_triple / "4.9/libgcc.a",
                sysroot_dir
                / "usr/lib/gcc"
                / sysroot_triple
                / "4.9/libgcc_eh.a",
                # The toolchain probes (stat) for the existence of crtbegin.o
                sysroot_dir / "usr/lib/gcc" / sysroot_triple / "4.9/crtbegin.o",
                # libgcc also needs sysroot libc.a,
                # although this might be coming from -Cdefault-linker-libraries.
                sysroot_dir / "usr/lib" / sysroot_triple / "libc.a",
            ]
    else:
        yield from linker_script_expander(
            [
                sysroot_dir / "lib/libc.so",
                sysroot_dir / "lib/libdl.so",
                sysroot_dir / "lib/libm.so",
                sysroot_dir / "lib/libpthread.so",
                sysroot_dir / "lib/librt.so",
                sysroot_dir / "lib/Scrt1.o",
            ]
        )

        # Not every sysroot dir has a libzircon.
        libzircon_so = sysroot_dir / "lib/libzircon.so"
        if libzircon_so.is_file():
            yield libzircon_so


def rust_source_file_listing_disabled(sources: Sequence[Path]) -> bool:
    return len(sources) == 1 and sources[0].name == "__SKIP_ENFORCEMENT__.rs"
