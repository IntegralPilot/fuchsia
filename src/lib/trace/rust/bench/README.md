# Benchmarks for Rust Trace Events

The easiest way to run these is to enable local benchmarks in your build:

```
fx set minimal.x64 --release --with //src/lib/trace/rust/bench --args=local_bench=true
fx build updates
ffx test run 'fuchsia-pkg://fuchsia.com/rust_trace_events_benchmarks#meta/trace_events.cm'
```

Alternatively, you can test through through the end to end benchmarking framework which will also
benchmark with the trace points both enabled and disabled.

```
fx set terminal.x64 --release --with //src/tests/end_to_end/perf:test
fx test --e2e tracing_microbenchmarks_test
```

See Also: [Running Performance Tests](docs/development/performance/running_performance_tests.md)
