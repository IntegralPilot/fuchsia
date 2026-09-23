# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities related to Clang repositories. See README.md for details."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("//common:repository_utils.bzl", "get_fuchsia_host_arch", "get_fuchsia_host_os")
load("//common:toolchains/clang/clang_utils.bzl", "process_clang_builtins_output")
load("//common:toolchains/clang/providers.bzl", "ClangInfo")
load("//common:toolchains/clang/sanitizer.bzl", "define_clang_sanitizer_config_settings")
load("//common:toolchains/clang/toolchain_utils.bzl", "define_clang_runtime_filegroups")

def prepare_clang_repository(repo_ctx, clang_install_dir, needs_symlinks = True):
    """Prepare a repository directory for a clang toolchain creation.

    This function must be called from a repository rule, and creates a number
    of files inside it.

    The files created by this function are:

      - Symlinks to the clang bin/, lib/ and other directories.

      - On MacOS, symlinks xcode/MacSDK/usr and xcode/MacSDK/Frameworks
        pointing to system-installed XCode OSX SDK directories.

      - An `empty` file which is ... empty.

      - A `generated_constants.bzl` file that exports values corresponding
        to the Clang toolchain, and which can be loaded at load-time (e.g.
        from BUILD.bazel files).

      - A `debug_probe_output.txt` which is used to debug how the content of
        `generated_constants.bzl` was computed. Only used for debugging.

    This expects the caller to add a BUILD.bazel file to the repository that
    will call setup_clang_repository(), passing the value written into
    generated_constants.bzl to it.

    After this, functions from toolchain_utils.bzl can be used to create
    individual Clang Bazel toolchain instances.

    Args:
      repo_ctx: A repository_ctx value.
      clang_install_dir: Path to the clang prebuilt toolchain installation,
          relative to the main project workspace.
      needs_symlinks: Whether we need to symlink the content to an outside directory
    """
    workspace_dir = str(repo_ctx.workspace_root)

    # Symlink the content of the clang installation directory into
    # the repository directory.

    # Symlink top-level items from Clang prebuilt install to repository directory
    # Note that this is possible because our C++ toolchain configuration redefine
    # the "dependency_file" feature to use relative file paths.
    if paths.is_absolute(clang_install_dir):
        clang_install_path = repo_ctx.path(clang_install_dir)
    else:
        clang_install_path = repo_ctx.path(workspace_dir + "/" + clang_install_dir)

    # We only need to symlink when the contents exist outside of the repository.
    if needs_symlinks:
        for f in clang_install_path.readdir():
            repo_ctx.symlink(f, f.basename)

    # Extract the builtin include paths by running the executable once.
    # Only care about C++ include paths. The list for compiling C is actually
    # smaller, but this is not an issue for now.
    #
    # Note that the paths must be absolute for Bazel, which matches the
    # output of the command, so there is not need to change them.
    #
    # Note however that Bazel will use relative paths when creating the list
    # of inputs for C++ compilation actions.

    # Create an empty file to be pre-processed. This is more portable than
    # trying to use /dev/null as the input.
    repo_ctx.file("empty", "")
    command = ["bin/clang", "-E", "-x", "c++", "-v", "./empty"]
    result = repo_ctx.execute(command)

    # Write the result to a file for debugging.
    repo_ctx.file("debug_probe_results.txt", result.stderr)

    short_version, long_version, builtin_include_paths = \
        process_clang_builtins_output(result.stderr)

    # Clang places a number of built-in headers (e.g. <mmintrin.h>) and
    # runtime libraries (e.g. libclang_rt.builtins.a) under a directory
    # whose location has changed over time.
    #
    # It used to be lib/clang/<long_version>/, but apparently this has
    # changed to lib/clang/<short_version>/ in clang-16. Support both schemes
    # by probing the file system.
    #
    lib_clang_internal_dir = None
    for version in [long_version, short_version]:
        candidate_dir = "lib/clang/" + version
        if repo_ctx.path(candidate_dir).exists:
            lib_clang_internal_dir = candidate_dir
            break

    if not lib_clang_internal_dir:
        fail("Could not find lib/clang/<version> directory!?")

    # Now convert that into a string that can go into a .bzl file.
    builtin_include_paths_str = "\n".join(["    \"%s\"," % path for path in builtin_include_paths])

    repo_ctx.file("generated_constants.bzl", content = '''
constants = struct(
  long_version = "{long_version}",
  short_version = "{short_version}",
  lib_clang_internal_dir = "{lib_clang_internal_dir}",
  builtin_include_paths = [
{builtin_paths}
  ],
  fuchsia_host_arch = "{fuchsia_host_arch}",
  fuchsia_host_os = "{fuchsia_host_os}",
)
'''.format(
        long_version = long_version,
        short_version = short_version,
        lib_clang_internal_dir = lib_clang_internal_dir,
        builtin_paths = builtin_include_paths_str,
        fuchsia_host_arch = get_fuchsia_host_arch(repo_ctx),
        fuchsia_host_os = get_fuchsia_host_os(repo_ctx),
    ))

