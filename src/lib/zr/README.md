# zr (Zircon Rust Core)

This library contains foundational, zero-dependency Rust primitives for the
Zircon kernel.

## Purpose

The goal of this crate is to provide the most basic abstractions needed by
other Rust code in the kernel, without assuming any bindings to C++ kernel
types.

## Dependencies

This crate must not depend on any other crates in the Zircon kernel tree.
