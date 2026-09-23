# TECHNICAL NOTE ON DEBUG SYMBOL GENERATION

## Foreword

The following information corresponds to the following versions
of Bazel and third-party rulesets, and is subject to change over
time:

- Bazel 7.5
- `@rules_cc` version 0.1.1
- `@rules_rust` version 0.59.1
- `@rules_go` version 0.54.0

Given that most of these details are completely undocumented, they
may change when upgrading any versions of these dependencies.

## Overview and motivation

This document outlines how debug symbols are managed in Bazel and how the
Fuchsia in-tree build configures this process to achieve the following
goals:

- Generating unstripped binaries, bearing debug symbols, independent
  of the `--compilation_mode` value being used.

- Generating stripped binaries for distribution (e.g. inside Fuchsia
  packages, as prebuilt IDK binaries or IDK host tools).

It details the default Bazel behavior for C++, Rust and Go, and explains
the specific mitigations applied by the Fuchsia in-tree build.

## Bazel debug symbol generation handling.

To generate binaries that contain debug symbols, two things are
necessary:

- Compilation must ensure that object files and libraries it produces
  contain debug symbols.

- Linking must *not* strip the debug symbols when generating binaries.

There is also an alternative way to produce split debug/code artifacts,
named "debug fission" or "split dwarf" that will not be covered here,
though Bazel can support it.

Bazel language-specific rulesets and toolchain definitions have slightly
different default behavior and implementations, but they all use two
flags to primarily control Bazel's behavior:

- `--compilation_mode`: Can be `dbg`, `fastbuild` or `opt`. This flag
  influences whether debug symbols are generated during compilation.

- [`--strip`][strip_flag]: Can be `never`, `always`, or `sometimes`
  (the default). This flag controls whether link commands strip debug
  symbols from the final binary.

  The default `--strip=sometimes` means that stripping should
  happen if the compilation mode is `fastbuild` only.

### C++

The C++ toolchain that `@rules_cc` automatically configures by probing
the host system will use the following logic:

- **`dbg`**: compilation produces debug symbols. Linking keeps them
  unless `--strip=always` is used.

- **`fastbuild`**: compilation does not produce debug symbols.
  Linking strips binaries anyway, unless `--strip=never` is used.

- **`opt`**: compilation does not produce debug symbols, and
  linking never strips, unless `--strip=always` is used.

More specifically, this is achieved through the following
C++ toolchain `feature()` definitions (from `@rules_cc` 0.1.1 sources):

- The `default_compile_flags` features adds `-g` to compile actions
  for `--compilation_mode=dbg`, and adds `-g0` instead for
  `--compilation_mode=opt`.

- The `strip_debug_symbols` feature adds flags to strip binaries at
  link time (e.g. `-Wl,-S`) if one of the following is true:

  - `--strip=always` is used.
  - `--strip=sometimes` (the default) is used, with
    `--compilation_mode=fastbuild`.

  Note that this condition is hard-coded into Bazel 7.5 sources
  and exposed through a link variable that the feature depends on.

- The `no_stripping` feature, which is disabled by default, will
  control the creation of a `cc_binary()` stripped output (as
  explained below).

The `cc_binary()` rule produces a default output, which will contain
debug symbols based on the current toolchain's features (e.g. by default
only `dbg` builds will generate a binary with debug symbols, all others
will be stripped at link time).

The rule also adds an action that takes the default output and processes
with the toolchain's `strip` tool (even if the default output does not have
debug symbols). The corresponding file is named with a `.stripped` suffix
and is *not* built by default (i.e. not listed in target[DefaultInfo]).
However, it is possible to use `target[DebugPackageInfo]` to access it,
using its `stripped_file` field to access its `File` value in rule
implementation functions or aspects.

The `DebugPackageInfo` value is produced by the `cc_binary()` rule
and also provides an `unstripped_file` field pointing to the `File`
value of the default output (which is not always unstripped!).

As a special case, enabling the `no_stripping` toolchain feature
prevents the call to the `strip` tool, replacing it with a simple
symlink (or copy on Windows).

The default output is also available as
`target[DefaultInfo].files_to_run.executable` at analysis time.

Shared libraries are normally produced by `cc_binary()` targets by
setting `linkshared=True` in the target definition.

However, the recent `cc_shared_library` rule is an alternative way to
produce them (implementing additional dependency checks). However, unlike
`cc_binary()`, it only produces a default output, and does not produce
a non-default stripped artifact at all, nor provide a `DebugPackageInfo`
value.

### Rust

In Rust, `-Cdebuginfo` controls debug symbol generation when compiling
rlibs, and `-Cstrip_level` controls whether they are stripped when
linking final binaries (executables, dylibs and cdylibs).

The `@rules_rust`'s `rust_toolchain()` rule uses two attributes whose values
must be dictionaries mapping Bazel compilation modes to `-Cdebuginfo` and
`-Cstrip_level` values, and whose defaults are:

```py
        debug_info = {
            "dbg": "2",          # full debug symbols
            "fastbuild": "0",    # no debug symbols
            "opt": "0",          # no debug symbols
        },
        strip_level = {
            "dbg": "none",         # no stripping
            "fastsbuild": "none",  # no stripping
            "opt": "debuginfo",    # remove all debuginfo
        },
```

