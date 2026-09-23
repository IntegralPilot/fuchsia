# cpp-log-decoder

Static FFI library that provides an entry point to the Rust log decoder.

## How to update FFI bindings

The [cbindgen program](https://github.com/mozilla/cbindgen) reads a
locally generated `Cargo.toml` manifest to create the header
for the `cpp-log-decoder` static Rust library, and its dependencies, to
expose a public C API.

There are four steps to create/update the bindings header:

1. Link the Fuchsia (or nightly) Rust toolchain.
2. Install cbindgen.
3. Generate the local `Cargo.toml` manifest.
4. Create/update the bindings headers.
5. Format the header.

## Link the Fuchsia Rust (or nightly) toolchain

The stable Rust toolchain cannot process Fuchsia-generated
`Cargo.toml` manifests. You therefore must link the Fuchsia toolchain in
your environment:

```
rustup toolchain link fuchsia "~/fuchsia/prebuilt/third_party/rust/linux-x64"
```

If you cannot link this toolchain in your environment, you may
alternately use the nightly Rust toolchain instead.

## Install cbindgen

We currently pin cbindgen at version 0.29.0 to avoid unexpected
behavior with cbindgen updates. You can install it with the following:

```
cargo install --version 0.29.0 --force cbindgen
```

## Generate the local `Cargo.toml` manifest for `cpp-log-decoder`

Generate a Cargo.toml by following [the instructions on
fuchsia.dev](https://fuchsia.dev/fuchsia-src/development/languages/rust/cargo)
for the build target
`//src/lib/diagnostics/log/message/rust/cpp-log-decoder:staticlib`.

As of this writing, this is done in the following way.

1. Using `fx args`, add `//build/rust:cargo_toml_gen` to `host_labels`.

2. Using `fx args`, add `//src/lib/diagnostics/log/message/rust/cpp-log-decoder:staticlib`
   to `build_only_labels`.

3. Build the `cargo_toml_gen` target and generate the `Cargo.toml`
   manifest:

```
fx args # then append "//build/rust:cargo_toml_gen" to `host_labels`
fx args # then append "//src/diagnostics/lib/cpp-log-decoder:staticlib"
        # to `build_only_labels`

fx build //build/rust:cargo_toml_gen
fx gen-cargo //src/lib/diagnostics/log/message/rust/cpp-log-decoder:_log_decoder_c_bindings_rustc_static.actual
fx gen-cargo //src/lib/diagnostics/log/message/rust:lib.actual
```

## Create/update the bindings header

The following command uses the Fuchsia toolchain and cbindgen to
create/update the bindings header. If you could not link the
Fuchsia toolchain, you may substitute `RUSTUP_TOOLCHAIN=nightly`.

```
RUSTUP_TOOLCHAIN=fuchsia cbindgen ~/fuchsia/src/lib/diagnostics/log/message/rust/cpp-log-decoder -o ~/fuchsia/src/lib/diagnostics/log/message/rust/cpp-log-decoder/log_decoder.h
```

