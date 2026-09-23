# processargs

`processargs` is a Rust library that makes it easier to work with the
[processargs] protocol.

## Building

To add this component to your build, append
`--with src/sys/bedrock/processargs:tests`
to the `fx set` invocation.

## Testing

Tests for `processargs` are available in the `processargs-unittests` package.

```
$ fx test processargs-unittests
```

