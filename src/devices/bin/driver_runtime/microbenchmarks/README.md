# Fuchsia Microbenchmarks

This set of tests includes microbenchmarks for Driver Runtime IPC primitives.

## Writing Benchmarks

This uses Zircon's
[perftest](https://fuchsia.googlesource.com/fuchsia/+/HEAD/zircon/system/ulib/perftest/)
library.

## Running Benchmarks

`fx test driver_runtime_microbenchmarks.cm --output -- -p`
