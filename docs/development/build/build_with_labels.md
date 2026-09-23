# Build with GN labels

`fx build` can now build GN targets directly. To do so, simply use one or more
GN target labels, with a `//` prefix on the command-line as in:

```sh
# Build the `cstr` and `uuid` libraries for Fuchsia.
fx build //src/lib/cstr //src/lib/uuid

# Equivalent to the previous command.
fx build //src/lib/cstr:cstr //src/lib/uuid:uuid
```

An error is printed if the label is not part of the configured GN graph. Even
if it is defined in the corresponding `BUILD.gn` file, it still needs to be a
transitive dependency from `//:default` or the other dependency lists defined
in your `args.gn`, such as `universe_package_labels` and others:

```none
# Build the corresponding tests for Fuchsia too.
$ fx build //src/lib/cstr:tests
ERROR: Unknown GN label (not in the configured graph): //src/lib/cstr:tests
```

For more details about why this happens, see [gn-root-targets].

## GN Toolchain suffixes

GN toolchain suffixes are supported (but the parentheses require shell quoting)
as in:

```sh
# Build the host tests for the `cstr` library.
fx build '//src/lib/cstr:tests(//build/toolchain:host_x64)'
```

Or even:

```sh
# Same as above
fx build //src/lib/cstr:tests\(//build/toolchain:host_x64\)
```

## GN Toolchain aliases

As a convenience, specific options can be used to append a GN toolchain suffix
for the _next_ labels that appear on the command-line, as in:

```sh
# Build the `cstr` and `fidl` tests for the host, and the `trace` library for Fuchsia.
fx build --host //src/lib/cstr:tests //src/lib/fidl:tests --fuchsia //src/lib/trace
```

A small fixed number of options are supported to alias toolchain definitions:

- `--host` matches the host toolchain label for your build machine
  (e.g. `//build/toolchain:host_x64` or `//build/toolchain:host_arm64`).

- `--default` matches the default toolchain for your current build
  configuration (e.g. `//build/toolchain/fuchsia:riscv64`).

- `--fuchsia` is another name for `--default` since Fuchsia binaries are built
  in the default GN toolchain (for now).

- `--fidl` matches `//build/fidl:fidling`, the GN toolchain used for processing
  FIDL files.

Other aliases may be added over time for convenience.

## GN Toolchain option

The `--toolchain=LABEL` option can be used to specific a GN toolchain label:

```sh
# Build the `cstr` tests for Linux/arm64 through cross-compiling
fx build --toolchain=//build/toolchain:linux_arm64 //src/lib/cstr:tests
```

It also accepts toolchain aliases, as in:

```sh
# Same as using `--host` directly as well.
fx build --toolchain=host //src/lib/cstr:tests
```

## GN labels to Ninja target paths

The `fx build` command will always print the list of Ninja targets it wants to
build, as in:

```none {:.devsite-disable-click-to-copy}
$ fx build //src/lib/cstr
Building Ninja target(s): obj/src/lib/cstr/cstr.stamp
...
```

Note that this lists includes all outputs from the corresponding build commands,
including ones generated implicitly by GN. Building any of them will trigger
the command that generates all of the outputs at once anyway.

```none {:.devsite-disable-click-to-copy}
$ fx build //src/lib/json_parser
Building Ninja target(s): obj/src/lib/json_parser/json_parser.inputdeps.stamp obj/src/lib/json_parser/json_parser.json_parser.cc.o obj/src/lib/json_parser/json_parser.rapidjson_validation.cc.o obj/src/lib/json_parser/json_parser.stamp
...
```

## Ninja targets to GN labels

Passing a Ninja target path is still supported, but will print a warning,
giving you the best build arguments to use instead, for example:

```none {:.devsite-disable-click-to-copy}
$ fx build host_x64/zbi host_x64/ffx
WARNING: Use '--host //zircon/tools/zbi' instead of Ninja path 'host_x64/zbi'
WARNING: Use '--host //src/developer/ffx/frontends/ffx:ffx_bin' instead of Ninja path 'host_x64/ffx'
Building Ninja target(s): host_x64/exe.unstripped/zbi host_x64/ffx host_x64/obj/src/developer/ffx/frontends/ffx/ffx_bin.stamp host_x64/obj/zircon/tools/zbi/zbi.zbi.cc.o host_x64/zbi host_x64/zbi.build-id.stamp
...
```

If you do not want to see this label, pass the Ninja path after the `--` option
separator, as anything following it is passed directly to Ninja:

```sh
fx build -- host_x64/zbi host_x64/ffx
```

However, as our Bazel migration carries on, this will not work for any
Bazel-generated outputs in the future.

[gn-root-targets]: /docs/development/build/gn_concepts/root_targets.md
