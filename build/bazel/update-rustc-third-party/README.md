`crates_vendor` target in the BUILD.bazel from this directory is used in `fx
update-rustc-third-party` to generate Bazel target for Rust libraries in
`//third_party/rust_crates`.

Always use `fx update-rustc-third-party` instead of building of this target
directly. The targets defined in BUILD.bazel rely on a temporary Bazel workspace
constructed in `update-rustc-third-party`, so building it directly won't work.

This setup uses the following vendored/prebuilt tools:

*   bazel
*   buildifier
*   Rust toolchain (rustc, cargo, etc.)
*   rules_rust

Bazel will fetch other dependencies from upstream, this includes Rust crates
needed to bootstrap crates_vendor (build the cargo_bazel binary).

This only supports running on Linux hosts.
