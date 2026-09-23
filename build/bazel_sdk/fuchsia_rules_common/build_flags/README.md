# `build_flags()` Bazel rules in the Fuchsia build

## Overview

The definitions in this directory implement the `build_flags()` Bazel rule,
used to implement the equivalent of GN `config()` items, i.e. targets that
carry groups of compiler / linker flags, that can be applied to their
direct dependents, but also used to define the default toolchain flags
used by C++ and Rust actions.

Build flags are used to solve different problems in a consistent way:

- A way to define *default* C++ and Rust toolchain flags that is
  consistent with the GN `config()`-based definitions used in
  `//build/config/BUILDCONFIG.gn`.

- A way for Bazel targets to specify compiler and linker overrides
  in a way similar to GN `config()`s. This makes porting existing Fuchsia
  GN targets to Bazel considerably easier.

- A way for C++ Bazel targets to specify exclusion of certain flags,
  even if they are part of the toolchain defaults. For example
  if one wants to force a target to always be compiled in "debug"
  mode, even if the default is "release".

  This feature only works for C++ due to limitations in `@rules_rust`.

There are several subtle differences between Bazel `build_flags()`
and GN `config()`s that are documented below.

## C++ Examples

### Target overrides

Here's an example where two `build_flags()` targets are defined. They
store groups of flags through Bazel providers, and do not build
anything:

```py
# From //src/build_flags/BUILD.bazel
build_flags(
    name = "optimize_size",
    cflags = [ "-Os" ],
)

build_flags(
   name = "thinlto",
   cflags = [ "-flto=thin" ],
   ldflags = [ "-flto=thin" ],
   rustflags = [ "-Clto=thin" ],
)
```

They can be used to define compiler and linker overrides for individual
target definition, for example:

```py
# From //src/foo/BUILD.bazel
load("//build/bazel/rules/cc/fx_cc_binary.bzl", "fx_cc_binary")

fx_cc_binary(
    name = "bin1",
    srcs = [ "main.cc" ],
    build_flags = [
      "//src/build_flags:optimize_size",
      "//src/build_flags:thinlto",
    ]
)
```

Where `fx_cc_binary()` is a special wrapper macro that generates a
`cc_binary()` with the right final flags, based on the labels to
`build_flags()` targets that appear as the `build_flags` value.

This is conceptually equivalent to a definition like:

```py
cc_binary(
    name = "bin1",
    srcs = [ "main.cc" ],
    copts = [ "-Os", "-flto=thin" ],
    linkopts = [ "-flto=thin" ],
)
```

However, the implementation is very different (see "Implementation Details"
section below for details).

IMPORTANT: The `build_flags` overrides only affect the current target,
not its dependencies. They also do not affect dependents (with one exception
for `ldflags`, see the "Configs vs Build Flags" section below).

This matches the use of non-public `config()` labels in a GN target
definition which would look like:

```py
config("optimize_size") {
  cflags = [ "-Os" ]
}

config("thinlto") {
  cflags = [ "-flto=thin" ]
  ldflags = cflags   # see note below
  rustflags = [ "-Clto=thin" ]
}

executable("bin1") {
  sources = [ "main.cc" ]
  configs += [
    ":optimize_size",
    ":thinlto",
  ]
}
```

Notice that `ldflags = cflags` is a valid expression in GN,
but not in Bazel, where each attribute definition is really
a function call 'parameter=argument` expression.

### Excluding default toolchain flags

In GN, each binary target type has its own list of GN `config()` that are
applied by default to all target definitions of that type. These are set
by calling `set_defaults()` in `//build/config/BUILDCONFIG.gn`.

This list appears as `configs` in GN target definitions, and can be changed
directly within it. For example here's how to ensure that a given target
gets always compiled in debug mode, independent of the `args.gn` values.

```py
# For this example, assume //build/config:{release,balanced,debug} is used by
# default based on the content of args.gn

executable(
    name = "bin2",
    srcs = [ "main2.c" ]
    configs += [ "//build/config:debug" ]
    configs += [ "//build/config:release", "//build/config:balanced" ]
    configs -= [ "//build/config:release", "//build/config:balanced" ]
)
```

This can look confusing but can be explained:

