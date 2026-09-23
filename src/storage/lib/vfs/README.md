# Fuchsia VFS

Fuchsia VFS implements bindings and protocol for serving filesystems on the
Fuchsia platform. It implements both the remoteio protocol and an abstraction
around VFS serving.

## Rust VFS

Rust VFS and rust pseudo directories have rustdoc style documentation which can
be generated with command like

```
fx gen-cargo //src/storage/lib/vfs/rust:vfs
cargo +nightly doc --manifest-path src/storage/lib/vfs/rust/Cargo.toml --target x86_64-unknown-fuchsia --no-deps
```

The generated docs can be found in
`src/storage/lib/vfs/rust/target/x86_64-unknown-fuchsia/doc`.

See also: `cargo doc --help`.
