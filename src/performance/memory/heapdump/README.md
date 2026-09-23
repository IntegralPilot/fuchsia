# Heapdump memory profiler

Memory profiling tool that can capture snapshots of live allocations at any
point in time and export them in a
[pprof](https://github.com/google/pprof)-compatible protobuf format.

## Profiling individual components

* Add `heapdump_instrumentation/collector.shard.cml` to the `include` list in
  your component's manifest.
* Add `//src/performance/memory/heapdump/collector` to the `subpackages` of
  your package.
* C++:
  * Add `//src/performance/memory/heapdump/instrumentation` to the `deps` of the
    `executable` target that you want to profile.
  * Add `#include <heapdump/bind.h>` and call `heapdump_bind_with_fdio()` at
    the beginning of `main` in your program.
* Rust:
  * Add `//src/performance/memory/heapdump/instrumentation:rust` to the `deps`
    of the `rustc_binary` target that you want to profile.
  * Call `heapdump::bind_with_fdio()` at the beginning of `main` in your
    program.

* Run your program as usual.
* Use `ffx profile heapdump snapshot` while your program is running to take a
  snapshot of all the current live allocations. For instance, assuming that your
  program is called `example.cm`:

```
ffx profile heapdump snapshot --by-name example.cm --output-file my_snapshot.pb.gz
```

* Use the `fx pprof` command to analyze the memory profile you acquired:

```
fx pprof -http=":" my_snapshot.pb.gz
```

Note: in alternative, when the component being profiled does not have access to
the package resolver (e.g. bootstrap components) or if subpackages cannot be
used, it is possible to add the collector package directly to the image (e.g.
`--with-base //src/performance/memory/heapdump/collector`) and, instead of
including the predefined shard, instantiate it somewhere else in component
hierarchy that has access to the resolver (the URL to be instantiated will be
`fuchsia-pkg://fuchsia.com/heapdump-collector#meta/heapdump-collector.cm`).
With this approach, the `fuchsia.memory.heapdump.process.Registry` capability
must then be manually routed to the component being profiled.

### Quickstart: Running the example

```
# Include heapdump's example component in the build.
fx set ... --with src/performance/memory/heapdump/example

# Build and run Fuchsia as usual, then start the example component.
ffx component run /core/ffx-laboratory:example fuchsia-pkg://fuchsia.com/heapdump-example#meta/heapdump-example.cm

# Take a live snapshot and process it with pprof.
ffx profile heapdump snapshot --output-file my_snapshot.pb.gz
fx pprof -http=":" my_snapshot.pb.gz
```

### Advanced: dealing with multiple instrumented processes at the same time

By default, the `snapshot` command operates in single-process mode. A snapshot
will only be emitted if exactly one running process has been instrumented.

If more than one running process has been instrumented, there are two
alternative possibilities:

* Use the `--by-name` or `--by-koid` command-line options to select a specific
  process to snapshot (for instance, `--by-name heapdump-example.cm`).
* Use the `--multi-process` option to explicitly allow snapshotting several
  processes in one go.

When `--multi-process` is used, the allocations are tagged with the name and the
koid of the owning process. The `-tag` family of pprof options can be used to
separate them in the UI (e.g. `-tagroot process_name`, `-tagroot process_koid`
or both `-tagroot process_name,process_koid`).

## Profiling several components at the same time

As an alternative to manually modifying each component, it is also possible to
enable heapdump in bulk at build time for all programs.

Add these lines to the `local/BUILD.gn` file (after creating it, if it does not
exist yet, see
[docs/development/build/assembly_developer_overrides.md](../../../../docs/development/build/assembly_developer_overrides.md)):
```
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("heapdump_everywhere") {
  platform = {
    development_support = {
      heapdump = {
        component_manager = true
        monikers = ["/**"]
      }
    }
  }
}
```

Then:
```
# Select the "heapdump" variant at build time, which will automatically
# instrument every built program.
fx set ... \
  --variant heapdump \
  --assembly-override //local:heapdump_everywhere \
  --include-clippy=false  # workaround for https://fxbug.dev/396658029

# Build and run Fuchsia as usual, then take a live snapshot of all the processes
# in one go.
ffx profile heapdump snapshot --output-file my_snapshot.pb.gz --multi-process
fx pprof -http=":" my_snapshot.pb.gz
```

It is possible to change the values in `local/BUILD.gn` to restrict the set of
processes to be profiled:
* `component_manager` controls whether Component Manager should be profiled or
  not.
* `monikers` contains a list of moniker patterns whose matching components will
  be profiled. Examples:
  * `monikers = ["/**"]` matches any component (i.e. profile the whole system).
  * `monikers = ["/core/**"]` matches any descendant of `core`.
  * `monikers = ["/core/*"]` matches only direct children of `core`.

As already described in the previous section, the `-tagroot` mechanism can be
used to separate stack traces belonging to different processes in the UI.

## Design

The instrumentation library intercepts all allocation and deallocation events,
and it keeps track of all live allocations by storing them into a specific VMO
(called "allocations VMO"), which is organized as an hash table containing
the allocated addresses as the keys and metadata as the values.

Each instrumented process shares a read-only handle to its VMOs to a centralized
component called "heapdump-collector". The collector can then easily take a
snapshot, at any time and without any further cooperation from the instrumented
process, by simply creating a `ZX_VMO_CHILD_SNAPSHOT` of the allocations VMO.

In order to guarantee that the resulting snapshot is always consistent, the
instrumentation updates the hash table atomically (i.e. inserting/removing an
allocation corresponds to single atomic operation).

### VMO format

The instrumentation library writes into the shared VMOs and the collector must
be able to correctly parse this data. It is therefore important that they agree
on the data structures' layout.

Because of the atomicity requirement, it is not possible to simply use FIDL to
serialize data into the VMO. Instead, heapdump's `heapdump_vmo` crate contains
ad hoc functions to manipulate the VMOs, that are used by both the
instrumentation and the collector.

However, while the usage of the shared `heapdump_vmo` crate makes it easy to
agree on a common format, it does not solve the issue of forward-compatibility:
we want to support an older instrumentation library connecting to a newer
collector, while retaining the possibility to change the VMO format in a
breaking way. This is the reason why all data structures defined in
`heapdump_vmo` have a version suffix (e.g. `_v1`): the current instrumentation
library always uses the latest version to operate its VMOs but, by making it
possible for different data structure versions to coexist in the code base, the
collector can keep supporting processes linked against older instrumentation
libraries.

The instrumentation library implicitly communicates the version of its data
structures when it registers to the collector over the
`fuchsia.memory.heapdump.process.Registry` FIDL protocol. In particular:

* calling `RegisterV1` corresponds to `allocations_table_v1` and
  `resources_table_v1`

**Note**: only one version has been defined at the moment.
