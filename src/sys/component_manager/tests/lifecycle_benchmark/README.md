# Component lifecycle performance benchmark

This test measures the performance of component framework lifecycle operations
such as starting components. A real instance of elf runner and package resolver
is used, with a nested instance of `component_manager`.

The metrics for this test are configured at
`//src/tests/end_to_end/perf/expected_metric_names/fuchsia.component.lifecycle.txt`.
If you add, delete, or rename a test you will need to update this file.
