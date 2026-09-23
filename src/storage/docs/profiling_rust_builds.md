# Profiling Rust Builds

This is a guide for how to profile the compilation of a Rust library or binary.

## Prerequisites

Install the following packages from `cargo`:

```shell
cargo install --git https://github.com/rust-lang/measureme crox flamegraph summarize
```

See [Using cargo](https://fuchsia.dev/fuchsia-src/development/languages/rust/cargo) for instructions
on setting up cargo if needed.

## Generating profile data

1. Change the BUILD.gn for your rustc_binary or rustc_library to include
   `configs += ["//build/config/rust:self-profile"]`.

2. `fx build <target>` or `fx clippy <target>`.  The former will also include LLVM linking in the
   overall report, the latter will generate a report only for a check-build.

3. Locate the `*.mm_profdata` file which was created in your output directory (e.g.
   `out/qemu/fxfs-0816704.mm_profdata`).

## Analyzing profile data

### Summarize the most expensive build steps

```shell
summarize summarize out/qemu/fxfs-0816704.mm_profdata | rg '\|' | awk '{print $4,$2}' | head
```

### Generate a flamegraph of the build

```shell
flamegraph out/qemu/fxfs-0816704.mm_profdata
```

A `rustc.svg` file will be created in the current working directory which can be viewed in e.g.
Chrome.

### Generating a Perfetto trace

```shell
crox out/qemu/fxfs-0816704.mm_profdata
```

A `chrome_profile.json` file will be created in the current working directory which can be viewed in
[ui.perfetto.dev].
