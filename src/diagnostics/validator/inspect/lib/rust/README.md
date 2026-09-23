# Rust Inspect Puppet

Reviewed on: 2023-05-24

This is the Rust Puppet for the Inspect Validator. The test
`inspect-validator-test-rust` checks that the Rust Inspect library conforms
to the specification.

## Building

To add this project to your build, append `--with //src/diagnostics/validator/inspect/lib/rust:tests`
to the `fx set` invocation.

For example:

```
fx set core.x64 --with --with //src/diagnostics/validator/inspect/lib/rust:tests
```

## Testing
To run the test:
```
fx test inspect-validator-test-rust
```
