# `lib/fastboot` Rust FFI

Fastboot uses some functionality implemented by Rust libraries, so we need some
FFI support.

## Generating the FFI code

Since we're calling Rust APIs from a C++ program we'll use `cbindgen` which
takes a bit more manual effort than the other way around (`bindgen`). This
currently does not tie into the build system, so the headers need to be
re-generated manually whenever the API changes.

### Prereqs

1. Install the Rust toolchain (https://www.rust-lang.org/tools/install).

2. Install `cbindgen`:

   ```shell
   cargo install cbindgen
   ```

### Generation

1. Adjust `fx set` to include cargo generation. See
   [fuchsia.dev cargo docs](https://fuchsia.dev/fuchsia-src/development/languages/rust/cargo)
   for the latest instructions; currently this is done by:

   ```shell
   fx set <product>.<board> --cargo-toml-gen <other fx args>
   ```

   Alternatively, you can use `fx args` and manually add the necessary target to
   the `host_labels` arg:

   ```
   host_labels = [
     "//build/rust:cargo_toml_gen",
   ]
   ```

2. Configure the Rust toolchain for `per-package-target` support.

   At the time of this writing, cbindgen requires `cargo` support for the
   `per-package-target` feature, which is not yet available on the stable
   releases, so we need to switch to the nightly release track:

   ```
   rustup default nightly
   ```

   Note: the nightly build may not always be compatible with different versions
   of `cbindgen`, so you may need to set the default to a specific version
   instead. These instructions were originally tested using:

   * `rustc` 1.73.0-nightly
   * `cbindgen` 0.24.5

3. Run the generation script in this directory (this requires a host Python
   installation):

   ```shell
   $ python3 cbindgen.py
   ```

4. Switch back the stable Rust toolchain (optional)

   If you want to return your host to the stable Rust toolchain, run:

   ```shell
   rustup default stable
   ```

   You can also just leave it on the nightly release
