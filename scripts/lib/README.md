# script libs

This directory contains libraries for scripting in the fuchsia.git
source tree and build.

## Testing

To run all tests, include //scripts/lib:tests in your build and run `fx test`:

```bash
$ fx set core.x64 --with-test //scripts/lib:tests
$ fx test --host
```