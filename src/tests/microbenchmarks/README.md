# Fuchsia Microbenchmarks

This set of tests includes microbenchmarks for core OS functionality,
including Zircon syscalls and IPC primitives, as well as microbenchmarks for
other layers of Fuchsia.

Some of the microbenchmarks are portable and can be run on Linux or Mac, for
comparison against Fuchsia.  This means that the name fuchsia_microbenchmarks
should be taken to mean "microbenchmarks that are part of the Fuchsia project"
rather than "microbenchmarks that only run on Fuchsia".

This used to be called "zircon_benchmarks", but it was renamed to reflect
that it covers more than just Zircon.

## Writing Benchmarks

This uses the [perftest C++ library][perftest].

## Running Benchmarks

fuchsia_microbenchmarks can be run the following ways:

*   **Iterate on Single Benchmarks:** If you want to run a smaller or faster set of tests, you can
    invoke the fuchsia_microbenchmarks component directly and pass some of the options accepted by
    the [perftest C++ library][perftest].

    For example, the following invocation runs only the `Syscall/Null`
    test case and prints some summary statistics:

    ```
    fx test fuchsia_microbenchmarks.cm --output -- -p --filter '^Syscall/Null$'
    ```

    Alternatively, for machine readable results, the following invocation will produce an output
    file in [`fuchsiaperf.json`][fuchsiaperf] format and will copy it back to the host in
    `<host-output-dir>`.

    ```
    fx test fuchsia_microbenchmarks.cm --output --ffx-output-directory <host-output-dir> -- -p --out /custom_artifacts/results.fuchsiaperf.json
    ```

    For the full list of options, run:
    ```
    fx test fuchsia_microbenchmarks.cm -- --help
    ```

*   **Maximal:** This approach uses the same end to end host test setup as Infra builds on CI and CQ.
    This runs the full suite multiple times, then outputs the results in fuchsiaperf.json files
    which can be post processed for the results.

    For this, you'll need some additional setup. Follow the instructions in [How to run performance
    tests][running-perf-tests] and use this command:

    ```
    fx test --e2e fuchsia_microbenchmarks_test
    ```

    This will take some time because it runs the fuchsia_microbenchmarks process multiple times.

    This entry point will copy the performance results ([`*.fuchsiaperf.json`][fuchsiaperf] files)
    back to the host in the `out/test_out` directory.

*   **Minimal:** Unit test mode. To check that the tests pass, without collecting any performance
    results, you can run the fuchsia_microbenchmarks component directly:

    ```
    fx test fuchsia_microbenchmarks.cm
    ```

    This will quickly run a small number of iterations of each benchmark to verify that they run
    successfully.

<!-- Links -->

[perftest]: /zircon/system/ulib/perftest/README.md
[running-perf-tests]: /docs/development/performance/running_performance_tests.md
[fuchsiaperf]: /docs/development/performance/fuchsiaperf_format.md
