# Performance tests

This directory contains performance tests.

Check the [instructions][instructions] to run these test locally.

This directory contains some performance tests written in Python using
[Lacewing][lacewing]. It also contains all the expected metrics that performance
tests may generate. Finally, it includes a GN group that contains all benchmarks
in the tree.

This directory contains the following performance test:

*  `tracing_microbenchmarks_test` - Tests the performance of the tracing
    subsystem.

The following example:

*   `perf_publish_example` - Simple example test that publishes a performance
    metric.

And the following test:

*   `perftest_trace_events_test` - Tests that we correctly read events from a
    trace session.

You can view the test results from CI builds in the [Skia Perf][skiaperf]
performance dashboard.

<!-- Reference links -->

[skiaperf]: /docs/development/performance/skia_perf.md
[instructions]: /docs/development/performance/running_performance_tests.md
[lacewing]: /src/testing/end_to_end/README.md