- At the start of the target definition, `configs` is pre-populated with
  the list of `config()`s that apply to all `executable()` targets in the
  current GN toolchain. In a typical build, this would include one of
  `//build/config:{release,balanced,debug}`, based on the content of
  `args.gn`.

- The `configs += [ "//build/config:debug" ]` appends the `debug` config
  label unconditionally. Since labels in `configs` are de-duplicated
  automatically by GN when resolving the graph, this does nothing
  if `configs` already includes this label.

- The next two lines are used to remove the `release` and `balanced`
  labels from `configs` conditionally. One line to add both the `release`
  and `balanced` flags, and another line to remove *all instances* of
  these flags from `configs`. The first is needed to avoid a GN error
  if `configs` doesn't include one of the values.

The end result is that no `release` or `balanced` label will persist in
`configs`, and one or two instances of `debug` might be in it, which will
be deduplicated automatically anyway.

The corresponding example in Bazel is much simpler and uses the
`disable_build_flags` attribute as in:

```py
fx_cc_binary(
    name = "bin2",
    srcs = [ "main2.cc" ],

    # Always compile this binary in debug mode, even
    # if the current args.gn selects "release" or "balanced"
    build_flags = [ "//build/config:debug" ],
    disable_build_flags = [
        "//build/config:release",
        "//build/config:balanced",
    ],
)
```

With Bazel `build_flags()`, the default list is not available to target
definitions, so instead, `disable_build_flags` can be used to pass a list
of labels that MUST be omitted from the final build flags.

Note that `disable_build_flags` is applied after `build_flags`, so any label that
appears in both lists will *not* be in the final result.

## Rust examples

The `rustc_xxx()` rules from `//build/bazel/rules/rust:...` have been
updated to support `build_flags` labels similar to the C++ `fx_cc_xxx()`.
E.g.:

```py
build_flags(
    name = "extra_rust_flags",
    rustflags = [ .... ],
)

rustc_binary(
    name = "rust_bin1",
    ...
    build_flags = [ ":extra_rust_flags" ],
)
```

There are however a few IMPORTANT DIFFERENCES:

- First, there is no support for `disable_build_flags`, because
  of the way `@rules_rust` is implemented, it simply doesn't allow disabling
  the default Rust toolchain flags for a given target. So only
  `build_flags` is supported.

  To disable some features, it might be necessary to define "negative"
  build flags that undo the effect of the default toolchain flags, but this
  is not simple, and some Rust compiler flags have *no* way to be disabled
  once set on the command line (e.g. `--cap-lints`).

- Second, the `build_flags` values are not configurable, unlike their C++
  counterpart. This means you cannot use a select() statement as in:

  ```py
  # A set of Rust flags that should only be applied to Rust targets when
  # building for Fuchsia.
  build_flags(
      name = "extra_fuchsia_rust_flags",
      rustflags = [ ... ]
  )

  rustc_binary(
      name = "rust_bin1",
      ...
      # This will cause an error.
      build_flags = select({
        "@platforms//os:fuchsia": [ ":extra_fuchsia_rust_flags" ],
        "//conditions:default": [],
      })
  )
  ```

  A simple solution is to move the `select()` statement to the `build_flags()`
  definition, as in:

  ```py
  build_flags(
      name = "extra_fuchsia_rust_flags",
      rustflags = select({
        "@platforms//os:fuchsia": [ ... ],
        "//conditions:default": [],
      })
  )

  rustc_binary(
      name = "rust_bin1",
      ...
      build_flags = [ ":extra_fuchsia_rust_flags" ],
  )
  ```

  This limitation may be lifted once https://fxbug.dev/516778625 is fixed
  though.

# Configs vs Build Flags

This section documents the subtle differences between GN `config()`s and
Bazel `build_flags()` definitions, as they are important when porting existing
GN target definitions to the Bazel graph.

These differences come from implementation limitations, as it is impossible
to support all GN features in Bazel.

## No `public_configs` and `all_dependent_configs` support.

In GN, a target can set its `public_configs` argument to a list of labels that
will apply to its *direct dependents*. Similarly, `all_dependent_configs` will
apply a list of config labels to *all transitive dependents* of the current
target.

