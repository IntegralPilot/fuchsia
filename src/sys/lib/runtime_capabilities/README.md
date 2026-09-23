# sandbox

`sandbox` is a Rust library for defining capabilities and transferring them
between programs.

## Building

To add this component to your build, append
`--with src/sys/component_manager/lib/sandbox:tests`
to the `fx set` invocation.

## Testing

Tests for `sandbox` are available in the `sandbox-tests` and `sandbox-unittests`
packages.

```
$ fx test sandbox-tests
$ fx test sandbox-unittests
```

