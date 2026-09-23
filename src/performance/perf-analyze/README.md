# Fuchsia Standalone Performance Analysis Tool (`perf-analyze`)

`perf-analyze` is a host-side tool designed to perform standalone, automated,
and human-interactive performance analysis on Fuchsia traces. The tool acts as a
CLI orchestrator that delegates trace processing to specialized analysis
plugins.

---

## Subcommands

1.  **`query`**: Executes SQL queries to inspect and extract structured data
    from traces. Backed by Perfetto's Trace Processor.
2.  **`analyze`**: Executes specialized analysis plugins to identify
    performance anomalies (e.g., binder delays, jank, CPU starvation).
3.  **`visualize`** *(Planned)*: Generates HTML reports, flamegraphs, SVGs, or
    deep-link trampoline URLs for the Perfetto UI.

---

## Global Options

*   **`--format <json|markdown|text>`**: Specifies the output format (default:
    `text`).
    *   `text`: Unformatted tab-separated values (TSV), ideal for shell
        scripting.
    *   `markdown`: Formatted Markdown tables, ideal for doc insertion.
    *   `json`: Structured JSON, ideal for automated tool ingestion.

---

## The `query` Subcommand

The `query` subcommand executes SQL queries against a trace file using
Perfetto's Trace Processor.

### Arguments

*   **`--trace <path_or_url>`** *(Required)*: File path to a local `.fxt` trace
    file or a URL to a remote trace.
*   **`--sql <query_string>`**: Executes a single raw SQL query. Mutually
    exclusive with `--batch`.
*   **`--batch <json_string_or_@filepath>`**: Executes a JSON array of queries
    in the format `[{"name": "query_name", "sql": "select ..."}, ...]`. Use
    `@filepath` to read the array from a local file. Mutually exclusive with
    `--sql`.

---

## Examples

#### 1. Print query help
```shell
fx perf-analyze query --help
```
*Output:*
```
usage: perf-analyze query [-h] --trace TRACE (--sql SQL | --batch BATCH)

options:
  -h, --help     show this help message and exit
  --trace TRACE  Trace file path or URL
  --sql SQL      SQL query to run
  --batch BATCH  Batch JSON or @file
```

#### 2. Run a single SQL query (default TSV format)
```shell
fx perf-analyze query \
  --trace src/performance/perf-analyze/test-data/sample_fxt.fxt \
  --sql "select count(*) as cnt from slice"
```
*Output:*
```
cnt
520
```

#### 3. Run a single SQL query (Markdown format)
```shell
fx perf-analyze --format markdown query \
  --trace src/performance/perf-analyze/test-data/sample_fxt.fxt \
  --sql "select count(*) as cnt from slice"
```
*Output:*
```
| cnt |
| --- |
| 520 |
```

#### 4. Run a batch of queries from a file (JSON format)
```shell
fx perf-analyze --format json query \
  --trace src/performance/perf-analyze/test-data/sample_fxt.fxt \
  --batch @src/performance/perf-analyze/test-data/sample_queries.json
```
*Output:*
```json
[
  {
    "name": "slice_count",
    "results": [
      {
        "cnt": 520
      }
    ]
  },
  {
    "name": "process_count",
    "results": [
      {
        "cnt": 2
      }
    ]
  }
]
```

## The `analyze` Subcommand

The `analyze` subcommand runs specialized analysis plugins against a trace file
to identify specific performance anomalies.

### Arguments

*   **`--trace <path_or_url>`** *(Required)*: File path to a local `.fxt` trace
    file or a URL to a remote trace.
*   **`--plugin <plugin_name>`** *(Required)*: Name of the analysis plugin to
    execute (e.g., `binder`).
*   **`--list-plugins`**: Lists all available analysis plugins.

---

## Available Plugins

### `binder` (Starnix Binder Analysis)

Analyzes Starnix binder delays, missed wakeups (scheduling delays),
late-spawned wakers (`SpawnLooper` instant events), thread pool exhaustion
(`no_available_threads` instant events), and process queue depth backlog
(`process_queue_depth` counters).