These are not supported with `build_flag()` due to vast differences on how
Bazel and GN operate. Fortunately, most uses of `public_configs` in the Fuchsia
GN graph are limited to a few special cases:

### Header include search path

Adding an `include_dirs` directory to all dependents for C++ library target.
This looks like:

```py
# //src/foo/BUILD.gn
config("foo.config") {
  include_dirs = [ "include" ]   # For //src/foo/include
}

source_set("foo") {
  ...
  public_configs = [ ":foo.config" ]
}
```

And is used to ensure that `//src/foo/include` will be in the include search
path of all dependent libraries that want to access `foo`'s headers.

In Bazel, this comes *for free*, as the `includes` in the C++ library's
definition is automatically propagated to dependents, as in:

```py
cc_library(
    name = "foo",
    includes = [ "include" ],
    ...
)
```

To add an include path specific to the target itself, use
`copts = [ "-I<path>" ]` instead where `<path>` is relative to the Bazel
execroot (e.g. `-Isrc/foo/include`).

### Macro definitions for public headers

Adding a `defines` macro to all dependents of a C++ library target. This is
similar:

```py
config("bar.config") {
  defines = [ "ENABLE_BAR=1" ],
}

source_set("bar") {
  public_configs = [ ":bar.config" ]
  ...
}
```

In Bazel, the `defines` attribute of a `cc_library()` target is also
propagated automatically to dependents. Targets can use `local_defines`
instead to define macros that only affect the target itself, or use
`copts = [ "-DENABLE_BAR=1" ]`.

```py
cc_library(
  name = "bar",
  defines = [ "ENABLE_BAR=1" ],
)
```

### Adding compiler flags to disable warnings

This is seldom used for third-party libraries whose public headers trigger
many compiler warnings, and which come with a Fuchsia-maintained `BUILD.gn`.

```py
# From //third_party/tink-cc/BUILD.gn

# Used as a public_config() for most Tink libraries.
config("tink_config") {
  cflags = [
    "-Wno-ignored-qualifiers",

    # The tink library uses absl headers containing deprecated API usage.
    "-Wno-deprecated-declarations",

    # The tink library uses absl headers with implicit copy constructors.
    "-Wno-deprecated-copy",

    # The tink library does not restrict extra semicolon.
    "-Wno-extra-semi",
  ]
  ...
}
```

There is *no direct way* to support this in Bazel, the alternatives are:

- Fix the headers (requiring either upgrading the library, or forking it).

- Define a `build_flags()` target to hold the flags then use it in dependents,
  e.g.:

  ```py
  # From //third_party/tink-cc/BUILD.bazel
  load("@rules_fuchsia_common//build_flags:build_flags.bzl")

  # These build flags are required to use the public Tink library headers
  # to silence annoying compiler warnings.
  build_flags(
      name = "tink_public_header_flags",
      cflags = [
          "-Wno-ignored-qualifiers",

          # The tink library uses absl headers containing deprecated API usage.
          "-Wno-deprecated-declarations",

          # The tink library uses absl headers with implicit copy constructors.
          "-Wno-deprecated-copy",

          # The tink library does not restrict extra semicolon.
          "-Wno-extra-semi",
      ],
  )
  ```

  Then

  ```py
  # From //src/using/tink/BUILD.bazel
  load("//build/bazel/rules/cc:fx_cc_library.bzl", "fx_cc_library")

  fx_cc_library(
      name = "foo",
      ...
      build_flags = [ "//third_party/tink-cc:tink_public_header_flags" ],
      deps = [ "//third_party/tink-cc:tink" ],
  )
  ```

## No support for `asmflags`

Bazel doesn't allow its C++ rules to specify assembler-specific flags at all.
If you really need them, you will need to split your targets and use standard
`copts` (or `build_flags()` with `cflags`) to specify them.

## No support for `inputs` or `libs`

These `config()` arguments can be used to specify extra inputs to the
compiler or link actions. However, there is *no way* to implement a similar
functionality in Bazel, so they are simply not available as `build_flags()`
attributes.

Any GN `config()` definitions that rely on these, will require an
alternative mechanism. Which one to use will depend on the use case.

## No support for `cflags_objc`, `cflags_objcc` or `swiftflags`

