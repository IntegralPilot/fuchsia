## Benchmarks for the mapped clock library


### Prerequisites

* Ensure `fx serve` is running for your device.
* Ensure `fx set` is configured `--with //src/lib/mapped-clock/benchmarks:pkg`, or
  if you already have a `fx set` setup, then append to `args.gn`:

  ```python
  local_bench = true
  developer_test_labels += ["//src/lib/mapped-clock/benchmarks:pkg"]
  ```

### Running

```
fx ffx test run fuchsia-pkg://fuchsia.com/mapped_clock_benchmarks#meta/mapped_clock_benchmarks.cm
```

See the output in the `ffx test` results on your terminal.
