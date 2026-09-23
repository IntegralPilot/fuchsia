<!--
Copyright 2026 The Fuchsia Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Analyze traces with PerfettoSQL and fx perf-analyze

After recording a Fuchsia trace (`.fxt`), you can query and analyze the
underlying trace data programmatically using SQL through Perfetto's Trace
Processor engine. While the Perfetto UI provides interactive timeline
visualization, SQL analysis enables automated anomaly detection, quantitative
performance regression analysis, and batch metric aggregation.

For an introduction to the core PerfettoSQL dialect, query syntax, and standard
tables, see the official
[PerfettoSQL getting started][perfetto-sql-getting-started] and
[Perfetto SQL tables][perfetto-sql-tables] documentation.

This guide explains how to analyze Fuchsia traces using:
* [Choosing the right trace analysis tool](#choosing-the-right-tool)
* [High-level performance triage with `fx perf-analyze`](#high-level-triage-with-fx-perf-analyze)
* [Fuchsia trace schema and data model](#fuchsia-trace-schema)
* [Common Fuchsia PerfettoSQL recipes](#common-fuchsia-perfettosql-recipes)
* [Running queries across tools and environments](#running-queries-across-tools)


## Choosing the right trace analysis tool {:#choosing-the-right-tool}

Fuchsia developers have three main ways to query and inspect trace data. Choose
the tool that best fits your workflow:

* **[Perfetto Web UI][perfetto-ui]**

  **Visual exploration & ad-hoc inspection**: Interactively browse timeline
  tracks, inspect slice details, run ad-hoc SQL queries in the browser, and
  share permalinks.

* **`fx perf-analyze`** *(Recommended for Fuchsia)*

  **Automated triage & scripted analysis**: Run curated diagnostic plugins
  (`cpu`, `binder`), generate structured output (`markdown`, `json`, `text`),
  and run batch SQL queries directly from the host terminal. This is the
  preferred interface for agentic interactions and automated workflows.

* **`trace_processor_shell`**

  **Standalone & non-Fuchsia environments**: The underlying engine behind
  PerfettoSQL. Use when developing outside a Fuchsia source checkout (where
  `fx` is unavailable), building custom Python/C++ automation against upstream
  Perfetto SDKs, or using the interactive terminal REPL.


## High-level performance triage with `fx perf-analyze` {:#high-level-triage-with-fx-perf-analyze}

For common triage workflows, you do not need to write raw SQL queries from
scratch. Fuchsia provides **`fx perf-analyze`**, a host-side CLI tool that
encapsulates standard performance heuristics into automated analysis plugins
and provides a flexible query runner.

`fx perf-analyze` supports local `.fxt` files as well as Perfetto UI permalink
URLs (automatically downloading and caching the trace).

### Automated triage plugins (`fx perf-analyze analyze`)

The `analyze` subcommand executes specialized diagnostic plugins:

```posix-terminal
fx perf-analyze analyze --trace <TRACE_FILE_OR_URL> --plugin <PLUGIN_NAME>
```

You can choose formatted Markdown table output (`--format markdown`), structured
JSON (`--format json`), or tab-separated text (`--format text`).

#### 1. CPU utilization and idle power triage (`cpu` plugin)

The `cpu` plugin runs a multi-dimensional diagnostic suite to audit system
activity, excessive wakeups, and power blockers:

* **Restless Sleepers (Wakeup Counts):** Identifies threads with high context
  switch/wakeup frequencies and short runtimes, pinpointing timer thrashing or
  uncoalesced polling loops that prevent CPU cores from entering deep sleep
  states.

* **Per-Core Utilization & Processing Rate:** Evaluates core duty cycles (%
  active non-idle time) weighted by DVFS frequency scaling counters
  (`Processing Rate:CPU:N`).

* **Top CPU Consumers (Usual Suspects):** Ranks threads by total accumulated
  CPU runtime.

* **Async Executor Overhead:** Measures time spent in Fuchsia async futures
  polling (`executor` / `fuchsia_async`).

* **Binder IPC Breakdown:** Quantifies Starnix/Android Binder IPC transaction
  volume and latencies.

* **Suspend and Wake Lease Tracking:** Audits System Activity Governor (SAG)
  suspend attempts and active wake leases preventing system suspend.

```posix-terminal
fx perf-analyze --format markdown analyze \
  --trace trace.fxt \
  --plugin cpu \
  --limit 10
```

#### 2. Starnix Binder & IPC bottleneck analysis (`binder` plugin)

The `binder` plugin audits IPC transaction delays and scheduling bottlenecks:

* **Missed Wakeups:** Detects scheduling delays where a thread was runnable (`R`
  state) but waited excessively long before being scheduled onto a CPU.

* **Binder Transaction Delays:** Measures end-to-end flow latencies across
  client dispatch, queue wait, and server processing.

```posix-terminal
fx perf-analyze --format markdown analyze \
  --trace trace.fxt \
  --plugin binder \
  --threshold-ms 10.0
```

### Running custom and batch queries with `fx perf-analyze query`

To run ad-hoc SQL queries from the command line:

```posix-terminal
fx perf-analyze --format markdown query \
  --trace trace.fxt \
  --sql "SELECT count(*) AS total_slices FROM slice"
```

To execute multiple queries in a single Trace Processor ingestion session
(avoiding repeated trace parsing overhead), pass a JSON query batch using
`@filepath`:

```json
[
  {
    "name": "total_slices",
    "sql": "SELECT count(*) AS count FROM slice;"
  },
  {
    "name": "unique_threads",
    "sql": "SELECT count(DISTINCT utid) AS count FROM thread;"
  }
]
```

```posix-terminal
fx perf-analyze --format json query \
  --trace trace.fxt \
  --batch @queries.json
```

## Fuchsia trace schema and data model {:#fuchsia-trace-schema}

When Perfetto Trace Processor ingests a Fuchsia trace (`.fxt`), it parses the
binary records into relational SQLite tables. For complete schema references,
see [Perfetto SQL tables][perfetto-sql-tables].

Key concepts and ID relationships specific to Fuchsia traces include:

* **Units:** All timestamps (`ts`) and duration values (`dur`) are stored as
  integer **nanoseconds**. Convert nanoseconds to milliseconds by dividing by
  `1e6` (for example, `dur / 1e6 AS dur_ms`).

* **Processes and Threads:**

  * `process.name`: Fuchsia user space processes are identified by their
    component URL/manifest name (for example, `archivist.cm`, `netstack.cm`,
    `scenic.cm`) or kernel process name (`kernel`).

  * `process.pid` and `thread.tid`: Match Fuchsia Kernel Object IDs (KOIDs).

  * `upid` and `utid`: Unique internal IDs generated by Trace Processor to
    distinguish processes and threads across PID/TID recycling.

* **Tracks and Slices (Join Hierarchy):**

  * Slices are associated with timeline tracks, not threads directly.

  * To associate a `slice` with its `thread` and `process`, join via
    `thread_track`:

    ```sql
    FROM slice s
    JOIN thread_track tt ON s.track_id = tt.id
    JOIN thread t USING (utid)
    JOIN process p USING (upid)
    ```

* **Flows (Cross-Process / Async Causality):**

  * The `flow` table maps causal relationships between asynchronous
    operations, such as client-to-server FIDL calls.

  * `flow.slice_out` identifies the originating client slice, and
    `flow.slice_in` identifies the server handling slice.

* **Kernel Counters & Power Categories:**

  * Traces captured with specialized categories emit counters under the
    `kernel` process (such as `Processing Rate:CPU:N` for CPU frequency
    scaling under `kernel:power`).

  * For details on how CPU frequency and bandwidth demand counters are captured
    and scaled, see [Recording CPU frequency in a
    trace][recording-cpu-frequency].


## Common Fuchsia PerfettoSQL recipes {:#common-fuchsia-perfettosql-recipes}

### 1. Measure FIDL IPC latency across processes {:#measure-fidl-latency}

Fuchsia components emit flow events to trace FIDL calls across process
boundaries. This query measures the transfer latency from client request
dispatch (`slice_out`) to server execution start (`slice_in`), along with
server execution time:

```sql
SELECT
  client_proc.name AS client_process,
  server_proc.name AS server_process,
  slice_out.name AS client_operation,
  slice_in.name AS server_operation,
  ROUND((slice_in.ts - slice_out.ts) / 1e6, 3) AS transfer_latency_ms,
  ROUND(slice_in.dur / 1e6, 3) AS server_dur_ms
FROM flow
JOIN slice AS slice_out ON flow.slice_out = slice_out.id
JOIN slice AS slice_in ON flow.slice_in = slice_in.id
JOIN thread_track AS client_tt ON slice_out.track_id = client_tt.id
JOIN thread AS client_th ON client_tt.utid = client_th.utid
JOIN process AS client_proc ON client_th.upid = client_proc.upid
JOIN thread_track AS server_tt ON slice_in.track_id = server_tt.id
JOIN thread AS server_th ON server_tt.utid = server_th.utid
JOIN process AS server_proc ON server_th.upid = server_proc.upid
ORDER BY transfer_latency_ms DESC
LIMIT 20;
```

### 2. Top time-consuming operations in a component {:#top-time-consuming-operations}

To find which operations consume the most aggregate time within a specific
component (e.g. `netstack.cm`):

```sql
SELECT
  slice.name AS operation,
  COUNT(*) AS count,
  ROUND(SUM(slice.dur) / 1e6, 3) AS total_dur_ms,
  ROUND(AVG(slice.dur) / 1e6, 3) AS avg_dur_ms,
  ROUND(MAX(slice.dur) / 1e6, 3) AS max_dur_ms
FROM slice
JOIN thread_track ON slice.track_id = thread_track.id
JOIN thread USING (utid)
JOIN process USING (upid)
WHERE process.name LIKE '%netstack%' AND slice.dur > 0
GROUP BY slice.name
ORDER BY total_dur_ms DESC
LIMIT 20;
```

### 3. Identify restless sleepers (thread wakeups and churn) {:#restless-sleepers}

Threads with high context switch / wakeup counts but very low average running
durations (e.g. `< 0.1 ms`) indicate polling churn or timer thrashing:

```sql
SELECT
  t.name AS thread_name,
  t.tid AS tid,
  p.name AS process_name,
  COUNT(*) AS wakeup_count,
  ROUND(AVG(ts.dur) / 1e6, 3) AS avg_duration_ms,
  ROUND(SUM(ts.dur) / 1e6, 3) AS total_duration_ms
FROM thread_state ts
JOIN thread t USING (utid)
LEFT JOIN process p USING (upid)
WHERE ts.state = 'Running'
GROUP BY utid
ORDER BY wakeup_count DESC
LIMIT 20;
```

### 4. CPU execution time per thread (`sched` table) {:#cpu-execution-time}

When a trace is captured with `kernel:sched`, the `sched` table records CPU
quantum durations:

```sql
SELECT
  process.name AS process_name,
  thread.name AS thread_name,
  COUNT(*) AS context_switches,
  ROUND(SUM(sched.dur) / 1e6, 3) AS total_cpu_time_ms,
  ROUND(AVG(sched.dur) / 1e3, 3) AS avg_quantum_us
FROM sched
JOIN thread USING (utid)
JOIN process USING (upid)
WHERE sched.dur > 0
GROUP BY process.name, thread.name
ORDER BY total_cpu_time_ms DESC
LIMIT 20;
```

### 5. Inspect trace event arguments {:#inspect-event-arguments}

To query key-value arguments attached to trace slices:

```sql
SELECT
  slice.name AS slice_name,
  args.key AS arg_name,
  COALESCE(
    args.string_value,
    CAST(args.int_value AS TEXT),
    CAST(args.real_value AS TEXT)
  ) AS arg_value
FROM slice
JOIN args ON slice.arg_set_id = args.arg_set_id
WHERE slice.name = 'ChannelMessage'
LIMIT 50;
```

### 6. Time interval overlap matching {:#interval-overlap-matching}

When querying slices or events that overlap with a specific time window
`[start_ts, end_ts]`, use overlap logic rather than strict containment to
capture events starting before or ending after the window:

```sql
SELECT
  slice.name,
  slice.ts,
  slice.dur
FROM slice
WHERE (slice.ts + slice.dur) >= <START_TS>
  AND slice.ts <= <END_TS>;
```


## Running queries across tools and environments {:#running-queries-across-tools}

### 1. Perfetto Web UI Query tab

You can run SQL queries interactively inside the [Perfetto UI][perfetto-ui]:
1. Open your `.fxt` file in [https://ui.perfetto.dev][perfetto-ui].
2. Click **Query (SQL)** in the left sidebar.
3. Enter your query and press `Ctrl+Enter` (or `Cmd+Enter` on macOS).

![Perfetto SQL Query interface](images/sql_query_interface.png "The Query (SQL) tab in the Perfetto UI"){: width="600"}

#### Inspecting large traces with HTTP RPC mode

If a trace file is too large to load smoothly in web browser memory, you can run
Trace Processor locally as an HTTP RPC daemon:

```posix-terminal
./prebuilt/third_party/perfetto/trace_processor_shell/linux-x64/trace_processor_shell \
  --httpd --http-port 9001 trace.fxt
```

Once running, navigate to [https://ui.perfetto.dev][perfetto-ui] and select
**Open with HTTP RPC** in the navigation bar.

### 2. Standalone Trace Processor Shell (`trace_processor_shell`)

`trace_processor_shell` is the standalone binary engine developed upstream by
Perfetto that powers both `fx perf-analyze` and Perfetto's analytical tools.

Fuchsia source checkouts include prebuilt binaries under:
```posix-terminal
prebuilt/third_party/perfetto/trace_processor_shell/<PLATFORM>/trace_processor_shell
```
Where `<PLATFORM>` is `linux-x64`, `linux-arm64`.

If you are working outside a Fuchsia checkout, you can download the binary
directly from Perfetto:
```posix-terminal
curl -LO https://get.perfetto.dev/trace_processor
chmod +x ./trace_processor
```

* **Interactive REPL:**

  ```posix-terminal
  ./prebuilt/third_party/perfetto/trace_processor_shell/linux-x64/trace_processor_shell \
    trace.fxt
  ```

* **Run an inline query with `fx perf-analyze`:**

  ```posix-terminal
  fx perf-analyze query \
    --trace trace.fxt \
    --sql "SELECT count(*) AS total_slices FROM slice;"
  ```

* **Run a batch query file with `fx perf-analyze`:**

  ```posix-terminal
  fx perf-analyze query \
    --trace trace.fxt \
    --batch @queries.json
  ```

<!-- Reference links -->

[perfetto-sql-getting-started]: https://perfetto.dev/docs/analysis/perfetto-sql-getting-started
[perfetto-sql-tables]: https://perfetto.dev/docs/analysis/sql-tables
[perfetto-ui]: https://ui.perfetto.dev/
[recording-cpu-frequency]: /docs/development/tracing/advanced/recording-cpu-frequency.md