The Fuchsia build does not use these GN features anyway.

## No support for `precompiled_header` and `precompiled_source`

The Fuchsia build does not use these GN features anyway.

## A note on `visibility` and `testonly`

Supported as expected in Bazel as they do in GN, i.e. they apply to the
`build_flags()` target definition itself, and their values are not applied
to their dependents.

# Implementation details

## Adding overrides to a `cc_binary()` target

Let's get back to the original example definition:

```py
# From //src/foo/BUILD.bazel
load("//build/bazel/rules/cc/fx_cc_binary.bzl", "fx_cc_binary")

fx_cc_binary(
    name = "bin1",
    srcs = [ "main.cc" ],
    build_flags = [
      "//src/build_flags:optimize_size",
      "//src/build_flags:thinlto",
    ]
)
```

Due to the fact that `fx_cc_binary()` must be a macro, and cannot
access the values of `build_flags()` dependencies when it is
evaluated, it instead generates several targets, that include
response files for the compiler and linker actions, and a final
definition that looks like this:

```py
... some details omitted for now.

_generate_final_build_flags(
    name = "bin1.final_build_flags",
    build_flags = [
      "//src/build_flags:optimize_size",
      "//src/build_flags:thinlto",
    ],
    target_type = "executable",
    ...
)

_cc_response_file_rule(
    name = "bin1.cc_compile.build_flags",
    action_kind = "cc_binary",
    final_build_flags = [ ":bin1.final_build_flags" ],
    ...
)

_cc_response_file_rule(
    name = "bin1.cc_link.build_flags",
    action_kind = "cc_link",
    final_build_flags = [ ":bin1.final_build_flags" ],
    ...
)

cc_binary(
    name = "bin1",
    srcs = [ "main.cc" ],
    copts = [ "@$(location :bin1.cc_compile.build_flags)" ],
    linkopts = [ "@$(location :bin1.cc_link.build_flags)" ],
    deps = [ ":bin1.cc_compile.build_flags" ],
    additional_linker_inputs = [ ":bin1.cc_link.build_flags" ],
    features = [ "-fuchsia_default_build_flags" ],
)
```

Where:

- `bin1.final_build_flags` is a target that computes the final set
  of `build_flags()` labels that apply to a given target, and makes
  it available as a provider. This doesn't generate any artifact, and
  is where all the logic dealing with default build_flags() labels
  and de-duplication happens. Note the use of `target_type = "executable"`
  to match the generation of an executable binary.

- `bin1.cc_compile.build_flags` is a target that generates a
  response file for the compiler action, computed by processing the list
  of labels computed by `bin1.final_build_flags` to only keep the flags
  used by the C++ compiler action. In this case, it will contain
  *all the default compiler flags* (from the toolchain), one per line,
  followed by `-Os` and `-flto`.

- `bin1.cc_link.build_flags` is a target generating a linker
  response file, also from the `bin1.final_build_flags` value. Its content
  is *all default linker flags* followed by `-flto`.

- `copts` in `bin1` contains an `@<path>` command-line argument
  that tells the compiler to use the content of
  `bin1.cc_compile.build_flags` to read arguments, this is
  how the `build_flags()` compiler flags are injected into
  the action.

- `deps` ensures that the artifact is part of the action's
  sandbox when its command is run.

- `linkopts` in `bin1` contains another `@<path>` argument
  to use the output of `bin1.cc_link.build_flags` to read
  linker arguments.

- `additional_linker_inputs` ensures that the artifact is
  part of the action's sandbox when its command is run.
  Due to the way Bazel C++ rules work, adding it to `deps`
  cannot work.

- `features` tells the C++ toolchain to *not* inject the
  default flags in the command-line, as these are already
  included in the response files.

The end result is that the command-line for the compiler
will look like:

```shell
..../clang/bin/clang++ \
     .... \
     @bazel-bin/k8-fastbuild/bin/src/bin1.cxx_compile.build_flags
```

But *in the end* the compiler will see a set of flags corresponding
to the equivalent definition:

```py
cc_binary(
    name = "bin1",
    srcs = [ "main.cc" ],
    copts = [ "-Os", "-flto=thin" ],
    linkopts = [ "-flto=thin" ],
)
```

