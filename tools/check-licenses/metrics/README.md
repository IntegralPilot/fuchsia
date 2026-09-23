# `tools/check-licenses/metrics`

The `metrics` package provides a high-performance, thread-safe, and zero-dependency
Observability (O11y) and metrics collection framework for the `check-licenses` tool.

Because `check-licenses` heavily parallelizes its filesystem traversal and
license classification phases, this package is designed to safely aggregate data
across thousands of concurrent goroutines without creating massive lock contention.

## Core Architecture

This package moves away from traditional "flat" metrics (e.g., `NumFiles = 42`)
and uses a **Dimensional Metrics** model.

### `Registry` (`registry.go`)
The central hub for all metrics. It holds maps of active `Counters` and `Timers`.
*   **Thread Safety:** Operations that register new metrics are protected by a
    global `sync.RWMutex`. Note: While sufficient for current workloads, this
    global lock is a known potential performance bottleneck under extreme
    concurrency and may be optimized in the future (e.g., by sharding the
    registry).
*   **Exporting:** The `Export(filepath)` function safely locks the entire
    registry, marshals it to JSON, and writes it to disk. This is typically
    called at the very end of the `result.SaveResults()` pipeline.

### `Counter` (`counter.go`)
A dimensional counter tracks incrementing values partitioned by predefined
labels (tags).
*   **Initialization:** When registering a counter via `RegisterCounter()`, you
    must declare its `LabelKeys` (e.g., `["extension", "status"]`).
*   **Incrementing:** To increment, call `Inc("txt", "cached")`. The function
    will panic if the number of provided values does not perfectly match the
    expected keys.
*   **Performance:** Under the hood, the labels are concatenated into a string
    key (e.g., `"txt,cached"`). A localized `sync.RWMutex` on the struct
    protects the underlying integer map, allowing massive parallel increments
    with minimal overhead.

### `Timer` (`timer.go`)
A performance tracking tool designed to answer "Where is the tool spending
its time?".
*   **Deferred Tracking:** It uses Go's closure pattern for extreme ease of use.
    By putting `defer metrics.PhaseDuration.Track()()` at the top of a
    function, the timer automatically records the start time, and upon function
    exit, calculates the elapsed duration.
*   **Aggregations:** It tracks `TotalDuration` (useful for finding bottlenecks),
    `CallCount` (how many times the phase ran), and `MaxDuration` (the worst-case
    latency for a single execution of that phase).

## Defining Metrics (`definitions.go`)

To prevent magic strings from leaking across the codebase, all valid metrics
must be explicitly registered as exported variables in `definitions.go`.

This file conceptually separates metrics into two distinct categories:

### 1. Domain Metrics
These metrics describe *what the tool actually found* in the repository (the
compliance state).
*   Example: `LicenseDetected` (labeled by `spdx_id`, `policy_category`,
    and `ecosystem`). This tells the OSRB exactly how the licensing makeup
    of the tree is shifting over time.

### 2. Operational Metrics
These metrics describe *how well the tool is running* (performance and efficiency).
*   Example: `FilesProcessed` (labeled by `extension` and `status` like "cached"
    vs "analyzed"). This proves whether optimization heuristics (like lazy
    loading) are actually saving I/O.
*   Example: `PhaseDuration` to track time spent in traversal vs. template
    expansion.
