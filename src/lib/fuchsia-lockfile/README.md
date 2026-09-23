# `fuchsia-lockfile`

`fuchsia-lockfile` is a host library that coordinates file access from multiple processes.

## Building

Since this is a host library, the host toolchain needs to be specified for the target.
For a `x64`, to add tests for this library to your build, append the following to the `fx set`:
`--with //src/lib/fuchsia-lockfile:host_tests\(//build/toolchain:host_x64\)`.

## Testing

To run the tests:

```
 fx test //src/lib/fuchsia-lockfile
```