## Default `build_flags()` labels.

The list of default `build_flags()` labels to use for C++ artifacts is
determined using the following scheme:

- A Bazel toolchain type (`@rules_fuchsia_common//build_flags:toolchain_type`)
  is defined to expose lists of default flags for different target types
  (i.e. "common", "executable" and "shared_library" artifacts).

  Instances of this toolchain type provide a provider that provides
  three lists of build_flags() information. Such as:

  ```py
  DefaultBuildFlagsSetInfo = provider(
    fields = {
      "common_build_flags": "(list[BuildFlagsInfo]) ...",
      "executable_build_flags": "(list[BuildFlagsInfo]) ...",
      "shared_library_build_flags": "(list[BuildFlagsInfo]) ...",
    }
  )
  ```

- The `_generate_final_build_flags()` rule, implemented in `build_flags.bzl`,
  depends on it, as an *optional* `toolchains` value. If available, it
  computes the default list of build flags for the current `target_type`
  value, adds the `build_flags` values, remove duplicates, then removes those
  values from `disable_build_flags`. The final result is stored in a provider
  exposed by the target.

- The `_cc_response_file_rule()` rule, uses these final values and the
  current `action_kind` value to compute C++ compiler / linker flags.

- The actual toolchain instance definitions are generated by `fx gen`
  in `@fuchsia_build_info//default_build_flags/BUILD.bazel`, and they
  look like the following:

  ```py
  # From @fuchsia_build_info//default_build_flags:BUILD.bazel

  load(
      "@fuchsia_rules_common//build_flags:toolchain.bzl",
      "build_flags_toolchain_instance"
  )

  # Default build_flags() for Fuchsia toolchains.
  build_flags_toolchain_instance(
      name = "fuchsia_default_build_flags_instance",
      common_build_flags = [ ... ],
      executable_build_flags = [ ... ],
      shared_library_build_flags = [ ... ],
  )

  toolchain(
      name = "fuchsia_default_build_flags",
      toolchain = ":fuchsia_default_build_flags_instance",
      toolchain_type = "@fuchsia_rules_common:toolchain_type",
      target_compatible_with = [ "@platforms//os:fuchsia" ],
  )

  # Default build_flags() for host toolchains.
  build_flags_toolchain_instance(
      name = "host_default_build_flags_instance",
      common_build_flags = [ ... ],
      executable_build_flags = [ ... ],
      shared_library_build_flags = [ ... ],
  )

  toolchain(
      name = "host_default_build_flags",
      toolchain = ":host_default_build_flags_instance",
      toolchain_type = "@fuchsia_rules_common:toolchain_type",
      target_compatible_with = [ "@platforms//os:linux" ],
  )
  ```

- These toolchain instances are registered in `toplevel.MODULE.bazel`
  with:

  ```py
  register_toolchains("@fuchsia_build_info//default_build_flags:all")
  ```

The values for `{common,executable,shared_library}_build_flags` in
the toolchain instance definitions above are generated from the GN
build configuration. More specifically:

- The set of default `config()` labels for Fuchsia and host GN toolchains
  (a.k.a. Bazel platforms) are written by a new `//build/config/BUILD.gn`
  target at `gn gen` time, then consumed by `//build/regenerator.py` which
  generates the content of `@fuchsia_build_info`.

- For each default `config()` label, the script checks whether there is
  a corresponding Bazel `build_flags()` definition with the same label.
  If it exists, its Bazel label is added to the output of the corresponding
  `*_build_flags` list.

  If the corresponding Bazel target doesn't exist, the `config()` label
  is written to a separate list of `missing_configs` that appears in a
  comment at the end of `@fuchsia_build_info//default_build_flags/BUILD.bazel`.

This scheme has several benefits:

- It allows introducing `build_flags()` equivalents to GN `config()`s
  progressively, instead of requiring all definitions to be available
  immediately.

- It doesn't care about how the `build_flags()` target are written or
  maintained (in particular to keep them in sync with their GN counterparts).

Initially, the default `build_flags()` will likely be written manually,
with `LINT` checks to ensure consistency with the GN definitions, however
tooling will be introduced later to make this process much easier (exact
details not discussed in the current document).
