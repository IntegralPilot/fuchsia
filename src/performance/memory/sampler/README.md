# Memory Sampler

`memory_sampler` is a continuous fleetwide sampling heap profiler:

  * Continuous: continuously capture profiles from a running process.
  * Fleetwide: designed to run on devices in the field; profiles are attached to
    crash reports, and can be similarly accessed.
  * Sampling: uses a poisson process to randomly sample allocations based on
    size; this reduces the overhead of the profiler, and makes it suitable to
    get a view of the more interesting allocations without capturing every
    single allocation of a process.
  * Heap profiler: produces (`pprof` compatible) profiles of the heap
    allocations and deallocations of the instrumented process.

`memory_sampler` comes with three parts:

  * `fuchsia.memory.sampler.Sampler`: a FIDL API which describes an interface to
    communicate allocation metadata between a process and a profiler.
  * `memory_sampler`: a Rust component which provides an implementation for this
    API.
  * `libsampler_instrumentation`: a shared library that one can link against to
    automatically instrument the allocator and communicate allocation metadata
    through the FIDL API.

`memory_sampler` supports profiling arbitrary processes that use the platform
default allocator, Scudo. This includes any C, C++ and Rust program built via
the SDK that don't configure an alternative allocator.

## Getting started

`memory_sampler` can be used to continuously profile in-tree
components. Out-of-tree components are not supported yet, but will once the
relevant parts have been added to the Fuchsia SDK.

### In-tree components

To profile a component with `memory_profiler`:

  1. Add the `memory_sampler` Fuchsia package target to your build (e.g. `fx set
     ... --with //src/performance/memory/sampler:memory_sampler`).
  2. Add this package in an appropriate realm.  See
     `//src/performance/memory/sampler/meta/memory_sampler.core_shard.cml` for
     an example shard to include in the `core` realm manifest, to include
     `memory_sampler` as a `core` component. `memory_sampler`'s URL is
     `fuchsia-pkg://fuchsia.com/memory_sampler#meta/memory_sampler.cm`, and it
     depends on the `fuchsia.feedback.CrashReporter` and
     `fuchsia.feedback.CrashReportingProductRegister` capabilities, both offered
     by the `feedback` `core` component.

  3. Route the `fuchsia.memory.sampler.Sampler` capability from `memory_sampler`
     to the instrumented process.

  4. Add the `//src/performance/memory/sampler/instrumentation:lib` shared
     library as a dependency of your binary.

When your component starts, `component_manager` will ensure the configured
instance of `memory_sampler` is running, and your process will regularly
communicate allocation information to `memory_sampler`.

At most once an hour, `memory_sampler` will file a crash report with the
`feedback` service that contains a `pprof`-compatible profile. This profile can
be symbolized and visualized by running the `fx pprof -flame
<path_to_the_profile>` command.

Note: on `core`, `feedback` will write the filed crash reports in a temporary
storage on the device. They can be found via the `fx shell "find /tmp /data
-name <your_program_name>*"` command.

### Out-of-tree components

Currently, out-of-tree components won't have access to `memory_sampler` because
it is entirely internal. However, we do plan to make it generally available via
the SDK; stay tuned.

## Memory Profiles

### Content

The profiles produced by `memory_sampler` contain 6 types of samples:

  * `residual_allocated_objects`: a count of (sampled) allocations that are
    still alive at the time of the production of the profile.
  * `residual_allocated_space`: the size of (sampled) allocations that are still
    alive at the time of the production of the profile.
  * `allocated_objects`: a count of (sampled) allocations observed over the
    duration of the profile (both alive and dead).
  * `allocated_space`: the size of (sampled) allocations observed over the
    duration of the profile (both alive and dead).
  * `deallocated_objects`: a count of (sampled) deallocations observed over the
    duration of the profile.
  * `deallocated_space`: the size of (sampled) deallocations observed over the
    duration of the profile.