def _clang_info_target_impl(ctx):
    return [ClangInfo(
        short_version = ctx.attr.short_version,
        long_version = ctx.attr.long_version,
        builtin_include_paths = ctx.attr.builtin_include_paths,
        fuchsia_host_arch = ctx.attr.fuchsia_host_arch,
        fuchsia_host_os = ctx.attr.fuchsia_host_os,
    )]

_clang_info_target = rule(
    implementation = _clang_info_target_impl,
    attrs = {
        "short_version": attr.string(
            doc = "Clang short version",
            mandatory = True,
        ),
        "long_version": attr.string(
            doc = "Clang long version",
            mandatory = True,
        ),
        "builtin_include_paths": attr.string_list(
            doc = "List of Clang builtin include paths, must be absolute.",
            mandatory = True,
            allow_empty = True,
        ),
        "fuchsia_host_arch": attr.string(
            doc = "Host cpu name, using Fuchsia conventions.",
            mandatory = True,
        ),
        "fuchsia_host_os": attr.string(
            doc = "Host os name, using Fuchsia conventions.",
            mandatory = True,
        ),
    },
)

# buildifier: disable=unnamed-macro
def setup_clang_repository(constants):
    """Create a few required targets in a Clang repository.

    This function should be called early from the top-level BUILD.bazel
    in the Clang repository. It creates a few targets that functions in
    toolchain_utils.bzl rely on.

    The top-level targets will be:

    - @repo//:clang_info

      A target exposing the generated_constants.bzl values through
      a ClangInfo provider, useful to access them at analysis time.

    - @repo//:clang_all

      A filegroup() covering all files requires for actions that need
      to access all file from the remote.

    - @repo//:cc-compiler-prebuilts

      A filegroup() covering the binary tools that must appear in the
      sandbox for all compile actions (independent from the target
      architecture).

    - @repo//:cc-linker-prebuilts

      A filegroup() covering the binary tools that must appear in the
      sandbox for all linking actions (independent from the target
      architecture).

    - @repo//:ar, @repo//:objcopy, @repo//:objdump, @repo//:nm, @repo//:strip

      Each one is a filegroup() covering the binaries needed in the sandbox
      for Bazel actions that perform one of the corresponding actions.

    - @repo//:libunwind-headers

      A filegroup exposing the libunwind headers.

    Args:
      constants: The value written to generated_constants.bzl by
         a call to prepare_clang_repository() that was performed in the
         repository rule for the current Clang repository.
    """

    # Define the top-level `clang_info` target used to expose
    # the content of generated_constants.bzl to rule implementation functions.
    _clang_info_target(
        name = "clang_info",
        short_version = constants.short_version,
        long_version = constants.long_version,
        builtin_include_paths = constants.builtin_include_paths,
        fuchsia_host_arch = constants.fuchsia_host_arch,
        fuchsia_host_os = constants.fuchsia_host_os,
    )

    define_clang_runtime_filegroups(constants)

    # The following filegroups are referenced from toolchain definitions
    # created by the generate_clang_cc_toolchain() function from
    # toolchain_utils.bzl.

    native.filegroup(
        name = "clang_all",
        srcs = native.glob(
            ["**/*"],
            exclude = ["**/*.html", "**/*.pdf", "xcode/**"],
        ),
    )

    native.filegroup(
        name = "cc-compiler-prebuilts",
        srcs = [
            "//:bin/clang",
            "//:bin/clang++",
            "//:bin/llvm-nm",
            "//:bin/llvm-strip",
        ],
    )

    native.filegroup(
        name = "cc-linker-prebuilts",
        srcs = [
            "//:bin/clang",
            "//:bin/ld.lld",
            "//:bin/ld64.lld",
            "//:bin/lld",
            "//:bin/lld-link",
        ],
    )

    native.filegroup(
        name = "ar",
        srcs = ["//:bin/llvm-ar"],
    )

    native.filegroup(
        name = "objcopy",
        srcs = ["//:bin/llvm-objcopy"],
    )

    native.filegroup(
        name = "objdump",
        srcs = ["//:bin/llvm-objdump"],
    )

    native.filegroup(
        name = "strip",
        srcs = ["//:bin/llvm-strip"],
    )

    native.filegroup(
        name = "nm",
        srcs = ["//:bin/llvm-nm"],
    )

    native.filegroup(
        name = "libunwind-headers",
        srcs = [
            "include/libunwind.h",
            "include/libunwind.modulemap",
            "include/mach-o/compact_unwind_encoding.h",
            "include/unwind.h",
            "include/unwind_arm_ehabi.h",
            "include/unwind_itanium.h",
        ],
    )

    define_clang_sanitizer_config_settings()

