# Inspect Validator

Reviewed on: 2024-04-2024

## Overview

Since Go is deprecated on Fuchsia, it does not have a complete FIDL implementation
or Inspect implementation. In order to be able to use new FIDL features for validating Inspect in
C++ and Rust, the Go puppet is hidden behind a dispatcher component written in Rust.

## Dispatcher

The files associated with the Rust component that dispatches to the Go puppet:

```
./meta/puppet.cml
./src/*
```

This component serves `diagnostics.validate.InspectPuppet`, like the puppets written in Rust
and C++ for the Rust and C++ libraries. It intercepts anything the Go Inspect implementation does
not support and returns the `TestResult::Unimplemented` result.

## Go puppet

The actual Go puppet is everything under `./puppet-internal`.

This puppet serves `diagnostics.validate.deprecated.InspectPuppet`.