Note: because `memory_sampler` is a sampling profiler, it only observes a
(randomly selected) subset of allocations. Moreover, the selection is based on
average memory allocated between samples: it skews the distribution towards
larger allocations. For this reason, counts and space are always underestimated,
but how much depends on the allocation profile of the instrumented process
(e.g. a process that only does large allocations is more likely to produce an
accurate profile than a process that performs a lot of very small
allocations). Nevertheless, outliers are still likelier to get sampled; chances
are that if your process suffers from an unforeseen pathological allocation
pattern, they will tend to show up on profiles.

### Profile filing frequency

`memory_sampler` produces profiles under roughly three conditions:

  1. A partial profile once the number of recorded stack traces that can
     be reclaimed reaches a threshold, to reduce memory use. This can be
     somewhat unpredictable, because it depends on the (de)allocation
     patterns and call sites.

  2. A partial profile at least once every 12 hours.

  3. A final profile when the instrumented process exits.

### Partial vs final profiles

`memory_sampler` regularly files partial profiles during the lifetime of the
instrumented process (depending on observed allocations), as well as a single
final profile at the end of the process. Both kind of profiles have the same shape, but different semantics:

  * Partial profiles: residual allocations are either leaks *or* allocations
    that have simply not been deallocated yet. Dead allocations correspond to
    allocations that have been deallocated *within the period covered by this
    profile*. In particular, if an allocation was deallocated in profile X, it
    won't appear in the list of dead allocations of profile X+N. This is done to
    restrict the growth of partial profiles.  Partial profiles also come with an
    iteration number; this number is meaningless, except that profiles come in
    numerical order (i.e. profile 1000 was captured earlier than profile 9999
    regardless of when they were captured).
  * Final profiles: residual allocations are allocations that were never
    deallocated within the lifetime of the process. If an instrumented process
    has memory leaks, they are very likely to appear here (if they were
    sampled). Dead allocations correspond to allocations that have been
    deallocated *between the last partial profile and the end of the process*,
    not over the entire lifetime of the process. This limitation is the result
    of a memory optimization.

Note: If one needs a picture of the overall allocations of a process over its
lifetime, one should look into every single profile captured during the lifetime
of the process; it's possible to get an accurate summary by merging every single
partial profile (discarding live allocations) with a final profile, but we don't
provide (yet) a script to perform this task.

## Performance tuning

Currently, `memory_sampler` hard-codes all its performance parameters; on a
private build, we encourage you to tune them to your liking. Some available
performance knobs follow:

  * `memory_sampler::Recorder::kSamplingIntervalBytes`: the average count of
    bytes allocated between two samples. Reducing this value increases the
    accuracy of the profiler, at the expense of the performance of the
    instrumented process. Note that if the value becomes so small that
    `memory_sampler` is unable to handle the amount of messages it receives,
    this would cause the kernel to kill the instrumented process (because of a
    buffer exhaustion in the FIDL channel).
  * `memory_sampler::sampler_service::RECLAIMABLE_STACK_TRACES_PROFILE_THRESHOLD`:
    the number of recorded stack traces that triggers a partial report. Reducing
    this number decreases the memory footprint of `memory_sampler`, at the
    expense of storage space and bandwidth (because more, smaller profiles get
    filed as a result).
  * `memory_sampler::sampler_service::MAX_DURATION_BETWEEN_PARTIAL_PROFILES`:
    the maximum elapsed time between two partial profiles. Reducing
    this duration will increase the frequency of partial profiles,
    which should in turn reduce the memory consumption of the
    profiler.

Note also that `memory_sampler` comes with built-in throttling of filed
profiles, to limit the rate of filing crash reports; in an `eng` build,
`feedback` does not upload any profile, so it is safe to reduce
`memory_sampler::crash_reporter::MIN_DURATION_BETWEEN_SNAPSHOTS_HOURS` to file
profiles more often (both to reduce latency between capture and consumption of
profiles, as well as to increase the sampling rate without significantly
increasing `memory_sampler`'s memory footprint).
