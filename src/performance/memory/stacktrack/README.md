# Stacktrack

Stacktrack is a profiling tool that records the peak stack usage observed for
each thread in a process.

The Stacktrack library runs within the profiled process and wraps the Zircon
VDSO symbols. Therefore, whenever the process makes a syscall, its current stack
usage will be analyzed before passing control to the real syscall handler.

Conversely, stacks that do not bottom-out in a syscall (e.g. pure calculations
or recursive calls performed entirely in user-space) will **not** be seen or
recorded by this tool.

## Profiling individual components

* Add `//src/performance/memory/stacktrack/instrumentation/collector.shard.cml`
  to the `include` list in your component's manifest.
* Add `//src/performance/memory/stacktrack/collector` to the `subpackages` of
  your package.
* C++:
  * Add `//src/performance/memory/stacktrack/instrumentation` to the `deps` of
    the `executable` target that you want to profile.
  * Add `#include <stacktrack/bind.h>` and call `stacktrack_bind_with_fdio()` at
    the beginning of `main` in your program.
* Rust:
  * Add `//src/performance/memory/stacktrack/instrumentation:rust` to the `deps`
    of the `rustc_binary` target that you want to profile.
  * Call `stacktrack::bind_with_fdio()` at the beginning of `main` in your
    program.

* Run your program as usual.
* Use `ffx profile stacktrack` while your program is running to dump the
  peak stack usage of all the threads in the process observed so far.

### Quickstart: Running the example

```
# Include stacktrack's example component in the build.
fx set ... --with src/performance/memory/stacktrack/example

# Build and run Fuchsia as usual, then start the example component.
ffx component run /core/ffx-laboratory:example fuchsia-pkg://fuchsia.com/stacktrack-example#meta/stacktrack-example.cm

# Dump the peak stack usage report in Markdown format.
ffx profile stacktrack

# OR: dump machine-readable JSON.
ffx --machine json-pretty profile stacktrack
```

## Design

The Stacktrack library interposes calls to the Zircon VDSO and tracks the
deepest stack trace observed for each thread. It maintains a list of active
threads in a shared VMO, allowing external tools to inspect the state of the
process at any time without requiring cooperation from the process itself (i.e.
without taking locks or pausing execution).

Each instrumented process shares a read-only handle to its VMOs to a centralized
component called "stacktrack-collector". The collector can then easily take a
snapshot, at any time and without any further cooperation from the instrumented
process, by simply creating a `ZX_VMO_CHILD_SNAPSHOT` of the threads VMO.

In order to guarantee that the resulting snapshot is always consistent, the
instrumentation updates the VMO using atomic operations. Specifically, when a
thread's stack trace needs to be updated (because a deeper stack is observed), a
new node is inserted into the list, and then the old node is removed. This
insert-then-remove pattern ensures that the thread remains visible from the
reader's perspective at all times.

### VMO format

The Stacktrack VMO consists of a header followed by an array of nodes, indexed
by zero-based integers. A linked list is built on top of this array, allowing a
list of active threads to be maintained without requiring contiguous storage.

Each node represents a thread, storing the deepest stack trace observed so far.
This structure allows a reader to reconstruct the state of all active threads
simply by traversing the linked list from the header.
