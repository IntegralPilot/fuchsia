# wlansoftmac-c

Static FFI library that provides an entry point to the Rust wlan-mlme
library for the C++ wlansoftmac driver.

## How to update FFI bindings

The [cbindgen program](https://github.com/mozilla/cbindgen) reads a
locally generated `Cargo.toml` manifest to create the `bindings.h` header
for the `wlansoftmac-c` static Rust library, and its dependencies, to
expose a public C API.

There are four steps to create/update the `bindings.h` header:

1. Link the Fuchsia (or nightly) Rust toolchain.
1. Install cbindgen 0.26.0.
1. Generate the local `Cargo.toml` manifest for `wlansoftmac-c`.
1. Create/update the `bindings.h` header.
1. Format the `bindings.h` header.

## Link the Fuchsia Rust (or nightly) toolchain

The stable Rust toolchain cannot process Fuchsia-generated
`Cargo.toml` manifests. You therefore must link the Fuchsia toolchain in
your environment:

```
rustup toolchain link fuchsia "$FUCHSIA_DIR/prebuilt/third_party/rust/linux-x64"
```

If you cannot link this toolchain in your environment, you may
alternately use the nightly Rust toolchain instead.

## Install cbindgen 0.26.0

We currently pin cbindgen at version 0.26.0 to avoid unexpected
behavior with cbindgen updates. You can install it with the following:

```
cargo install --version 0.26.0 --force cbindgen
```

## Generate the local `Cargo.toml` manifest for `wlansoftmac-c`

Generate a Cargo.toml for `wlansoftmac-c` by following [the instructions on
fuchsia.dev](https://fuchsia.dev/fuchsia-src/development/languages/rust/cargo)
for the build target
`//src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding:wlansoftmac-c`.

As of this writing, this is done in the following way.

1. Using `fx args`, add `//build/rust:cargo_toml_gen` to `host_labels`.

2. Using `fx args`, add `//src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding:_wlansoftmac_c_rustc_static`
   to `build_only_labels`.

3. Build the `cargo_toml_gen` target and generate the `Cargo.toml`
   manifest:

```
fx args # then append "//build/rust:cargo_toml_gen" to `host_labels`
fx args # then append "//src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding:_wlansoftmac_c_rustc_static"
        # to `build_only_labels`

fx build //build/rust:cargo_toml_gen
fx gen-cargo //src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding:_wlansoftmac_c_rustc_static
```

## Create/update the `bindings.h` header

The following command uses the Fuchsia toolchain and cbindgen to
create/update the `bindings.h` header. If you could not link the
Fuchsia toolchain, you may substitute `RUSTUP_TOOLCHAIN=nightly`.

```
RUSTUP_TOOLCHAIN=fuchsia cbindgen $FUCHSIA_DIR/src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/ -o $FUCHSIA_DIR/src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/bindings.h
```

**NOTE:** The wlan-mlme library and wlansoftmac driver each use
bindings for their respective language. This means cbindgen
cannot properly identify whether the types in those libraries exist
or not. Therefore, it is is normal to see `WARN` logs like the
following for some types:

```
WARN: Can't find zx_status_t. This usually means that this type was incompatible or not found.
```

## Format `bindings.h`

cbindgen uses a `/* */` style comment for the header guard which is
incompatible with `fx format-code`. Change this final comment line in
`bindings.h` to

```
#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_RUST_DRIVER_C_BINDING_BINDINGS_H_
```

and then run `fx format-code`

```
fx format-code --files=$FUCHSIA_DIR/src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/bindings.h
```

## Troubleshooting

### cbindgen fails when running `cargo metadata`

Remove the local `Cargo.toml` manifest and `Cargo.lock` and re-run `fx gen-cargo`.

### ERROR: Parsing crate `wlansoftmac_c`: can't find dependency version for `xyz`

Rebuild Fuchsia with `fx build` and re-run `fx gen-cargo //src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding:_wlansoftmac_c_rustc_static`.

### cbindgen did not generate a binding as expected

Try running `cbindgen` with `-v` argument to get a better understand
what types and functions were found and why they may have been rejected.

### Alternate method How to generate a manifest and pass it to cbindgen

1. Generate the manifest with cargo:

```
cargo +nightly metadata --all-features --format-version 1 --manifest-path Cargo.toml > metadata.json
```

2. Generate the C bindings with cbindgen using the generated metadata:

```
RUSTUP_TOOLCHAIN=fuchsia cbindgen $FUCHSIA_DIR/src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/ -o $FUCHSIA_DIR/src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/bindings.h --metadata metadata.json
```
