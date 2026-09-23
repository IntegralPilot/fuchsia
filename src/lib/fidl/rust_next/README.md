# Next-generation FIDL bindings for Rust

This directory contains the library crates for new Rust bindings. These bindings are being actively
developed by the FID team, with dkoloski@ as the primary point of contact.

## Features

These bindings add support for wire types, allowing developers to decode and use FIDL messages
without copying data out of the message buffer. This avoids allocation costs associated with
decoding and improves overall performance.

These bindings are executor-agnostic and are generic over FIDL transports. This means that they can
be used with other executors like `tokio`, and their protocols can be run over bidirectional
transports other than Fuchsia channels.

These bindings also have first-class support for handle-less FIDL, and can enforce that FIDL data
does not contain handles at compile time. This allows users to persist FIDL data to disk, run
FIDL protocols over transports which do not support transferring handles (for example: sockets), and
use FIDL safely on host machines which do not support Fuchsia handles.

## Documentation

See the [generated crate documentation] for each library crate for explanations of the various types
and traits introduced and how they relate to each other.

Examples of these FIDL bindings can be found alongside the code generator in
[`//tools/fidl/fidlgen_rust_next/examples`][examples].

[generated crate documentation]: https://fuchsia-docs.firebaseapp.com/rust/fidl_next/index.html
[examples]: https://source.corp.google.com/h/fuchsia/fuchsia/+/main:tools/fidl/fidlgen_rust_next/examples/

## Crates

The library code for these bindings is split up into three primary crates which layer on top of each
other:

### `fidl_next_codec`

This crate is the base of the library code, and defines the core traits for encoding and decoding
FIDL data. It defines the wire types for FIDL data, and provides essential abstractions for writing
encoding and decoding logic safely.

### `fidl_next_protocol`

This crate builds on top of the codec layer to provide support for FIDL transports and untyped
protocols. It provides implementations for `async` clients and servers, as well as a FIDL transport
implementation for Fuchsia channels.

### `fidl_next_bind`

This crate builds on top of the protocol layer and adds types which add strong typing to generated
FIDL bindings. Many of the types from the untyped protocol layer have wrappers, such as `Client` and
`Server`. This crate also adds traits which formalize the concept of FIDL protocols and methods in
the type system.

## Code generation

The code generator for these bindings is located in [`//tools/fidl/fidlgen_rust_next`][examples].
