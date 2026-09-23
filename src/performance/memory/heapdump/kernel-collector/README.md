# Heapdump kernel memory profiler

Heap allocation profiling for Zircon kernel. It captures live allocations at
any point in time and export that in a
[pprof](https://github.com/google/pprof)-compatible protobuf format.

## Building

To enable zircon heap profiler append
`--args kernel_memory_profiler=true` and
`--with src/performance/memory/heapdump/kernel-collector`
to the `fx set` invocation.

Note: Kernel memory profiler and HEAP_COLLECT_STATS are mutually exclusive.

## Running

Use `ffx component run` to launch this component into any realm that can read bootfs:

```
ffx component run /core/ffx-laboratory:kernel-collector fuchsia-pkg://fuchsia.com/kernel-collector#meta/kernel-collector.cm
```

Then collect a profile with:

```
fx ffx profile heapdump snapshot --by-koid 1 --output-file /tmp/profile.pb.gz --collector core/ffx-laboratory:kernel-collector
```

This outputs a serialized `perftools.profiles.Profile` protocol buffer, that can
be turned into a flame graph with:

```
fx pprof -flame /tmp/profile.pb.gz
```

## Tuning

You can verify the kernel profile buffer size by inspecting the logs:

```
fx log --filter kernel-collector
```

The buffer size can be modified in `//zircon/kernel/lib/heap/heap_wrapper.cc`.

