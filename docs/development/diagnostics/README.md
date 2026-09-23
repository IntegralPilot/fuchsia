# Diagnostics and observability on Fuchsia

Fuchsia offers a variety of systems to observe a running Fuchsia system. Each
observability system provides different information and tradeoffs. The
observability systems are:

* [Logging](#logging)
* [Tracing](#tracing)
* [Inspect](#inspect)
* [Cobalt](#cobalt) (telemetry metrics)
* [Snapshots](#snapshots)
* [Detect](#detect)

## Logging

Logs play an important part in diagnostics as they describe human readable
events that have happened on a Fuchsia system. In Fuchsia, a component can
record log messages through the [`fuchsia.logger.LogSink`][logsink-ref]
protocol. [`sdk/lib/syslog/cpp`] and [`src/lib/diagnostics/log/rust`] are the
supported libraries for recording logs on Fuchsia. These libraries provide the
encoder implementation for things such as tags, structured key/value pairs,
metadata, message, and [`fuchsia.logger.LogSink`][logsink-ref] integration. For
more information on how Fuchsia components can write logs, see
[Recording logs][recording-logs].

You should consider using logging if you want the ability to:

* View low frequency messages and events that are human readable.
* View the data in snapshots and other Fuchsia tools.

## Tracing

Fuchsia's [tracing system][tracing-docs] offers a comprehensive way to collect,
aggregate, and visualize fine-grained diagnostic tracing information from
Fuchsia user space processes and the Zircon kernel. Traces represent events from
a Fuchsia system with submicrosecond precision, designed for high-frequency
event recording and post-collection visual analysis.

When combined with the [Perfetto UI][perfetto-ui], tracing allows you to see
instants and durations visually along an interactive timeline.

You should consider using tracing if you want the ability to:

* See instant milestones and duration slices visually on a timeline.

* Measure precise submicrosecond latencies and identify execution bottlenecks.

* Follow asynchronous operations and flow events across processes and threads.

* Correlate user space execution with Zircon kernel scheduling events.

* Query and aggregate trace data offline using SQL in the Perfetto Trace
  Processor.

## Inspect

Fuchsia provides you with Inspect which enables Fuchsia components to expose
structured diagnostic information about their current state. You can then use
the `ffx inspect` tool to query this diagnostic information. For more
information, see [Fuchsia component inspection][inspect-overview] and
[`ffx inspect`][ffx-inspect-ref].

You should consider using Inspect if you want the ability to:

* Monitor the state of a component at a specific given time.
* Represent information about the current state and recent history of a
  component.
* Collect data for low-frequency snapshots of a component's state to enable
  fleet-wide metrics.
* View the Inspect data in snapshots and tools.
* Build visualizations, analysis, and create automated anomaly detection based
  on snapshots.

## Cobalt

Cobalt is a pipeline for collecting metrics data from user-owned devices in the
field and producing aggregated reports.

Cobalt includes a suite of features for preserving user privacy and anonymity
while giving product owners the data they need to improve their products.

When using Cobalt, it is recommended that your component is instrumented with
Inspect. If your component is instrumented with Inspect, [Sampler][sampler-docs]
forwards Inspect diagnostics data to the Cobalt telemetry system for fleet
monitoring.

For more information about Cobalt, see [Cobalt: Telemetry with built-in privacy
][cobalt-readme].

## Snapshots

Fuchsia devices have the ability to capture snapshots that contain information
about the current state of the entire system. This includes information about
the Fuchsia build running on the system, kernel and system logs, and Inspect
data from the system and its components. You can also capture a snapshot with
the [`ffx target snapshot`][ffx-snapshot-ref] tool.

## Detect

Additionally, Fuchsia devices run [Detect][detect-docs] which is able to trigger
crash reports based on periodically snapshotting a subset of components Inspect
data and checking for various conditions, which if true, then cause it to
trigger a crash or feedback report.

## Working with diagnostics and observability

As you work with Fuchsia logs, Inspect, and tracing to diagnose issues, you may
want to use some of these tools:

* [`ffx log`][ffx-log-ref] to retrieve logs from a Fuchsia device.
* [`ffx target snapshot`][ffx-target-snapshot-ref] to take a snapshot of a
  Fuchsia target devices's state.
* [`ffx triage`][ffx-triage-ref] to analyze logs and inspect components from
  Fuchsia snapshots.
* [`ffx inspect`][ffx-inspect-ref] to query data exposed by components through
  the Inspect API.
* [`ffx trace`][ffx-trace-ref] to collect, aggregate, and visualize diagnostics
  tracing information.
* [zxdb][zxdb-docs] to debug your code that is running on Fuchsia. zxdb is a
  console-mode debugger for code running on Fuchsia.

You may also want to:

* [Understand how logs are stored on a Fuchsia device][logs-concepts]
* [Understand how the tracing system collects and aggregates diagnostic information][tracing-docs]

You can also follow the diagnostics codelabs:

* [Implement Inspect for a component][inspect-codelab]
* [Use ffx log to retrieve logs from a device][logging-docs]
* [Use ffx triage to analyze snapshots][triage-codelab]

You can also follow the tracing tutorials and guides:

* [Use ffx trace to record and visualize a trace][tracing-codelab]
* [Analyze traces with PerfettoSQL and fx perf-analyze][analyze-traces-with-sql]
* [Implement asynchronous tracing][tracing-async-guide]

[`sdk/lib/syslog/cpp`]: /sdk/lib/syslog/cpp
[`src/lib/diagnostics/log/rust`]: /src/lib/diagnostics/log/rust
[ffx-target-snapshot-ref]: https://fuchsia.dev/reference/tools/sdk/ffx.md#ffx_target_snapshot
[logsink-ref]: https://fuchsia.dev/reference/fidl/fuchsia.logger.md#LogSink
[ffx-log-ref]: https://fuchsia.dev/reference/tools/sdk/ffx.md#ffx_log
[recording-logs]: /docs/development/diagnostics/logs/recording.md
[inspect-overview]: /docs/development/diagnostics/inspect/README.md
[ffx-inspect-ref]: https://fuchsia.dev/reference/tools/sdk/ffx.md#ffx_inspect
[ffx-snapshot-ref]: https://fuchsia.dev/reference/tools/sdk/ffx.md#ffx_target_snapshot
[detect-docs]: /docs/development/diagnostics/analytics/detect.md
[ffx-triage-ref]: https://fuchsia.dev/reference/tools/sdk/ffx.md#ffx_triage
[understand-log]: /docs/development/diagnostics/logs/viewing.md
[zxdb-docs]: /docs/development/debugger/README.md
[logs-concepts]: /docs/concepts/components/diagnostics/README.md#logs
[inspect-codelab]: /docs/development/diagnostics/inspect/codelab.md
[triage-codelab]: /docs/development/diagnostics/triage/codelab.md
[logging-docs]: /docs/development/diagnostics/logs/README.md
[tracing-docs]: /docs/concepts/kernel/tracing-system.md
[tracing-codelab]: /docs/development/tracing/tutorial/README.md
[analyze-traces-with-sql]: /docs/development/tracing/analyze-traces-with-sql.md
[tracing-async-guide]: /docs/development/tracing/advanced/tracing-asynchronously.md
[perfetto-docs]: https://perfetto.dev/docs/
[perfetto-ui]: https://ui.perfetto.dev/
[sampler-docs]: /docs/development/diagnostics/analytics/sampler.md
[ffx-trace-ref]: https://fuchsia.dev/reference/tools/sdk/ffx.md#ffx_trace
[cobalt-readme]: https://fuchsia.googlesource.com/cobalt/+/main/README.md
