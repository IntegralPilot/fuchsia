# Fuchsia tracing guides

The [Fuchsia tracing system][fuchsia-tracing-system] allows you to collect and
visualize diagnostic information from Fuchsia components.

Fuchsia's tracing system offers a comprehensive way to collect, aggregate, and
visualize diagnostic tracing information from the Fuchsia user space processes
and the Zircon kernel. Traces, like logs, represent events from a Fuchsia
system, but are fine grained, higher frequency and are meant for machine
consumption to compute other insights and visualizations.

Tracing and profiling are a powerful pair of tools to gain insights into the
performance of a system. In comparison to profiling which samples data every
so often, tracing captures every event in a short duration and can include extra
information with the trace event.

Generally, [profiling][fuchsia-profiling] is more useful if you don't know where
the problem is, and aren't sure where to put trace spans, or you aren't able to
add tracing events.

Tracing involves three high-level steps:

* Instrument: Instrument your code so that you can measure the runtime of
  specific functions by surrounding these pieces of code with trace events and
  add labels to emit during a trace.
* Record: Configure and start a tracing tool (`ffx trace`) to generate a trace.
* Analyze: Visualize the report with a UI such as Perfetto to identify issues
  and potential ways to optimize your code.

## General

* [Use trace events][use-trace-events]: This document explains the various types
  of tracing events that Fuchsia supports and how to use them in your
  components.
* [Analyze traces with PerfettoSQL and fx perf-analyze][analyze-traces-with-sql]:
  Query trace data with SQL, perform automated triage, and calculate performance
  metrics using `fx perf-analyze` and Perfetto Trace Processor.

Fuchsia recommends that you use the [Perfetto UI][perfetto-ui] to view the
traces that you collect from Fuchsia components. For more information on using
Perfetto, see the [Perfetto documentation][perfetto-docs].

## Tutorial

If you're new to tracing, see [Tutorial on Fuchsia tracing][tracing-tutorial].

This tutorial provides a high level overview of tracing and the steps necessary
to add tracing to your Fuchsia components.

## Advanced guides

These guides are for advanced users who already understand the basics
of tracing in Fuchsia:

* [Tracing booting Fuchsia](/docs/development/tracing/advanced/recording-a-boot-trace.md)
* [Kernel tracing](/docs/development/tracing/advanced/recording-a-kernel-trace.md)
* [Asynchronous tracing](/docs/development/tracing/advanced/tracing-asynchronously.md)

<!-- Reference links -->

[fuchsia-tracing-system]: /docs/concepts/kernel/tracing-system.md
[tracing-tutorial]: /docs/development/tracing/tutorial/README.md
[use-trace-events]: /docs/development/tracing/trace_events.md
[analyze-traces-with-sql]: /docs/development/tracing/analyze-traces-with-sql.md
[perfetto-ui]: https://ui.perfetto.dev/
[perfetto-docs]: https://perfetto.dev/docs/
[fuchsia-profiling]: /docs/development/profiling/profiling-cpu-usage.md
