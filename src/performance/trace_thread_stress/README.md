# Trace Thread Stress

A program to stress the tracing system with a large number of threads under one
process. This aims to help uncover race conditions within the tracing system.

## Running the workload

Include the component in your build:

```
fx set ... --with //src/performance/trace_thread_stress
```

Then run the workload:

```
ffx component run core/trace_manager/workloads:thread_stress
fuchsia-pkg://fuchsia.com/trace_thread_stress#meta/trace_thread_stress.cm
```

This directory also contains `take_trace.sh` which will start a trace in
streaming mode before running the workload, which is convenient when trying out
changes to the tracing system.

## Configuring the workload

The workload takes three parameters via structured config:

* `thread_count` The number of threads to generate trace data from. Defaults to
  100.
* `duration_ms` The length of time to run the workload. Defaults to 10000 (10
  seconds).
* `interval_ms` The time each thread waits between writing trace events.
  Defaults to 10.

These can be set in the `ffx component run` command, for example

```
ffx component run core/trace_manager/workloads:thread_stress
fuchsia-pkg://fuchsia.com/trace_thread_stress#meta/trace_thread_stress.cm
--config "thread_count=10"
```

will run the workload with only 10 threads instead of 100.
