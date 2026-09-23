# Fxfs Dynamic I/O Stress Benchmarks

This directory contains dynamic storage stress benchmarks designed to evaluate
filesystem responsiveness, dirty-page backpressure, and page reclamation
behavior under realistic CPU and Memory load.

Unlike the microbenchmarks in
[src/storage/benchmarks](file:///usr/local/google/home/bcastell/fuchsia/src/storage/benchmarks)
which test isolated operations, these benchmarks run concurrent, multi-threaded
workloads to simulate real-world multitasking environments and reveal
pathological storage edge cases.

## Core Workload Design & Client Mapping

The benchmarks run a mix of concurrent subcommands designed to emulate typical
consumer client device storage profiles (e.g., desktop or multimedia patterns):
* **Sequential Write/Read (Throttled)**: Emulates bulk streaming operations.
  * *15 MiB/s Write (No Mid-Stream Fsync)*: Emulates a web browser downloading
    a large file or software update. Chunks are buffered in memory and rely on
    OS background dirty page writeback, issuing a single `fsync` on final close.
  * *15 MiB/s Write (Periodic 64 MiB Fsync)*: Emulates continuous video recording
    or multimedia capture, where file buffers and container headers are flushed
    periodically to guarantee crash recoverability.
  * *10 MiB/s Read*: Maps to active media playback (streaming video or viewing
    high-resolution photo galleries).
* **Random Write/Read (Throttled)**: Emulates SQLite WAL database transactions
  (such as web browser history commits or application state persistence).
  * *1 MiB/s (4 KB blocks, Fsync Every 16 Ops)*: Emulates active foreground
    transaction commits and background sync checks competing for controller IOPS.

This concurrency evaluates the "choking" effect where heavy sequential writes
saturating the device pipeline starve critical, latency-sensitive database I/O.

## Evaluation Tracks & Disaggregated Catapult Metrics

To clearly expose I/O starvation and tail latency degradation on Chromeperf
dashboards, metrics are emitted using a disaggregated CamelCase
taxonomy (`<TrackName>/<Role>/<Metric>`), capturing exact `p95` and `p99` tail
latencies alongside throughput across worker roles (`seq_writer`, `seq_reader`,
`rand_writer`, `rand_reader`).

The suite evaluates 3 core tracks across Light and Heavy concurrency mixes:

1. **Track 1: Fairness** - Evaluates background client starvation on an unloaded system.
   * **Run 1.1: Fairness (Light)** - Emulates active SQLite WAL transactions
     (random R/W) while a large browser download streams in background.
   * **Run 1.2: Fairness (Heavy)** - Emulates active database commits and media
     playback running concurrent with continuous video recording (periodic 64 MiB
     flushes) and a background file download (no mid-stream flushes).
2. **Track 2: Correctness** - Tests whether the system maintains stability under
   unconstrained pathological throughput.
   * **Run 2.1: Correctness (Light)** - Unconstrained sequential write/read +
     unthrottled random clients. Evaluates raw physical bandwidth arbitration.
   * **Run 2.2: Correctness (Heavy)** - Symmetric unconstrained streaming and
     random access. Checks if the system survives dirty page cache exhaustion
     without kernel OOM panics.
3. **Track 3: Stress** - Evaluates the fairness mixes under extreme contention.
   * **Run 3.1: Stress (Light)** - Emulates Fairness (Light) under heavy CPU
     scheduling contention (simulating background trace profiling overhead).
   * **Run 3.2: Stress (Heavy)** - Emulates Fairness (Heavy) under combined
     CPU load and kernel memory pressure (`k mem` Warning state).

---

## Experimental Findings & Shortcomings

### 1. Kernel Clean Page Reclamation Delays
* **Observation**: Zircon's virtual memory scanner does not immediately
  reclaim clean, written-back pages from memory. A fast sequential writer can
  generate dirty pages faster than they are reclaimed, leading to Out of Memory
  (OOM) reboots even when the written data has already been flushed to disk.
* **Mitigation**: While sending `DONT_NEED` signals from user space forces
  reclamation, this is not a clean architectural solution for the filesystem.
  The future implementation of **Writeback V2** aims to natively address this
  by coordinating page eviction directly with writeback completion.

### 2. "Stop the World" Flushes on Critical Pressure
* **Observation**: When `Fxfs` receives a `Critical` memory pressure signal,
  it currently performs a global, synchronous flush ("stop the world"). This
  leads to severe, multi-second (or even multi-minute) stalls for client
  threads, directly impacting responsiveness.
* **Next Steps**: We are exploring more graceful degradation policies, such
  as:
  * Avoiding full synchronous halts.
  * Partially flushing only enough dirty pages to unblock the memory subsystem.

### 3. Dirty Page Backpressure (Soft/Hard Limits)
* **Observation**: Introducing soft and hard dirty page limits to throttle
  fast writers significantly reduces worst-case write and `fsync` latencies.
  Under certain conditions, enforcing these backpressure limits actually
  *increases* overall throughput by reducing queue thrashing and optimizing
  disk scheduling.

---

## Usage

To run the orchestrated test suite across all evaluation tracks:
```bash
fx test fuchsia-pkg://fuchsia.com/io-stress-test#meta/io_stress_suite.cm
```

To run a specific evaluation track (e.g. `fairness`, `correctness`, or `stress`):
```bash
fx test fuchsia-pkg://fuchsia.com/io-stress-test#meta/io_stress_suite.cm -- --track fairness
```

For manual one-off experimentation with specific workload combinations, you can invoke the standalone `io_stress` component directly:
```bash
fx test fuchsia-pkg://fuchsia.com/io-stress-test#meta/io_stress.cm -- random --rate-mibs 1 + sequential
```
