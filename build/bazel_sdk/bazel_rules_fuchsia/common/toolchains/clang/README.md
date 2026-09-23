# Common Bazel C++ toolchain functions

This directory contains common Starlark functions that can be used to
create custom Bazel external repositories containing definitions for Bazel
C++ toolchain instances that use a prebuilt Clang toolchain directory.

This is only written to support the Fuchsia platform build, and in the near
future, the Fuchsia Bazel SDK's fuchsia_clang_repository() rule.

Users are expected to create a custom repository rule that does the
following:

1. Call `prepare_clang_repository()`, defined in `repository_utils.bzl`,
   passing a `repository_ctx object` as well as the path to the Clang
   installation directory, relative to the main project workspace.

   This function will populate the directory with a few important files,
   but will _not_ generate a WORKSPACE.bazel or BUILD.bazel file.


2. Create a `BUILD.bazel` file whose content will be used to define
   Bazel C++ toolchain definitions. This file must call
   `setup_clang_repository()`, also defined in `repository_utils.bzl`,
   to define a few important targets.

   After that, the same file can call `generate_clang_cc_toolchain()`,
   defined in `toolchain_utils.bzl` to create one Bazel C++ toolchain
   instance, targeting a specific target os/cpu pair.

3. The WORKSPACE.bazel file must call register_toolchains() to register
   the toolchains defined in the `BUILD.bazel` file previously, after
   creating the external repository.
