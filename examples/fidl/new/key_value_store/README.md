# README

This directory contains all of the example code for the "key-value store" series
of FIDL examples. See the associated [docs][docs] for more information.

Each example should include the following:

* The [FIDL][impl-fidl] definition.
* The [CML][impl-cml] definitions for the client and server.
* The [realm][impl-realm] definition for performing end-to-end testing.
* An [implementation][impl-cpp-natural] of the client and server using the C++ (Natural) bindings.
* An [implementation][impl-cpp-wire] of the client and server using the C++ (Wire) bindings.
* An [implementation][impl-rust] of the client and server using the Rust bindings.

[docs]: /docs/development/languages/fidl/examples/key_value_store/README.md
[impl-cml]: baseline/meta
[impl-cpp-natural]: baseline/cpp-natural
[impl-cpp-wire]: baseline/cpp-wire
[impl-fidl]: baseline/fidl
[impl-rust]: baseline/rust
[impl-realm]: baseline/realm
