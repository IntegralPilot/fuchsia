# fx test

This directory contains the source code for `fx test`.

See [user guide](https://fuchsia.dev/fuchsia-src/reference/testing/fx-test)
for usage instructions.

## Development Instructions

This tool is automatically included in your build. The rest of this
section provides a guide for accelerating development cycles on the
tool.

### Build just the tool

To build only the new implementation, run:

```
fx build host-tools/test
```

This avoids a full build.

### Include and run tests

To test the new implementation's libraries, include
`--with //scripts/fxtest/python:tests` in your `fx set`.
For example:

```bash
fx set core.x64 --with //scripts/fxtest/python:tests

# This should work with the new implementation!
fx test --host
```