In other words, debug symbols are only generated by default for `"dbg"`
builds, and are always removed (even if manually enabled on a per-target
basis) for `"opt"` builds.

The `rust_binary()` rule only generates a single binary output. There is
no optional `<target>.stripped` output nor `DebugPackageInfo` provider.

### Go

The `@rules_go` default `go_config()` definition will only generate debug symbols for
the `"dbg"` compilation mode, and controls stripping using the value of the
[`--strip` flag][strip_flag] in the same way as `@rules_cc` (i.e. only stripping for
`"fastbuild"` or when `--strip=always` is used).

See [the default go configuration][rules_go_config_defaults] for details.

Interestingly, debug symbol generation for test binaries is
[forcefully disabled when stripping is enabled][rules_go_disable_test_debug].


## Fuchsia build mitigations

The Fuchsia build requires all debug symbols to be available post-build,
and that stripped versions be used for distribution. This is needed for:

- Local debugging sessions while running stripped binaries on an attached
  Fuchsia device or emulator.

- Symbolizing stack traces, either during local development, when
  running tests on infra, or for diagnostics on the go/crash dashboard.

In particular, infra build must collect debug binaries post-build and
upload them to cloud storage.

These goals are achieved by doing the following:

- The in-tree `.bazelrc`  sets `--strip=never` to ensure stripping is never
  performed. This is enforced by all language rulesets. This looks
  like:

  // LINT.IfChange
  ```
  common --strip=never
  ```
  // LINT.ThenChange(//build/bazel/templates/template.bazelrc:debug_symbols)

- For C++, the flags for the `default_compile_flags` feature enforce the
  generation of debug symbols for all compile actions *and* link actions.
  This looks like:

  // LINT.IfChange
  ```py
    _flag_configs = struct(
        ...
        debuginfo = _make_flag_config(
            cflags = [ "-g3", "-gdwarf-5", ... ],
            ldflags = [ "-g3", "-gdwarf-5", ... ],
            ...
        ),
    )

    return feature(
        name = "default_compile_flags",
        flag_sets = [
            flag_set(
                actions = _all_compile_actions,
                ...[
                    default_system_flags,
                    _flag_configs.debuginfo,
                    ...
                ]
            ),
            flag_set(
                actions = _all_link_actions,
                ...[
                    default_system_flags,
                    _flag_configs.debuginfo,
                    ...
                ]),
        ]
    )
  ```
  // LINT.ThenChange(//build/bazel_sdk/bazel_rules_fuchsia/common/toolchains/clang/cc_features.bzl)

  Note that adding `-g` to linker commands is used to add extra debug
  information for linker-generated code (e.g. trampolines or PLT stubs),
  something that is not performed by default Bazel C++ toolchains.

- For Rust, the `rust_toolchain()` definition provides custom `debug_info` and
  `strip_level` dictionaries to force debug symbol generation and prevent
  stripping too (the latter being redundant with `--strip=never`). This looks
  like:

  // LINT.IfChange
  ```py
    debug_info = {
        "dbg": "2",
        "fastbuild": "1",  # default "0", i.e. no symbols.
        "opt": "1",  # default "0", i.e. no symbols.
    },
    strip_level = {
        "dbg": "none",
        "fastbuild": "none",
        "opt": "none",
    },
  ```
  // LINT.ThenChange(//build/bazel/toolchains/rust/rust.BUILD.bazel)

- For Golang, the `.bazelrc` file sets `--@rules_go//go/config:debug=True`
  to ensure debug symbols are always generated. This looks like:

  // LINT.IfChange
  ```
  common --@io_bazel_rules_go//go/config:debug=True
  ```
  // LINT.ThenChange(//build/bazel/templates/template.bazelrc:debug_symbols)

## Fuchsia development considerations

The `fuchsia_package()` macro always ensures that binary dependencies are
stripped before being packaged into Fuchsia package. This is done by an
explicit action that invokes the C++ toolchain's `strip` tool, independent
from how the binary was produced. However this only works for targets
defined using the Bazel SDK rules.

For all other binary targets that need to be packaged or distributed, such
as host tools or Fuchsia binaries not built with the SDK, a dedicated Bazel
rule will be needed to perform similar stripping. Tracked by
https://fxbug.dev/443982549.

[strip_flag]: https://bazel.build/docs/user-manual#strip
[fuchsia_cc_toolchain_debuginfo]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/bazel_sdk/bazel_rules_fuchsia/common/toolchains/clang/cc_features.bzl;drc=45259f8d1473df80ecd8c180e97382886f19baa4;l=203

[rules_go_config_defaults]: https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/bazel_vendor/rules_go+/BUILD.bazel;drc=e4dbec1ba84bb7513c6f0b369172c85927f9b250;l=109
[rules_go_disable_test_debug]: https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/bazel_vendor/rules_go+/go/private/rules/test.bzl;drc=e4dbec1ba84bb7513c6f0b369172c85927f9b250;l=153