> [!TIP]
> **Trace Category Recommendation**: Including the `starnix:binder` trace category
> significantly improves the fidelity and depth of binder analysis.
> With `starnix:binder`, the plugin reconstructs full round-trip IPC transaction flows,
> driver queue wait times, thread pool exhaustion signals, and backlog counters.
> If `starnix:binder` is not found in the trace, the plugin issues an explicit notification
> and falls back to best-effort heuristics using `binder_ioctl` slices and `binder:%` thread
> naming patterns.

The plugin executes 5 diagnostic queries:
1. **Missed Wakeups (Wakeup Latencies)**: Identifies scheduling delays (`Runnable` state duration) for binder threads exceeding the threshold.
2. **Binder Delays (Transaction Queue Latencies)**: Identifies completed and incomplete transaction queue latencies exceeding the threshold.
3. **Spawn Looper Events (Late-Spawned Wakers)**: Identifies `SpawnLooper` thread-scoped instant events (`dur = 0`) and their enclosing syscall/ioctl parent slice context when the binder thread pool exhausts available threads.
4. **No Available Threads Events (Thread Pool Exhaustion)**: Identifies `no_available_threads` process-scoped instant events recorded when an incoming command cannot find an idle thread and queues to the process queue.
5. **Binder Process Queue Depth (Counter Summary)**: Summarizes `process_queue_depth` counter track peaks, averages, and sample counts per process.

#### Fallback Behavior Without `starnix:binder`

When the trace does not contain slices under the `starnix:binder` category:
* An initial **Trace Category Status** section is reported recommending the inclusion of `starnix:binder`.
* **Missed Wakeups** and **Binder Delays** fall back to matching `binder_ioctl` slices and `binder:%` / `binder_%` thread names.
* **Spawn Looper Events**, **No Available Threads**, and **Process Queue Depth** return empty result sets with a note indicating the category was not found and that enabling it will strengthen the analysis.

#### Plugin-Specific Arguments

*   **`--threshold-ms <float>`**: Threshold for scheduling delay and queue
    latency in milliseconds (default: `10.0`). Does not filter instant events or
    queue depth counters.
*   **`--complete-only`**: Only return complete transactions (default: False,
    includes incomplete transactions).

#### Example

```shell
fx perf-analyze --format markdown analyze \
  --trace "https://ui.perfetto.dev/#!/?s=b3615b084da54e9a0742dd8f6280de355df4f513" \
  --plugin binder \
  --threshold-ms 15.0
```

---

### `cpu` (CPU Utilization & Idle Power Diagnostics)

Provides a multi-dimensional diagnostic view of system activity during an idle
trace, identifying timer thrashing, excessive wakeups, per-core utilization and
frequency scaling, async executor overhead, binder IPC chatter, and suspend/wake
lease blockers.

The plugin executes 6 diagnostic queries in prioritized order:
1. **Restless Sleepers (Wakeup Counts)**: Identifies threads with high context
   switch / wakeup counts preventing deep sleep (VddMin).
2. **Per-Core Utilization & Processing Rate**: Analyzes core duty cycles
   (% active non-idle time) and CPU frequency scaling via Fuchsia kernel
   `Processing Rate:CPU:N` counters.
3. **Top CPU Consumers (Usual Suspects)**: Ranks threads by total accumulated
   CPU runtime.
4. **Fuchsia Async Executor Overhead**: Measures runtime and slice counts for
   async executors managing futures.
5. **Binder IPC Breakdown**: Quantifies Starnix/Android IPC transaction traffic
   and latencies broken down by thread and process.
6. **Suspend and Wake Lease Tracking**: Verifies SAG suspend attempts and
   attributes active wake leases to components.

#### Plugin-Specific Arguments

*   **`--limit <int>`**: Maximum number of rows returned for ranked queries
    (default: `15`).

#### Example

```shell
fx perf-analyze --format markdown analyze \
  --trace src/performance/perf-analyze/test-data/sample_fxt.fxt \
  --plugin cpu \
  --limit 10
```

---

## Running Tests

Unit tests are written using standard Python `unittest`. To build and run all
unit tests, execute:

```shell
fx test perf_analyze_test binder_test result_formatter_test cpu_test plugins_test
```