def _empty_host_cpp_toolchain_repository_impl(repo_ctx):
    _BUILD_BAZEL_CONTENT = """
load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
load("@rules_cc//cc/toolchains:cc_toolchain.bzl", "cc_toolchain")
load("//common:toolchains/clang/toolchain_utils.bzl", "empty_cc_toolchain_config")

package(default_visibility = ["//visibility:public"])

# An empty filegroup.
filegroup(
  name = "empty",
  srcs = [],
)

empty_cc_toolchain_config(
  name = "empty_cc_toolchain_config",
)

cc_toolchain(
  name = "empty_cc_toolchain",
  all_files = ":empty",
  ar_files = ":empty",
  as_files = ":empty",
  compiler_files = ":empty",
  dwp_files = ":empty",
  linker_files = ":empty",
  objcopy_files = ":empty",
  strip_files = ":empty",
  supports_param_files = 0,
  toolchain_config = ":empty_cc_toolchain_config",
  toolchain_identifier = "empty_cc_toolchain",
)

toolchain(
  name = "empty_cpp_toolchain",
  exec_compatible_with = HOST_CONSTRAINTS,
  target_compatible_with = HOST_CONSTRAINTS,
  toolchain = ":empty_cc_toolchain",
  toolchain_type = "@bazel_tools//tools/cpp:toolchain_type",
)
"""
    repo_ctx.file("WORKSPACE.bazel", "")
    repo_ctx.symlink(repo_ctx.path(Label("//common:BUILD.bazel")).dirname, "common")
    repo_ctx.file("BUILD.bazel", _BUILD_BAZEL_CONTENT)

empty_host_cpp_toolchain_repository = repository_rule(
    implementation = _empty_host_cpp_toolchain_repository_impl,
    doc = """Generate a repository that contains an empty C++ toolchain definition.

Useful when running on machines without an installed GCC or Clang.
Usage example, from a WORKSPACE.bazel file:

  load(
      "//common:toolchains/clang/repository_utils.bzl",
      "empty_host_cpp_toolchain_repository",
  )

  empty_host_cpp_toolchain_repository(
    name = "no_host_cpp",
  )

  register_toolchain("@no_host_cpp:empty_cpp_toolchain")

""",
)
