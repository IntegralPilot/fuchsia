# Rust ZBI lib

Rust version of [C library](/src/firmware/lib/zbi) to work with ZBI format.

See Rust documentation for `zbi` for library details.

# Notes

Currently [`sdk/lib/zbi-format`](/sdk/lib/zbi-format) is not ready to provide Rust bindings from FIDL.
So we are using `buildgen` generated version from C headers: [`src/firmware/lib/zbi-rs/src/zbi_format.rs`](/src/firmware/lib/zbi-rs/src/zbi_format.rs)

Another alternative is manually created version: [`src/sys/lib/fuchsia-zbi/abi`](/src/sys/lib/fuchsia-zbi/abi).
