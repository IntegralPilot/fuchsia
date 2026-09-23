# Logging in Rust

This document explains how to get started with logging in Rust programs on
Fuchsia. For general information about recording and viewing logs, see the
[language-agnostic logging documentation][doc-logging].

## Required capabilities {#capabilities}

Ensure that your component requests the appropriate logging capabilities by
including the following in your component manifest:

```json5
{
  include: [
    "syslog/client.shard.cml"
  ],
  ...
}
```

## Initialization {#initialization}

You must initialize logging before you can [record logs](#record) from Rust code.
Initialization is handled by the [`fuchsia`][ref-fuchsia] crate setup macros.

### GN dependencies

Add the following `deps` to your `BUILD.gn` file:

```gn
deps = [
  "//src/lib/fuchsia",
]
```

### Setup

In your Rust source files, logging is enabled by default for any function
initialized using the `fuchsia::main` or `fuchsia::test` macros:

```rust
#[fuchsia::main]
fn main() {
    // ...
}

#[fuchsia::test]
fn example_test() {
    // ...
}
```

You can also pass the `logging` flag to make this explicit:

```rust
#[fuchsia::main(logging = true)]
fn main() {
    // ...
}

#[fuchsia::test(logging = true)]
fn example_test() {
    // ...
}
```

## Add tags

Log messages can include one or more tags to provide additional context.
To enable log tags for a given scope, pass the `logging_tags` parameter during
[initialization](#initialization):

```rust
#[fuchsia::main(logging_tags = ["foo", "bar"])]
fn main() {
    // ...
}

#[fuchsia::test(logging_tags = ["foo", "bar"])]
fn example_test_with_tags() {
    // ...
}
```

## Record logs {#record}

Rust programs on Fuchsia generally use the `log` crate macros to record
logs.

### GN dependencies

Add the `log` crate to the `deps` entry of your `BUILD.gn` file:

```gn
deps = [
  "//third_party/rust_crates:log",
]
```

### Log events

Call the macros provided by the [`log`][log-crate] crate to record logs at the declared
severity level:

```rust
fn main() {
    log::trace!("something happened: {}", 5); // maps to TRACE
    log::debug!("something happened: {}", 4); // maps to DEBUG
    log::info!("something happened: {}", 3);  // maps to INFO
    log::warn!("something happened: {}", 2);  // maps to WARN
    log::error!("something happened: {}", 1); // maps to ERROR

    # You can also use the log crate to emit structured logs.
    log::info!(my_key = 3, other_key_debug:?; "something happened");
}
```

Note: While Fuchsia provides its own `FX_` prefixed logging macros in
C++, Fuchsia has aligned on the [`log`][log-crate] crate as the logging
interface for Rust. See the [documentation][log-crate] for guidance on
how to format your logs.

## Standard streams

Rust macros such as `println!`, `eprintln!` etc. map to standard out (`stdout`)
and standard error (`stderr`). Using these streams may require additional setup
work for your program.

For more details, see the [standard streams][std-streams] section in the
language-agnostic logging documentation.

[doc-logging]: /docs/concepts/components/diagnostics/README.md#logs
[log-crate]: https://fuchsia-docs.firebaseapp.com/rust/log/index.html
[ref-fuchsia]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia/
[rust-dev]: /docs/development/languages/rust/README.md
[std-streams]: /docs/development/diagnostics/logs/recording.md#stdout-stderr
