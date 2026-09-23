# Power Framework Benchmarks

Power Framework currently has microbenchmarks exercising the AcquireWakeLease call
from the System Activity Governor (SAG), and the Lease operation fidls from the
Topology Test Daemon. These benchmarks are based on the
[Criterion](https://docs.rs/criterion/latest/criterion/) benchmark
infrastructure. Benchmark functions and fidl proxy obtaining functions are
separately declared in each some_work.rs and can be run either as standard
integration tests or to profile the performance of the corresponding fidl.
The integration tests run in CQ and can verify the correctness of the
implementations of the same function for fidl proxy creation and benchmarks.

## Benchmark Metrics

These benchmarks measure the latency of wake lease operations under different
active lease states in the System Activity Governor (SAG). They help isolate the
overhead introduced by two main mechanisms:

1. **Power Broker state transitions:** Bypassed when *any* wake lease is already held.
2. **Long wake lease monitoring (timer tasks):** Bypassed when *any unmonitored*
wake lease is already held.

The following metrics are collected:

*   **`TakeDropWakeLease`**: Runs with no leases active. Measures the full cycle
of acquiring and dropping a monitored wake lease. This incurs both the Power Broker
transition overhead and the long-lease monitoring timer task creation.
*   **`TakeMonitoredWakeLease`**: Runs while a normal wake lease is held in the
background. Bypasses the Power Broker transition overhead but still incurs the
long-lease monitoring timer task creation.
*   **`TakeWakeLease`**: Runs while a background unmonitored lease is held.
Bypasses both the Power Broker transition overhead and the timer task creation
(clean fast path).
*   **`ToggleLease`**: Measures the lease toggling operation via the Topology
Test Daemon.
*   **`LargeTopologyLease`**: Measures lease acquisition and release in a large
topology with multiple elements.
*   **`LargeTopologyWithBackgroundLeases`**: Measures lease acquisition and
release in a large topology where background leases are held.
*   **`LargeSharedTopology`**: Measures lease acquisition and release in a
realistic large topology of 100 elements, where ~80% of elements (the shared
infrastructure) are held active by initial leases, and individual leases under test
span the topology dependencies but only modify the required power levels of 0-5 elements
(with 0 elements modifying when an active shared element is leased).

## Running the Benchmarks

1. Add the benchmark test target to your `fx set` line, and configure your
   build for release (optimized). Note that the benchmarks rely on `SL4F`
   which is currently only available on terminal and workstation builds:

    ```
    fx set terminal.x64 --with //src/tests/end_to_end/perf:test --release
      --with-test //src/power:tests #integration test will be included
    ```

2. Build Fuchsia

    ```
    fx build
    ```

3. Start the Fuchsia emulator

    ```
    ffx emu start --headless --net tap
    ```

4. In a separate terminal, serve Fuchsia packages

    ```
    fx serve -v
    ```

5. Run the tests

    ```
    fx test --e2e power_framework_microbenchmarks -o
    ```

6. Run integration tests (optional)

    ```
    fx test -o power-framework-bench-integration-tests --test-filter=*test_topologytestdaemon_toggle -- --repeat 2000
    ```

After completing, the tests will print the name of the
[catapult_json](https://github.com/catapult-project/catapult/blob/main/docs/histogram-set-json-format.md)
output file containing the benchmark results.

## Tracing the benchmarks

To take traces of the benchmarks, use the end-to-end wrapper, which runs the
benchmarks via a Lacewing wrapper. For example:

1. `fx set` and build

    ```
    fx set workbench_eng.x64 --release --with-test //src/power/bench/power_framework:tests
    fx build
    ```

2. Start an emulator and server using the same steps as in the previous section.

3. Run the test, collecting artifacts including the `.fxt` and `.json` trace
   files in a timestamped subdirectory of `test_artifacts`.

    ```
    mkdir -p test_artifacts
    fx test --e2e power_framework_benchmarks --outdir test_artifacts --timestamp-artifacts
    ```
