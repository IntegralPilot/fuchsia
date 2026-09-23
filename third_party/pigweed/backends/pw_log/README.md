# Fuchsia pw_log Backend

This a backend for the Pigweed logging facet,
[pw_log](https://pigweed.dev/pw_log/).

Facets are resolved globally for a build, but different parts of the Fuchsia
build (within the same toolchain) have different logging requirements. For
example drivers and command-line tools. To address this the backend
`//third_party/pigweed/backends/pw_log` contains the interface and some common
code but depends on an implementation. These must be linked into the target
that uses Pigweed code that uses logging.

The current targets are:

`//third_party/pigweed/backends/pw_log:dfv1`
: uses the DFV1 logging APIs.

`//third_party/pigweed/backends/pw_log:printf`
: uses libc `printf` to send messages to stdout.

More targets can be added in the future by adding more `source_set`s that
provide the C symbol:
```c
void pw_log_fuchsia_impl(int level, const char* module_name, const char* file_name, int line_number,
                         const char* message)
```
